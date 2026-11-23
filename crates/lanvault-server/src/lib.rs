//! `lanvault-server`: content-addressed multi-volume vault engine, Bit-Rot scrubber,
//! GFS snapshot GC, Argon2id + IP-lockout authentication, and Axum 0.8 Web/WS server.

pub mod auth;
pub mod gc;
pub mod scrub;
pub mod vault;
pub mod web;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::{DashMap, DashSet};
use lanvault_core::{
    Database, DeviceIdentity, DiscoveryAnnouncement, LanVaultError, MdnsRegistrar, PoolMode,
    Result,
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

/// Reason why the LanVault HTTP/WS server exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerExitReason {
    Stopped,
    GracefulRestart,
}

/// Server startup configuration (can be loaded from JSON or `key = value` TOML via `--config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
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
            bind_addr: "0.0.0.0".to_string(),
            port: 7842,
            vault_root: PathBuf::from("./everyonenas_data"),
            default_quota_bytes: 100 * 1024 * 1024 * 1024,
            web_password: "everyonenas-change-me".to_string(),
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
                _ => {}
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
    std::fs::create_dir_all(&config.vault_root)?;
    lanvault_core::install_rust_panic_hook();
    lanvault_core::configure_debug_log_file(&config.vault_root.join("lanvault_debug.log"));
    let db_path = config.vault_root.join("index.db");
    let db = Arc::new(Database::open(&db_path)?);
    let vault_store = Arc::new(VaultStore::new(Arc::clone(&db)));

    // Ensure at least one default vault exists on first boot.
    if db.list_vaults()?.is_empty() {
        vault_store.create_vault(
            "Default Vault".to_string(),
            config.vault_root.to_string_lossy().into_owned(),
            None,
            PoolMode::Single,
            config.default_quota_bytes,
            true,
            Vec::new(),
        )?;
    }

    let identity = DeviceIdentity::load_or_generate(&config.vault_root.join("device.key"))?;
    let device_id = format!("lv-{}", &identity.fingerprint_hex()[..12]);
    let pairing_pin = generate_six_digit_pin();

    let (ws_tx, _) = broadcast::channel::<WsServerEvent>(256);
    let (alert_tx, mut alert_rx) = broadcast::channel::<BitRotAlertEvent>(64);

    // Forward Bit-Rot alerts into the unified WebSocket broadcast channel.
    let ws_tx_for_alerts = ws_tx.clone();
    let alert_task = tokio::spawn(async move {
        while let Ok(alert) = alert_rx.recv().await {
            let _ = ws_tx_for_alerts.send(WsServerEvent::BitRotAlert(alert));
        }
    });

    // Background periodic GFS snapshot retention & orphan chunk GC task.
    let gc_db = Arc::clone(&db);
    let gc_store = Arc::clone(&vault_store);
    let gc_task = tokio::spawn(async move {
        let interval_dur = Duration::from_secs(GC_INTERVAL_SECS);
        let mut ticker =
            tokio::time::interval_at(tokio::time::Instant::now() + interval_dur, interval_dur);
        loop {
            ticker.tick().await;
            let db_clone = Arc::clone(&gc_db);
            let store_clone = Arc::clone(&gc_store);
            let run_res = tokio::task::spawn_blocking(move || -> Result<()> {
                let collector = GarbageCollector::new(Arc::clone(&db_clone), store_clone);
                let now = unix_timestamp_secs();
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
                Ok(())
            })
            .await;
            if let Err(err) = run_res {
                warn!("periodic GC task join error: {err}");
            }
        }
    });

    let scrubber = Arc::new(BitRotScrubber::new(
        Arc::clone(&db),
        Arc::clone(&vault_store),
        alert_tx,
    ));

    let auth = Arc::new(if config.web_password.starts_with("$argon2") {
        AuthManager::from_password_hash(config.web_password.clone())?
    } else {
        AuthManager::from_plaintext_password(&config.web_password)?
    });

    let bind_ip: IpAddr = config.bind_addr.parse().map_err(|_| {
        LanVaultError::InvalidInput(format!("invalid bind_addr: {}", config.bind_addr))
    })?;
    let listener = TcpListener::bind(SocketAddr::from((bind_ip, config.port))).await?;
    let local_addr = listener.local_addr()?;
    if let Some(tx) = bound_addr_tx {
        let _ = tx.send(local_addr);
    }
    let lan_ip = detect_lan_ipv4().to_string();

    let mdns_handle = match MdnsRegistrar::register(DiscoveryAnnouncement {
        id: device_id.clone(),
        os: current_os_name().to_string(),
        name: config.device_name.clone(),
        fp: identity.fingerprint_hex(),
        port: local_addr.port(),
    }) {
        Ok(reg) => Some(reg),
        Err(e) => {
            warn!("mDNS registration unavailable, continuing with HTTP/UDP: {e}");
            None
        }
    };

    let restart_notify = Arc::new(Notify::new());
    let state = Arc::new(AppState {
        device_id,
        os_name: current_os_name().to_string(),
        keep_alive_level: config.keep_alive_level,
        lan_ip,
        port: local_addr.port(),
        mdns_active: AtomicBool::new(mdns_handle.is_some()),
        started_at: Instant::now(),
        throughput_bps: AtomicU64::new(0),
        active_chunk_writes: AtomicU64::new(0),
        accepting_tasks: AtomicBool::new(true),
        pairing_pin,
        online_peers: DashSet::new(),
        peer_session_keys: DashMap::new(),
        db: Arc::clone(&db),
        vault_store,
        scrubber,
        auth,
        ws_tx,
        restart_notify: Arc::clone(&restart_notify),
    });

    let app = build_router(Arc::clone(&state));
    info!(addr = %local_addr, "EveryoneNAS server listening");

    let graceful_restart = Arc::new(AtomicBool::new(false));
    let graceful_restart_flag = Arc::clone(&graceful_restart);
    let state_for_shutdown = Arc::clone(&state);
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
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received; draining active tasks and checkpointing WAL");
                Arc::clone(&state_for_shutdown).execute_graceful_restart().await;
            }
        }
    };

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal)
    .await?;

    gc_task.abort();
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

fn generate_six_digit_pin() -> String {
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
