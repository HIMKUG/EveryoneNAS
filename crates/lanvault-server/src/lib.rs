//! `lanvault-server`: content-addressed multi-volume vault engine, Bit-Rot scrubber,
//! GFS snapshot GC, Argon2id + IP-lockout authentication, and Axum 0.8 Web/WS server.

pub mod auth;
pub mod gc;
pub mod scrub;
mod tls;
pub mod vault;
pub mod web;

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use lanvault_core::{
    blake3_hash, Database, DeviceIdentity, DiscoveryAnnouncement, LanVaultError, MdnsRegistrar,
    PoolMode, Result, RetentionTier, SnapshotRecord, TrustedPeer, VaultRecord,
    VaultVolumeRecord,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot, Notify};
use tracing::{info, warn};

use crate::auth::AuthManager;
use crate::gc::{GarbageCollector, GfsPolicy};
use crate::scrub::{BitRotAlertEvent, BitRotScrubber};
use crate::vault::{unix_timestamp_secs, VaultStore};
use crate::web::{build_router, AppState, WsServerEvent};

/// Periodic background GFS snapshot GC interval (6 hours).
const GC_INTERVAL_SECS: u64 = 6 * 3600;
const GC_CHECK_INTERVAL_SECS: u64 = 10 * 60;
const GC_LAST_RUN_SETTING: &str = "gc_last_run_unix_secs";
const SCRUB_INTERVAL_SECS: u64 = 24 * 3600;
const SCRUB_CHECK_INTERVAL_SECS: u64 = 30 * 60;
const SCRUB_LAST_RUN_SETTING: &str = "scrub_last_run_unix_secs";
const INDEX_RECOVERY_SETTING: &str = "index_recovery_version";

/// Reason why the LanVault HTTP/WS server exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerExitReason {
    Stopped,
    GracefulRestart,
}

/// Server startup configuration (can be loaded from JSON or `key = value` TOML via `--config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub bind_addr: String,
    pub port: u16,
    pub vault_root: PathBuf,
    pub default_quota_bytes: u64,
    pub web_password: String,
    pub device_name: String,
    pub keep_alive_level: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1".to_string(),
            port: 7842,
            vault_root: PathBuf::from("./everyonenas_data"),
            default_quota_bytes: 100 * 1024 * 1024 * 1024,
            web_password: String::new(),
            device_name: "EveryoneNAS-Server".to_string(),
            keep_alive_level: default_keep_alive_level().to_string(),
        }
    }
}

impl ServerConfig {
    /// Loads configuration from a `.json` or simple `key = value` `.toml` file.
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let trimmed = raw.trim();
        if trimmed.starts_with('{') {
            return Ok(serde_json::from_str(trimmed)?);
        }

        let mut cfg = Self::default();
        for (line_no, line) in raw.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                return Err(LanVaultError::InvalidInput(format!(
                    "malformed config line {}: {line}",
                    line_no + 1
                )));
            };
            let key = k.trim();
            let val = v.trim().trim_matches('"').trim_matches('\'');
            match key {
                "bind_addr" => cfg.bind_addr = val.to_string(),
                "port" => {
                    cfg.port = val.parse().map_err(|_| {
                        LanVaultError::InvalidInput(format!("invalid port: {val}"))
                    })?;
                }
                "vault_root" => cfg.vault_root = PathBuf::from(val),
                "default_quota_bytes" => {
                    cfg.default_quota_bytes = val.parse().map_err(|_| {
                        LanVaultError::InvalidInput(format!("invalid default_quota_bytes: {val}"))
                    })?;
                }
                "web_password" => cfg.web_password = val.to_string(),
                "device_name" => cfg.device_name = val.to_string(),
                "keep_alive_level" => cfg.keep_alive_level = val.to_string(),
                _ => {
                    return Err(LanVaultError::InvalidInput(format!(
                        "unknown config key `{key}` on line {}",
                        line_no + 1
                    )));
                }
            }
        }
        Ok(cfg)
    }
}

/// Initializes SQLite WAL, default vault, mDNS responder, and serves the Axum HTTP/WS server
/// until a graceful restart signal or Ctrl-C is triggered.
pub async fn run_server(config: ServerConfig) -> Result<ServerExitReason> {
    let (_shutdown_tx, shutdown_rx) = oneshot::channel();
    serve_with_shutdown(config, shutdown_rx, None).await
}

/// Serves the Axum HTTP/WS server until `shutdown_rx`, graceful restart, or Ctrl-C fires.
///
/// If `bound_addr_tx` is provided, the real bound `SocketAddr` is sent immediately after
/// `TcpListener::bind` succeeds (eliminating port TOCTOU when binding port `0`).
pub async fn serve_with_shutdown(
    config: ServerConfig,
    shutdown_rx: oneshot::Receiver<()>,
    bound_addr_tx: Option<oneshot::Sender<SocketAddr>>,
) -> Result<ServerExitReason> {
    serve_with_shutdown_and_state(config, shutdown_rx, bound_addr_tx, None).await
}

/// Variant of `serve_with_shutdown` that also returns the live `AppState` to in-process hosts
/// (used by `lanvault-ffi` for direct interactive pairing approval without HTTP login round-trips).
pub async fn serve_with_shutdown_and_state(
    config: ServerConfig,
    shutdown_rx: oneshot::Receiver<()>,
    bound_addr_tx: Option<oneshot::Sender<SocketAddr>>,
    app_state_tx: Option<oneshot::Sender<Arc<AppState>>>,
) -> Result<ServerExitReason> {
    #[cfg(unix)]
    let vault_root_existed = config.vault_root.exists();
    std::fs::create_dir_all(&config.vault_root)?;
    #[cfg(unix)]
    if !vault_root_existed {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) = std::fs::set_permissions(
            &config.vault_root,
            std::fs::Permissions::from_mode(0o700),
        ) {
            if !matches!(
                err.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
            ) {
                return Err(LanVaultError::Io(err));
            }
        }
    }
    let vault_root = normalize_absolute_dir(&config.vault_root)?;
    lanvault_core::install_rust_panic_hook();
    lanvault_core::configure_debug_log_file(&vault_root.join("lanvault_debug.log"));
    let db_path = vault_root.join("index.db");
    let db = Arc::new(Database::open(&db_path)?);
    let vault_store = Arc::new(VaultStore::new(Arc::clone(&db)));

    // Ensure at least one default vault exists on first boot.
    if db.list_vaults()?.is_empty() {
        let metadata_path = vault_root.join("vault.meta.json");
        if metadata_path.exists() {
            let meta = VaultStore::read_vault_meta(&vault_root)?;
            let vault = VaultRecord {
                vault_id: meta.vault_id.clone(),
                name: meta.name.clone(),
                pool_mode: meta.pool_mode,
                quota_bytes: meta.quota_bytes,
                used_bytes: 0,
                e2ee_enabled: meta.e2ee_enabled,
                created_at: meta.created_at,
            };
            let volumes: Vec<VaultVolumeRecord> = meta
                .volumes
                .iter()
                .map(|volume| VaultVolumeRecord {
                    volume_id: volume.volume_id.clone(),
                    vault_id: meta.vault_id.clone(),
                    mount_path: volume.mount_path.clone(),
                    volume_guid: volume.volume_guid.clone(),
                    role: volume.role,
                    is_online: true,
                })
                .collect();
            db.insert_vault(&vault, &volumes)?;
        } else {
            vault_store.create_vault(
                "Default Vault".to_string(),
                vault_root.to_string_lossy().into_owned(),
                None,
                PoolMode::Single,
                config.default_quota_bytes,
                true,
                Vec::new(),
            )?;
        }
    }
    if db.get_setting(INDEX_RECOVERY_SETTING)?.as_deref() != Some("1") {
        if recover_index_from_volumes(&db, &vault_root)? {
            db.set_setting(INDEX_RECOVERY_SETTING, "1")?;
        } else {
            warn!("index recovery was incomplete and will be retried on next startup");
        }
    }

    let identity = Arc::new(DeviceIdentity::load_or_generate(
        &vault_root.join("device.key"),
    )?);
    let device_id = format!("lv-{}", &identity.fingerprint_hex()[..12]);
    let clipboard_key = identity.derive_local_storage_key("clipboard-group");
    let pairing_pin = generate_six_digit_pin();

    let (ws_tx, _) = broadcast::channel::<WsServerEvent>(256);
    let (alert_tx, mut alert_rx) = broadcast::channel::<BitRotAlertEvent>(64);

    // Forward Bit-Rot alerts into the unified WebSocket broadcast channel.
    let ws_tx_for_alerts = ws_tx.clone();
    let alert_task = tokio::spawn(async move {
        loop {
            match alert_rx.recv().await {
                Ok(alert) => {
                    let _ = ws_tx_for_alerts.send(WsServerEvent::BitRotAlert(alert));
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "bit-rot alert subscriber lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let scrubber = Arc::new(BitRotScrubber::new(
        Arc::clone(&db),
        Arc::clone(&vault_store),
        alert_tx,
    ));

    if config.web_password.len() < 8 {
        return Err(LanVaultError::InvalidInput(
            "web_password must be explicitly configured with at least 8 characters".to_string(),
        ));
    }
    let auth = Arc::new(if config.web_password.starts_with("$argon2") {
        AuthManager::from_password_hash(
            config.web_password.clone(),
            Arc::clone(&identity),
        )?
    } else {
        AuthManager::from_plaintext_password(
            &config.web_password,
            Arc::clone(&identity),
        )?
    });

    let bind_ip: IpAddr = config.bind_addr.parse().map_err(|_| {
        LanVaultError::InvalidInput(format!("invalid bind_addr: {}", config.bind_addr))
    })?;
    let tcp = TcpListener::bind(SocketAddr::from((bind_ip, config.port))).await?;
    let local_addr = tcp.local_addr()?;
    let lan_ip = detect_lan_ipv4();
    let listener = tls::LanListener::new(tcp, &identity, IpAddr::V4(lan_ip))?;
    if let Some(tx) = bound_addr_tx {
        let _ = tx.send(local_addr);
    }
    let lan_ip = lan_ip.to_string();

    let mdns_handle = match MdnsRegistrar::register(DiscoveryAnnouncement {
        id: device_id.clone(),
        os: current_os_name().to_string(),
        name: config.device_name.clone(),
        fp: identity.fingerprint_hex(),
        port: local_addr.port(),
    }) {
        Ok(reg) => Some(reg),
        Err(e) => {
            warn!("mDNS registration unavailable, continuing with HTTPS/UDP: {e}");
            None
        }
    };

    let restart_notify = Arc::new(Notify::new());
    let state = Arc::new(AppState {
        device_id,
        identity: Arc::clone(&identity),
        clipboard_key,
        os_name: current_os_name().to_string(),
        keep_alive_level: config.keep_alive_level,
        lan_ip,
        port: local_addr.port(),
        mdns_active: AtomicBool::new(mdns_handle.is_some()),
        started_at: Instant::now(),
        throughput_bps: AtomicU64::new(0),
        transfer_bytes: AtomicU64::new(0),
        throughput_sample: std::sync::Mutex::new((Instant::now(), 0)),
        active_chunk_writes: AtomicU64::new(0),
        upload_semaphore: tokio::sync::Semaphore::new(32),
        accepting_tasks: AtomicBool::new(true),
        pairing_pin: std::sync::Mutex::new(pairing_pin),
        pending_pair_lock: std::sync::Mutex::new(()),
        pending_pair_requests: DashMap::new(),
        renew_challenges: DashMap::new(),
        renew_rate_limits: DashMap::new(),
        transfer_budget_lock: std::sync::Mutex::new(()),
        transfer_locks: std::array::from_fn(|_| Arc::new(tokio::sync::RwLock::new(()))),
        online_peers: DashMap::new(),
        peer_leases: DashMap::new(),
        db: Arc::clone(&db),
        vault_store: Arc::clone(&vault_store),
        scrubber: Arc::clone(&scrubber),
        auth,
        ws_tx,
        restart_notify: Arc::clone(&restart_notify),
        telemetry_cache: std::sync::RwLock::new(None),
    });

    if let Some(tx) = app_state_tx {
        let _ = tx.send(Arc::clone(&state));
    }

    let gc_db = Arc::clone(&db);
    let gc_store = Arc::clone(&vault_store);
    let gc_state = Arc::clone(&state);
    let gc_task = tokio::spawn(async move {
        let interval_dur = Duration::from_secs(GC_CHECK_INTERVAL_SECS);
        let mut ticker = tokio::time::interval(interval_dur);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let db_clone = Arc::clone(&gc_db);
            let store_clone = Arc::clone(&gc_store);
            let state_clone = Arc::clone(&gc_state);
            let run_res = tokio::task::spawn_blocking(move || -> Result<()> {
                let now = unix_timestamp_secs();
                if let Err(error) = state_clone.reclaim_expired_transfers(now) {
                    warn!(%error, "transfer cleanup failed");
                }
                let last_run = db_clone
                    .get_setting(GC_LAST_RUN_SETTING)?
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(0);
                if now.saturating_sub(last_run) < GC_INTERVAL_SECS as i64 {
                    return Ok(());
                }
                let collector = GarbageCollector::new(Arc::clone(&db_clone), store_clone);
                for vault in db_clone.list_vaults()? {
                    match collector.run_gc(&vault.vault_id, now, GfsPolicy::default()) {
                        Ok(report) => info!(
                            vault_id = %vault.vault_id,
                            expired_snapshots = report.expired_snapshots,
                            deleted_chunks = report.deleted_chunks,
                            reclaimed_bytes = report.reclaimed_bytes,
                            "periodic GFS garbage collection completed"
                        ),
                        Err(err) => warn!(
                            vault_id = %vault.vault_id,
                            "periodic GFS garbage collection failed: {err}"
                        ),
                    }
                }
                db_clone.set_setting(GC_LAST_RUN_SETTING, &now.to_string())?;
                Ok(())
            })
            .await;
            if let Err(err) = run_res {
                warn!("periodic GC task join error: {err}");
            }
        }
    });

    let scrub_db = Arc::clone(&db);
    let scrubber_task = Arc::clone(&scrubber);
    let scrub_task = tokio::spawn(async move {
        let interval_dur = Duration::from_secs(SCRUB_CHECK_INTERVAL_SECS);
        let mut ticker = tokio::time::interval(interval_dur);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let db_clone = Arc::clone(&scrub_db);
            let scrubber_clone = Arc::clone(&scrubber_task);
            let result = tokio::task::spawn_blocking(move || -> Result<()> {
                let now = unix_timestamp_secs();
                let last_run = db_clone
                    .get_setting(SCRUB_LAST_RUN_SETTING)?
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(0);
                if now.saturating_sub(last_run) < SCRUB_INTERVAL_SECS as i64 {
                    return Ok(());
                }
                for vault in db_clone.list_vaults()? {
                    match scrubber_clone.run_scrub(&vault.vault_id) {
                        Ok(report) => info!(
                            vault_id = %vault.vault_id,
                            checked = report.checked_chunks,
                            corrupted = report.corrupted_chunks,
                            healed = report.healed_chunks,
                            "periodic bit-rot scrub completed"
                        ),
                        Err(err) => warn!(
                            vault_id = %vault.vault_id,
                            "periodic bit-rot scrub failed: {err}"
                        ),
                    }
                }
                db_clone.set_setting(SCRUB_LAST_RUN_SETTING, &now.to_string())?;
                Ok(())
            })
            .await;
            if let Err(err) = result {
                warn!("periodic scrub task join error: {err}");
            }
        }
    });

    let app = build_router(Arc::clone(&state));
    info!(addr = %local_addr, "EveryoneNAS server listening");

    let graceful_restart = Arc::new(AtomicBool::new(false));
    let graceful_restart_flag = Arc::clone(&graceful_restart);
    let state_for_shutdown = Arc::clone(&state);
    let terminate_signal = async {
        #[cfg(unix)]
        {
            if let Ok(mut signal) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                signal.recv().await;
            }
        }
        #[cfg(not(unix))]
        {
            std::future::pending::<()>().await;
        }
    };
    let shutdown_signal = async move {
        tokio::select! {
            _ = restart_notify.notified() => {
                graceful_restart_flag.store(true, Ordering::SeqCst);
                info!("graceful restart signal received; stopping Axum listener");
            }
            _ = shutdown_rx => {
                info!("external shutdown signal received; draining active tasks and checkpointing WAL");
                Arc::clone(&state_for_shutdown).execute_graceful_restart().await;
            }
            _ = terminate_signal => {
                info!("SIGTERM received; draining active tasks and checkpointing WAL");
                Arc::clone(&state_for_shutdown).execute_graceful_restart().await;
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received; draining active tasks and checkpointing WAL");
                Arc::clone(&state_for_shutdown).execute_graceful_restart().await;
            }
        }
    };

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<tls::LanConnectionInfo>(),
    )
    .with_graceful_shutdown(shutdown_signal)
    .await?;

    gc_task.abort();
    scrub_task.abort();
    alert_task.abort();

    if let Some(reg) = mdns_handle {
        let _ = reg.shutdown();
    }

    if graceful_restart.load(Ordering::SeqCst) {
        Ok(ServerExitReason::GracefulRestart)
    } else {
        Ok(ServerExitReason::Stopped)
    }
}

#[derive(Debug, Deserialize)]
struct RecoveredManifest {
    snapshot_id: String,
    vault_id: String,
    #[serde(default)]
    rule_id: String,
    device_id: String,
    root_tree_hash_hex: String,
    created_at: i64,
    files: Vec<RecoveredManifestFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct RecoveredManifestFile {
    rel_path: String,
    size: u64,
    #[serde(default)]
    mtime_ns: i64,
    #[serde(default)]
    chunk_hashes_hex: Vec<String>,
}

fn recover_index_from_volumes(db: &Database, vault_root: &Path) -> Result<bool> {
    let mut processed_manifests = HashSet::new();
    let mut complete = true;
    for vault in db.list_vaults()? {
        let mut online_volumes = 0usize;
        let mut offline_volumes = 0usize;
        for volume in db.list_vault_volumes(&vault.vault_id)? {
            if !volume.is_online {
                offline_volumes += 1;
                if vault.pool_mode != PoolMode::MirrorRaid1 {
                    complete = false;
                }
                continue;
            }
            online_volumes += 1;
            let root = PathBuf::from(&volume.mount_path);
            let chunks_root = root.join("chunks");
            if let Ok(buckets) = std::fs::read_dir(&chunks_root) {
                for bucket in buckets.flatten() {
                    let Ok(entries) = std::fs::read_dir(bucket.path()) else {
                        continue;
                    };
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if !path.is_file() {
                            continue;
                        }
                        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                            continue;
                        };
                        let Ok(hash) = hex::decode(name) else {
                            continue;
                        };
                        let Ok(hash) = <Vec<u8> as TryInto<[u8; 32]>>::try_into(hash) else {
                            continue;
                        };
                        let stored_size = entry.metadata()?.len();
                        db.insert_chunk_if_missing(
                            &vault.vault_id,
                            &hash,
                            0,
                            stored_size,
                        )?;
                    }
                }
            } else {
                complete = false;
            }

            let manifests_root = root.join("manifests");
            let manifests = match std::fs::read_dir(&manifests_root) {
                Ok(entries) => entries,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            for entry in manifests.flatten() {
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("json")
                    || !processed_manifests.insert(path.clone())
                {
                    continue;
                }
                let manifest: RecoveredManifest = match serde_json::from_slice(
                    &std::fs::read(&path)?,
                ) {
                    Ok(manifest) => manifest,
                    Err(err) => {
                        warn!(path = %path.display(), "skipping invalid recovery manifest: {err}");
                        complete = false;
                        continue;
                    }
                };
                let root_hash: [u8; 32] = match hex::decode(&manifest.root_tree_hash_hex)
                    .ok()
                    .and_then(|bytes| bytes.try_into().ok())
                {
                    Some(hash) => hash,
                    None => {
                        warn!(
                            path = %path.display(),
                            "skipping manifest with invalid root hash"
                        );
                        complete = false;
                        continue;
                    }
                };
                let mut hashes = Vec::new();
                let mut canonical = Vec::new();
                for hash_hex in manifest
                    .files
                    .iter()
                    .flat_map(|file| file.chunk_hashes_hex.iter())
                {
                    let Ok(hash) = hex::decode(hash_hex) else {
                        continue;
                    };
                    let Ok(hash) = <Vec<u8> as TryInto<[u8; 32]>>::try_into(hash) else {
                        continue;
                    };
                    hashes.push(hash);
                }
                let mut files = manifest.files.clone();
                files.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));
                for file in &files {
                    canonical.extend_from_slice(file.rel_path.as_bytes());
                    canonical.push(0);
                    canonical.extend_from_slice(&file.size.to_le_bytes());
                    canonical.extend_from_slice(&file.mtime_ns.to_le_bytes());
                    for hash_hex in &file.chunk_hashes_hex {
                        let hash: Option<[u8; 32]> = hex::decode(hash_hex)
                            .ok()
                            .and_then(|bytes| bytes.try_into().ok());
                        if let Some(hash) = hash {
                            canonical.extend_from_slice(&hash);
                        }
                    }
                }
                if blake3_hash(&canonical) != root_hash {
                    warn!(
                        path = %path.display(),
                        "skipping manifest whose root hash does not match its contents"
                    );
                    complete = false;
                    continue;
                }
                hashes.sort_unstable();
                hashes.dedup();
                if hashes.iter().any(|hash| {
                    db.get_chunk(&manifest.vault_id, hash)
                        .map(|chunk| chunk.is_none())
                        .unwrap_or(true)
                }) {
                    warn!(
                        path = %path.display(),
                        "skipping manifest with missing chunks"
                    );
                    complete = false;
                    continue;
                }
                if db.get_trusted_peer(&manifest.device_id)?.is_none() {
                    db.upsert_trusted_peer(&TrustedPeer {
                        device_id: manifest.device_id.clone(),
                        device_name: "recovered-device".to_string(),
                        os_type: "unknown".to_string(),
                        ed25519_pubkey: Vec::new(),
                        last_ip: None,
                        last_port: None,
                        auto_connect: false,
                        paired_at: manifest.created_at,
                        revoked_at: Some(manifest.created_at),
                    })?;
                }
                let record = SnapshotRecord {
                    snapshot_id: manifest.snapshot_id,
                    vault_id: manifest.vault_id,
                    rule_id: if manifest.rule_id.is_empty() {
                        "legacy".to_string()
                    } else {
                        manifest.rule_id
                    },
                    device_id: manifest.device_id,
                    root_tree_hash: root_hash,
                    retention_tier: RetentionTier::Daily,
                    file_count: manifest.files.len() as u64,
                    total_bytes: manifest
                        .files
                        .iter()
                        .fold(0u64, |total, file| total.saturating_add(file.size)),
                    created_at: manifest.created_at,
                };
                db.insert_snapshot_with_chunks(&record, &hashes)?;
            }
        }
        if vault.pool_mode == PoolMode::MirrorRaid1 && offline_volumes > 0 {
            if online_volumes == 0 {
                complete = false;
            } else {
                warn!(
                    vault_id = %vault.vault_id,
                    offline_volumes,
                    "mirror_raid1 recovery completed with degraded redundancy (offline mirror volume)"
                );
            }
        }
        let actual_bytes = db
            .list_all_chunks(&vault.vault_id)?
            .iter()
            .fold(0u64, |total, chunk| total.saturating_add(chunk.stored_size));
        db.set_vault_used_bytes(&vault.vault_id, actual_bytes)?;
    }
    let _ = vault_root;
    Ok(complete)
}

fn normalize_absolute_dir(path: &Path) -> Result<PathBuf> {
    let resolved = match std::fs::canonicalize(path) {
        Ok(canonical) => canonical,
        Err(_) if !path.is_absolute() => std::env::current_dir()?.join(path),
        Err(_) => path.to_path_buf(),
    };
    #[cfg(windows)]
    {
        let raw = resolved.to_string_lossy();
        if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{unc}")));
        }
        if let Some(stripped) = raw.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(stripped));
        }
    }
    Ok(resolved)
}

pub(crate) fn generate_six_digit_pin() -> String {
    let mut rng = rand::thread_rng();
    let num: u32 = rng.gen_range(0..1_000_000);
    format!("{num:06}")
}

fn current_os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "android") {
        "android"
    } else {
        "linux"
    }
}

fn default_keep_alive_level() -> &'static str {
    if cfg!(target_os = "windows") {
        "WIN_SERVICE"
    } else if cfg!(target_os = "android") {
        "LEVEL_0_FOREGROUND"
    } else {
        "LEVEL_3_MAGISK_DAEMON"
    }
}

fn detect_lan_ipv4() -> Ipv4Addr {
    if let Ok(sock) = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        if sock.connect((Ipv4Addr::new(192, 168, 1, 1), 80)).is_ok() {
            if let Ok(SocketAddr::V4(v4)) = sock.local_addr() {
                return *v4.ip();
            }
        }
    }
    Ipv4Addr::LOCALHOST
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanvault_core::VolumeRole;

    #[test]
    fn mirror_recovery_is_complete_when_online_mirror_has_all_data() {
        let dir = std::env::temp_dir().join(format!(
            "lanvault_mirror_recovery_test_{}_{}",
            std::process::id(),
            unix_timestamp_secs()
        ));
        let online_root = dir.join("disk_a");
        let offline_root = dir.join("disk_b_missing");
        std::fs::create_dir_all(online_root.join("chunks").join("00")).unwrap();
        std::fs::create_dir_all(online_root.join("manifests")).unwrap();

        let chunk_data = b"hello mirror chunk";
        let chunk_hash = blake3_hash(chunk_data);
        let chunk_hex = hex::encode(chunk_hash);
        std::fs::write(
            online_root.join("chunks").join(&chunk_hex[..2]).join(&chunk_hex),
            chunk_data,
        )
        .or_else(|_| {
            let bucket = online_root.join("chunks").join(&chunk_hex[..2]);
            std::fs::create_dir_all(&bucket)?;
            std::fs::write(bucket.join(&chunk_hex), chunk_data)
        })
        .unwrap();

        let rel_path = "doc.txt";
        let mut canonical = Vec::new();
        canonical.extend_from_slice(rel_path.as_bytes());
        canonical.push(0);
        canonical.extend_from_slice(&(chunk_data.len() as u64).to_le_bytes());
        canonical.extend_from_slice(&0i64.to_le_bytes());
        canonical.extend_from_slice(&chunk_hash);
        let root_hash_hex = hex::encode(blake3_hash(&canonical));

        let manifest_json = serde_json::json!({
            "snapshot_id": "snap-mirror-1",
            "vault_id": "v-mirror",
            "rule_id": "rule-1",
            "device_id": "lv-dev1",
            "root_tree_hash_hex": root_hash_hex,
            "created_at": 1_700_000_000i64,
            "files": [{
                "rel_path": rel_path,
                "size": chunk_data.len(),
                "mtime_ns": 0,
                "chunk_hashes_hex": [chunk_hex],
            }]
        });
        std::fs::write(
            online_root.join("manifests").join("snap-mirror-1.json"),
            serde_json::to_vec(&manifest_json).unwrap(),
        )
        .unwrap();

        let db = Database::open(&dir.join("index.db")).unwrap();
        db.insert_vault(
            &VaultRecord {
                vault_id: "v-mirror".to_string(),
                name: "Mirror".to_string(),
                pool_mode: PoolMode::MirrorRaid1,
                quota_bytes: 1024 * 1024,
                used_bytes: 0,
                e2ee_enabled: false,
                created_at: 1_700_000_000,
            },
            &[
                VaultVolumeRecord {
                    volume_id: "vol-1".to_string(),
                    vault_id: "v-mirror".to_string(),
                    mount_path: online_root.to_string_lossy().into_owned(),
                    volume_guid: None,
                    role: VolumeRole::Primary,
                    is_online: true,
                },
                VaultVolumeRecord {
                    volume_id: "vol-2".to_string(),
                    vault_id: "v-mirror".to_string(),
                    mount_path: offline_root.to_string_lossy().into_owned(),
                    volume_guid: None,
                    role: VolumeRole::Mirror,
                    is_online: false,
                },
            ],
        )
        .unwrap();

        let complete = recover_index_from_volumes(&db, &dir).unwrap();
        assert!(complete);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
