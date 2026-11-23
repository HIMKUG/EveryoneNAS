//! `flutter_rust_bridge` v2 FFI 暴露层，严格对齐 `ARCHITECTURE_CONTRACTS.md` 第 4 节全部函数及 DTO，
//! 并扩展统一三态运行模式（`client_only` / `server_only` / `dual`）控制接口。
//!
//! 生命周期与状态治理：
//! - 单一 `tokio::sync::Mutex<LifecycleState>` 串行化模式切换、启服与停服，彻底消除 TOCTOU 竞态。
//! - 独立维护用户数据目录 `client_state.db` (`Arc<lanvault_core::Database>`)，与本地服务端完全解耦，
//!   保障 `client_only` 模式下 `trigger_incremental_scan` 与 `update_power_network_policy` 独立可用。
//! - `RoleMode`、`DeviceIdentity` 与 `vault_master_key` 全部持久化，重启不漂移。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use argon2::Argon2;
use rand::{rngs::OsRng, RngCore};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, watch, Mutex as AsyncMutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use lanvault_client::{
    reconcile_directory, ClipboardClient, DirectoryWatcher, PowerNetworkPolicy,
    DeviceEnvironment, ReconnectController, ReconnectEvent, ReconnectTarget, SyncEngine,
    TokenBucket, WatchEvent,
};
use lanvault_core::{
    unpack_chunk, CHUNK_MAX_SIZE,
    blake3_hash, decrypt_aead, encrypt_aead, pairing_identity_message, pairing_response_message,
    parse_role_mode, verify_ed25519, Database, DeviceIdentity, DiscoveredPeerInfo,
    DiscoveryBrowser, PoolMode, RoleMode, SessionKeyExchange, SymmetricKey, SyncRuleRecord,
    TransferHistoryRecord, TrustedPeer, VolumeRole,
};
use lanvault_server::scrub::{BitRotAlertEvent, BitRotScrubber};
use lanvault_server::vault::{ExtraVolumeSpec, VaultStore};
use lanvault_server::{ServerConfig, ServerExitReason};

use crate::frb_generated::StreamSink;

/// 默认服务端监听端口（对齐 `ARCHITECTURE_CONTRACTS.md` §3.1）。
const DEFAULT_SERVER_PORT: u16 = 7842;

/// 优雅重启最大排空等待时间（3 秒，对齐契约 §3.1 路由 4）。
const GRACEFUL_DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// 服务端启动绑定超时时间。
const SERVER_BIND_TIMEOUT: Duration = Duration::from_secs(10);

const SETTING_ROLE_MODE: &str = "role_mode";
const SETTING_VAULT_MASTER_KEY_HEX: &str = "vault_master_key_hex";
const SETTING_POWER_POLICY_JSON: &str = "power_network_policy";
const SETTING_ACTIVE_PEER_JSON: &str = "active_peer_session_json";
const SETTING_AUTO_RECONNECT: &str = "auto_reconnect_enabled";
const SETTING_DEFAULTS_INITIALIZED: &str = "default_sync_rules_initialized";
const SETTING_DEBUG_MODE: &str = "debug_mode";
const SETTING_BACKUP_STORAGE_JSON: &str = "backup_storage_config_json";
const SETTING_LAST_SERVER_CONFIG_JSON: &str = "last_server_config_json";
const SETTING_SYNC_PAUSED: &str = "sync_paused";
const SETTING_BIT_ROT_AUTO_SCRUB: &str = "bit_rot_auto_scrub";
const SETTING_RULE_SCHEDULE_PREFIX: &str = "rule_schedule:";

/// FFI DTO: mDNS / 局域网发现的对端设备。
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredPeer {
    pub device_id: String,
    pub device_name: String,
    pub os_type: String,
    pub ip: String,
    pub port: u16,
    pub fingerprint: String,
    #[serde(skip)]
    last_seen: Instant,
}

impl From<DiscoveredPeerInfo> for DiscoveredPeer {
    fn from(info: DiscoveredPeerInfo) -> Self {
        Self {
            device_id: info.device_id,
            device_name: info.device_name,
            os_type: info.os_type,
            ip: info.ip.to_string(),
            port: info.port,
            fingerprint: info.fingerprint,
            last_seen: Instant::now(),
        }
    }
}

/// FFI DTO: 增量同步进度流式事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncProgressEvent {
    pub rule_id: String,
    pub current_file: String,
    pub bytes_processed: u64,
    pub total_bytes: u64,
    pub chunks_uploaded: u64,
    pub chunks_deduplicated: u64,
    pub throughput_bps: u64,
    pub status: String,
    pub bit_rot_health: String,
    pub corrupted_chunks: u64,
}

impl Default for SyncProgressEvent {
    fn default() -> Self {
        Self {
            rule_id: String::new(),
            current_file: String::new(),
            bytes_processed: 0,
            total_bytes: 0,
            chunks_uploaded: 0,
            chunks_deduplicated: 0,
            throughput_bps: 0,
            status: "idle".to_string(),
            bit_rot_health: "unknown".to_string(),
            corrupted_chunks: 0,
        }
    }
}

struct LocalServerHandle {
    port: u16,
    vault_path: PathBuf,
    db_path: PathBuf,
    quota_bytes: u64,
    password_hash: [u8; 32],
    stop_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

/// 受单一异步互斥锁保护的三态模式与本地服务生命周期状态，杜绝并发竞态。
struct LifecycleState {
    role_mode: RoleMode,
    last_server_config: Option<ServerConfig>,
    server_handle: Option<LocalServerHandle>,
    local_admin_cookie: Option<String>,
    watchers: Vec<DirectoryWatcher>,
    watcher_dispatch_task: Option<JoinHandle<()>>,
    reconnect_task: Option<JoinHandle<()>>,
    reconnect_event_task: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRuleDto {
    pub rule_id: String,
    pub local_dir: String,
    pub target_vault_id: String,
    pub enabled: bool,
    pub schedule_mode: String,
}

impl From<SyncRuleRecord> for SyncRuleDto {
    fn from(r: SyncRuleRecord) -> Self {
        Self {
            rule_id: r.rule_id,
            local_dir: r.local_dir,
            target_vault_id: r.target_vault_id,
            enabled: r.enabled,
            schedule_mode: "realtime".to_string(),
        }
    }
}

#[derive(Clone)]
struct ActivePeerSession {
    device_id: String,
    addr: SocketAddr,
    endpoint_base_url: String,
    session_cookie: Option<String>,
    channel_key: SymmetricKey,
    default_vault_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct PersistedPeerSession {
    device_id: String,
    addr: String,
    endpoint_base_url: String,
    session_cookie: Option<String>,
    channel_key_hex: String,
    default_vault_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct EncryptedSetting {
    ciphertext_hex: String,
}

#[derive(Serialize, Deserialize)]
struct VaultKeyBackup {
    version: u8,
    salt_hex: String,
    ciphertext_hex: String,
}

/// 全局 FFI 异步运行时与服务句柄容器。
struct FfiRuntimeState {
    runtime: tokio::runtime::Runtime,
    client_db: Arc<Database>,
    client_identity: DeviceIdentity,
    client_device_id: String,
    vault_master_key: RwLock<SymmetricKey>,
    lifecycle: AsyncMutex<LifecycleState>,
    reconnect: ReconnectController,
    policy: Arc<RwLock<PowerNetworkPolicy>>,
    rate_limiter: Arc<TokenBucket>,
    progress_tx: broadcast::Sender<SyncProgressEvent>,
    latest_progress: Arc<Mutex<SyncProgressEvent>>,
    discovered_peers: Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    mdns_browser_started: Mutex<bool>,
    active_peer: Arc<RwLock<Option<ActivePeerSession>>>,
    environment: Arc<RwLock<DeviceEnvironment>>,
    sync_inflight: Arc<AsyncMutex<HashSet<String>>>,
}

static GLOBAL_STATE: OnceLock<Arc<FfiRuntimeState>> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());
static DAEMON_MODE: AtomicBool = AtomicBool::new(false);

fn client_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LANVAULT_DATA_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    #[cfg(target_os = "android")]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = fs::metadata("/proc/self")
            .map(|metadata| metadata.uid())
            .unwrap_or(20_000);
        return match uid {
            0 => PathBuf::from("/data/adb/lanvault"),
            2000 => PathBuf::from("/data/local/tmp/lanvault"),
            _ => std::env::var_os("TMPDIR")
                .and_then(|tmp| PathBuf::from(tmp).parent().map(Path::to_path_buf))
                .map(|files| files.join("lanvault"))
                .unwrap_or_else(|| PathBuf::from("/data/user/0/com.lanvault.app/files/lanvault")),
        };
    }
    #[cfg(not(target_os = "android"))]
    {
        if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
            let trimmed = appdata.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("EveryoneNAS");
            }
        }
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            let trimmed = xdg.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("lanvault");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            let trimmed = home.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed)
                    .join(".local")
                    .join("share")
                    .join("lanvault");
            }
        }
        std::env::temp_dir().join("lanvault")
    }
}

fn encrypt_setting(value: &[u8], protection_key: &SymmetricKey, aad: &[u8]) -> Result<String> {
    let encrypted = encrypt_aead(protection_key, value, aad)?;
    serde_json::to_string(&EncryptedSetting {
        ciphertext_hex: hex::encode(encrypted),
    })
    .map_err(Into::into)
}

fn decrypt_setting(
    raw: &str,
    protection_key: &SymmetricKey,
    aad: &[u8],
) -> Result<Vec<u8>> {
    let envelope: EncryptedSetting =
        serde_json::from_str(raw).context("invalid encrypted local setting")?;
    let ciphertext =
        hex::decode(envelope.ciphertext_hex).context("invalid encrypted setting hex")?;
    Ok(decrypt_aead(protection_key, &ciphertext, aad)?)
}

fn load_or_generate_vault_master_key(
    db: &Database,
    protection_key: &SymmetricKey,
) -> Result<SymmetricKey> {
    const AAD: &[u8] = b"lanvault.local.vault-master-key.v1";
    if let Some(raw) = db.get_setting(SETTING_VAULT_MASTER_KEY_HEX)? {
        let bytes = if raw.trim_start().starts_with('{') {
            decrypt_setting(&raw, protection_key, AAD)?
        } else {
            let legacy = hex::decode(raw.trim())
                .context("persisted vault_master_key_hex is not valid hex")?;
            db.set_setting(
                SETTING_VAULT_MASTER_KEY_HEX,
                &encrypt_setting(&legacy, protection_key, AAD)?,
            )?;
            legacy
        };
        return SymmetricKey::from_slice(&bytes)
            .context("persisted vault master key has invalid length");
    }

    let key = SymmetricKey::generate();
    db.set_setting(
        SETTING_VAULT_MASTER_KEY_HEX,
        &encrypt_setting(key.as_bytes(), protection_key, AAD)?,
    )?;
    Ok(key)
}

fn load_persisted_role_mode(db: &Database) -> Result<RoleMode> {
    match db.get_setting(SETTING_ROLE_MODE)? {
        Some(raw) => parse_role_mode(&raw).map_err(Into::into),
        None => Ok(RoleMode::Dual),
    }
}

fn load_persisted_policy(db: &Database) -> Result<PowerNetworkPolicy> {
    match db.get_setting(SETTING_POWER_POLICY_JSON)? {
        Some(raw) => serde_json::from_str(&raw).context("invalid persisted power_network_policy"),
        None => Ok(PowerNetworkPolicy::default()),
    }
}

fn default_sync_dirs() -> Vec<(&'static str, PathBuf)> {
    #[cfg(target_os = "android")]
    {
        vec![
            ("rule-photos", PathBuf::from("/storage/emulated/0/DCIM")),
            ("rule-docs", PathBuf::from("/storage/emulated/0/Documents")),
        ]
    }
    #[cfg(target_os = "windows")]
    {
        let home = std::env::var("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(r"C:\Users\Public"));
        vec![
            ("rule-photos", home.join("Pictures")),
            ("rule-docs", home.join("Documents")),
        ]
    }
    #[cfg(not(any(target_os = "android", target_os = "windows")))]
    {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        vec![
            ("rule-photos", home.join("Pictures")),
            ("rule-docs", home.join("Documents")),
        ]
    }
}

fn ensure_default_sync_rules(db: &Database, policy: &PowerNetworkPolicy) -> Result<()> {
    if db.get_setting(SETTING_DEFAULTS_INITIALIZED)?.as_deref() == Some("1") {
        return Ok(());
    }
    let ssids_json = if policy.allowed_ssids.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&policy.allowed_ssids)?)
    };
    for (rule_id, dir) in default_sync_dirs() {
        db.upsert_sync_rule(&SyncRuleRecord {
            rule_id: rule_id.to_string(),
            local_dir: dir.to_string_lossy().into_owned(),
            target_vault_id: "default".to_string(),
            wifi_only: policy.wifi_only,
            allowed_ssids: ssids_json.clone(),
            require_charging: policy.require_charging,
            max_kbps: policy.max_kbps,
            enabled: true,
        })?;
    }
    db.set_setting(SETTING_DEFAULTS_INITIALIZED, "1")?;
    Ok(())
}

fn load_persisted_peer(
    db: &Database,
    protection_key: &SymmetricKey,
) -> Result<Option<ActivePeerSession>> {
    const AAD: &[u8] = b"lanvault.local.active-peer.v1";
    let Some(raw) = db.get_setting(SETTING_ACTIVE_PEER_JSON)? else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let decoded = if raw.trim_start().starts_with('{') {
        decrypt_setting(&raw, protection_key, AAD)?
    } else {
        let legacy = raw.into_bytes();
        db.set_setting(
            SETTING_ACTIVE_PEER_JSON,
            &encrypt_setting(&legacy, protection_key, AAD)?,
        )?;
        legacy
    };
    let persisted: PersistedPeerSession =
        serde_json::from_slice(&decoded).context("invalid persisted active_peer_session_json")?;
    let addr: SocketAddr = persisted
        .addr
        .parse()
        .context("invalid persisted peer SocketAddr")?;
    let key_bytes = hex::decode(&persisted.channel_key_hex)
        .context("invalid persisted channel_key_hex")?;
    let key_arr: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| anyhow!("persisted channel_key must be 32 bytes"))?;
    Ok(Some(ActivePeerSession {
        device_id: persisted.device_id,
        addr,
        endpoint_base_url: persisted.endpoint_base_url,
        session_cookie: persisted.session_cookie,
        channel_key: SymmetricKey::from_bytes(key_arr),
        default_vault_id: persisted.default_vault_id,
    }))
}

fn save_persisted_peer(
    db: &Database,
    peer: &ActivePeerSession,
    protection_key: &SymmetricKey,
) -> Result<()> {
    const AAD: &[u8] = b"lanvault.local.active-peer.v1";
    let persisted = PersistedPeerSession {
        device_id: peer.device_id.clone(),
        addr: peer.addr.to_string(),
        endpoint_base_url: peer.endpoint_base_url.clone(),
        session_cookie: peer.session_cookie.clone(),
        channel_key_hex: hex::encode(peer.channel_key.as_bytes()),
        default_vault_id: peer.default_vault_id.clone(),
    };
    let json = serde_json::to_vec(&persisted)?;
    db.set_setting(
        SETTING_ACTIVE_PEER_JSON,
        &encrypt_setting(&json, protection_key, AAD)?,
    )?;
    Ok(())
}

#[derive(Deserialize)]
struct RemoteVaultSummary {
    vault_id: String,
}

#[derive(Deserialize)]
struct RemoteVaultsResponse {
    vaults: Vec<RemoteVaultSummary>,
}

async fn fetch_remote_default_vault_id(
    endpoint_base_url: &str,
    session_cookie: Option<&str>,
) -> Option<String> {
    let client = Client::builder().timeout(Duration::from_secs(5)).build().ok()?;
    let url = format!("{endpoint_base_url}/api/vaults");
    let mut req = client.get(&url);
    if let Some(cookie) = session_cookie {
        req = req.header("Cookie", format!("lv_session={cookie}"));
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: RemoteVaultsResponse = resp.json().await.ok()?;
    body.vaults.into_iter().next().map(|v| v.vault_id)
}

fn stop_client_background_tasks(life: &mut LifecycleState) {
    life.watchers.clear();
    if let Some(t) = life.watcher_dispatch_task.take() {
        t.abort();
    }
    if let Some(t) = life.reconnect_task.take() {
        t.abort();
    }
    if let Some(t) = life.reconnect_event_task.take() {
        t.abort();
    }
}

fn start_client_background_tasks(
    state: &Arc<FfiRuntimeState>,
    life: &mut LifecycleState,
    active_peer: Option<ActivePeerSession>,
) {
    stop_client_background_tasks(life);
    if life.role_mode == RoleMode::ServerOnly {
        return;
    }
    if state
        .client_db
        .get_setting(SETTING_SYNC_PAUSED)
        .ok()
        .flatten()
        .as_deref()
        == Some("1")
    {
        return;
    }
    // Reached from cold init (no ambient runtime) as well as from async callers,
    // so enter the FFI runtime explicitly before spawning or `tokio::spawn`.
    let _runtime_guard = state.runtime.enter();

    if let Ok(rules) = state.client_db.list_sync_rules() {
        let (watch_tx, mut watch_rx) = mpsc::channel::<WatchEvent>(64);
        let mut started = Vec::new();
        for rule in rules.into_iter().filter(|r| r.enabled) {
            let root = PathBuf::from(&rule.local_dir);
            if root.is_dir() {
                match DirectoryWatcher::start(rule.rule_id.clone(), root, watch_tx.clone()) {
                    Ok(w) => started.push(w),
                    Err(err) => warn!(rule_id = %rule.rule_id, "DirectoryWatcher skipped: {err}"),
                }
            }
        }
        if !started.is_empty() {
            life.watchers = started;
            life.watcher_dispatch_task = Some(state.runtime.spawn(async move {
                while let Some(ev) = watch_rx.recv().await {
                    let rule_id = match ev {
                        WatchEvent::PathsChanged { rule_id, .. } => rule_id,
                        WatchEvent::RescanRequired { rule_id, .. } => rule_id,
                    };
                    if let Err(err) = trigger_incremental_scan(rule_id.clone()).await {
                        warn!(rule_id = %rule_id, "auto incremental scan from watcher failed: {err}");
                    }
                }
            }));
        }
    }

    if let Some(peer) = active_peer {
        life.reconnect_task = Some(state.reconnect.spawn_peer_loop(ReconnectTarget {
            device_id: peer.device_id,
            addr: peer.addr,
        }));
        let mut rx = state.reconnect.subscribe();
        let db = Arc::clone(&state.client_db);
        let event_state = Arc::clone(state);
        life.reconnect_event_task = Some(state.runtime.spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ReconnectEvent::PeerConnected { .. }) => {
                        if let Ok(rules) = db.list_sync_rules() {
                            for rule in rules.into_iter().filter(|r| r.enabled) {
                                if let Err(err) =
                                    trigger_incremental_scan(rule.rule_id.clone()).await
                                {
                                    warn!(
                                        rule_id = %rule.rule_id,
                                        "reconnect reconciliation failed: {err}"
                                    );
                                }
                            }
                        }
                    }
                    Ok(ReconnectEvent::PeerDisconnected { .. }) => {}
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "reconnect event subscriber lagged");
                        if event_state.reconnect.is_enabled() {
                            if let Ok(rules) = db.list_sync_rules() {
                                for rule in rules.into_iter().filter(|r| r.enabled) {
                                    if let Err(err) =
                                        trigger_incremental_scan(rule.rule_id.clone()).await
                                    {
                                        warn!(
                                            rule_id = %rule.rule_id,
                                            "lagged reconciliation failed: {err}"
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }));
    }
}

fn is_daemon_mode_active(explicit_daemon: bool) -> bool {
    explicit_daemon
        || DAEMON_MODE.load(Ordering::Acquire)
        || std::env::var("LANVAULT_DAEMON_MODE").ok().as_deref() == Some("1")
}

fn runtime_state() -> Result<Arc<FfiRuntimeState>> {
    runtime_state_with_mode(false)
}

fn runtime_state_with_mode(explicit_daemon: bool) -> Result<Arc<FfiRuntimeState>> {
    if explicit_daemon {
        DAEMON_MODE.store(true, Ordering::Release);
    }
    if let Some(state) = GLOBAL_STATE.get() {
        return Ok(Arc::clone(state));
    }

    let _init_guard = INIT_LOCK
        .lock()
        .map_err(|_| anyhow!("internal lock poisoned"))?;
    if let Some(state) = GLOBAL_STATE.get() {
        return Ok(Arc::clone(state));
    }

    let daemon_mode = is_daemon_mode_active(explicit_daemon);
    let data_dir = client_data_dir();
    fs::create_dir_all(&data_dir).with_context(|| {
        format!(
            "failed to create LanVault client data directory at {}",
            data_dir.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&data_dir, fs::Permissions::from_mode(0o700))?;
    }

    lanvault_core::install_rust_panic_hook();
    lanvault_core::configure_debug_log_file(&data_dir.join("lanvault_debug.log"));

    let client_db = Arc::new(
        Database::open(data_dir.join("client_state.db"))
            .context("failed to open client_state.db")?,
    );
    let client_identity = DeviceIdentity::load_or_generate(&data_dir.join("client_device.key"))
        .context("failed to load or generate client DeviceIdentity")?;
    let client_device_id = format!("lv-{}", &client_identity.fingerprint_hex()[..12]);
    let local_storage_key = client_identity.derive_local_storage_key("client-state");
    let vault_master_key =
        load_or_generate_vault_master_key(&client_db, &local_storage_key)?;
    let persisted_role = if daemon_mode {
        RoleMode::ServerOnly
    } else {
        load_persisted_role_mode(&client_db)?
    };
    let persisted_policy = load_persisted_policy(&client_db)?;
    if !daemon_mode {
        ensure_default_sync_rules(&client_db, &persisted_policy)?;
    }
    let persisted_peer = if daemon_mode {
        None
    } else {
        load_persisted_peer(&client_db, &local_storage_key)?
    };
    let debug_enabled = client_db
        .get_setting(SETTING_DEBUG_MODE)?
        .as_deref()
        == Some("1");
    lanvault_core::set_debug_mode(debug_enabled);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("lanvault-ffi-rt")
        .build()
        .context("failed to initialize global Tokio runtime for LanVault FFI")?;

    let (progress_tx, _) = broadcast::channel(256);
    let reconnect_enabled = !daemon_mode
        && persisted_role != RoleMode::ServerOnly
        && client_db
            .get_setting(SETTING_AUTO_RECONNECT)?
            .as_deref()
            != Some("0");
    let rate_limiter = Arc::new(TokenBucket::new(persisted_policy.max_kbps));
    let state = Arc::new(FfiRuntimeState {
        runtime,
        client_db,
        client_identity,
        client_device_id,
        vault_master_key: RwLock::new(vault_master_key),
        lifecycle: AsyncMutex::new(LifecycleState {
            role_mode: persisted_role,
            last_server_config: None,
            server_handle: None,
            local_admin_cookie: None,
            watchers: Vec::new(),
            watcher_dispatch_task: None,
            reconnect_task: None,
            reconnect_event_task: None,
        }),
        reconnect: ReconnectController::new(reconnect_enabled),
        policy: Arc::new(RwLock::new(persisted_policy)),
        rate_limiter,
        progress_tx,
        latest_progress: Arc::new(Mutex::new(SyncProgressEvent::default())),
        discovered_peers: Arc::new(Mutex::new(HashMap::new())),
        mdns_browser_started: Mutex::new(false),
        active_peer: Arc::new(RwLock::new(persisted_peer.clone())),
        environment: Arc::new(RwLock::new(DeviceEnvironment {
            on_wifi: !cfg!(target_os = "android"),
            current_ssid: None,
            is_charging: !cfg!(target_os = "android"),
        })),
        sync_inflight: Arc::new(AsyncMutex::new(HashSet::new())),
    });

    if !daemon_mode && persisted_role != RoleMode::ServerOnly {
        let mut life = state.lifecycle.blocking_lock();
        start_client_background_tasks(&state, &mut life, persisted_peer);
    }

    if GLOBAL_STATE.set(Arc::clone(&state)).is_err() {
        return GLOBAL_STATE
            .get()
            .cloned()
            .ok_or_else(|| anyhow!("FFI runtime state initialization race"));
    }
    Ok(state)
}

/// 供 `c_abi` 与 Android JNI 同步桥接复用的全局 Tokio 执行入口。
pub(crate) fn block_on<F: Future>(future: F) -> Result<F::Output> {
    let state = runtime_state()?;
    Ok(state.runtime.block_on(future))
}

/// 供 Android 特权守护进程（Shizuku / Root）使用的全局 Tokio 执行入口：
/// 初始化时强制处于守护进程服务端模式，不创建默认同步规则也不启动客户端目录监听。
#[cfg(target_os = "android")]
pub(crate) fn block_on_daemon<F: Future>(future: F) -> Result<F::Output> {
    let state = runtime_state_with_mode(true)?;
    Ok(state.runtime.block_on(future))
}

fn read_bit_rot_health_from_db(db_path: &Path) -> (String, u64) {
    if !db_path.exists() {
        return ("unknown".to_string(), 0);
    }
    let Ok(db) = Database::open(db_path) else {
        return ("unknown".to_string(), 0);
    };
    let Ok(corrupted_chunks) = db.count_corrupted_chunks() else {
        return ("unknown".to_string(), 0);
    };
    if corrupted_chunks > 0 {
        return ("corrupted".to_string(), corrupted_chunks);
    }
    let any_volume_offline = db
        .list_vaults()
        .ok()
        .map(|vaults| {
            vaults.iter().any(|v| {
                db.list_vault_volumes(&v.vault_id)
                    .map(|vols| vols.iter().any(|vol| !vol.is_online))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if any_volume_offline {
        ("degraded".to_string(), 0)
    } else {
        ("healthy".to_string(), 0)
    }
}

fn query_local_bit_rot_health(state: &FfiRuntimeState) -> (String, u64) {
    let db_path = {
        let Ok(life) = state.lifecycle.try_lock() else {
            return ("unknown".to_string(), 0);
        };
        match life.server_handle.as_ref() {
            Some(handle) if !handle.task.is_finished() => Some(handle.db_path.clone()),
            _ => None,
        }
    };
    let Some(db_path) = db_path else {
        return ("unknown".to_string(), 0);
    };
    read_bit_rot_health_from_db(&db_path)
}

fn emit_progress(state: &FfiRuntimeState, mut event: SyncProgressEvent) {
    let (bit_rot_health, corrupted_chunks) = query_local_bit_rot_health(state);
    event.bit_rot_health = bit_rot_health;
    event.corrupted_chunks = corrupted_chunks;
    match state.latest_progress.lock() {
        Ok(mut guard) => *guard = event.clone(),
        Err(_) => error!("internal lock poisoned while updating latest_progress"),
    }
    let _ = state.progress_tx.send(event);
}

async fn stop_local_server_gracefully(lifecycle: &mut LifecycleState) -> Result<()> {
    lifecycle.local_admin_cookie = None;
    if let Some(mut handle) = lifecycle.server_handle.take() {
        let _ = handle.stop_tx.send(true);

        if tokio::time::timeout(GRACEFUL_DRAIN_TIMEOUT, &mut handle.task)
            .await
            .is_err()
        {
            warn!(
                vault = %handle.vault_path.display(),
                "local LanVault server did not exit within 3s drain window; aborting task"
            );
            handle.task.abort();
        }

        if handle.db_path.exists() {
            let db = Database::open(&handle.db_path).with_context(|| {
                format!("failed to open SQLite at {}", handle.db_path.display())
            })?;
            db.wal_checkpoint_truncate()
                .context("failed to execute WAL checkpoint during server shutdown")?;
        }

        info!(
            vault = %handle.vault_path.display(),
            "local LanVault server stopped gracefully (WAL truncated)"
        );
    }

    Ok(())
}

/// 供 `c_abi` 轮询获取最新一次同步进度事件。
pub(crate) fn latest_sync_progress() -> Result<SyncProgressEvent> {
    let state = runtime_state()?;
    let mut snapshot = state
        .latest_progress
        .lock()
        .map_err(|_| anyhow!("internal lock poisoned"))?
        .clone();
    let (bit_rot_health, corrupted_chunks) = query_local_bit_rot_health(&state);
    snapshot.bit_rot_health = bit_rot_health;
    snapshot.corrupted_chunks = corrupted_chunks;
    Ok(snapshot)
}

/// 供 `c_abi` 启动（幂等）后台 mDNS + UDP 7843 双通道发现并返回当前已发现的合法对端列表。
pub(crate) fn poll_mdns_peers() -> Result<Vec<DiscoveredPeer>> {
    let state = runtime_state()?;
    let should_start = {
        let mut started = state
            .mdns_browser_started
            .lock()
            .map_err(|_| anyhow!("internal lock poisoned"))?;
        if *started {
            false
        } else {
            *started = true;
            true
        }
    };

    if should_start {
        let _runtime_guard = state.runtime.enter();
        let (tx, mut rx) = mpsc::channel::<DiscoveredPeerInfo>(64);
        let browser = match DiscoveryBrowser::start(state.client_device_id.clone(), tx) {
            Ok(browser) => browser,
            Err(err) => {
                if let Ok(mut started) = state.mdns_browser_started.lock() {
                    *started = false;
                }
                return Err(err).context("failed to start dual-channel DiscoveryBrowser");
            }
        };
        let peers_cache = Arc::clone(&state.discovered_peers);

        state.runtime.spawn(async move {
            let _browser_guard = browser;
            while let Some(info) = rx.recv().await {
                let peer: DiscoveredPeer = info.into();
                if let Ok(mut guard) = peers_cache.lock() {
                    guard.insert(peer.device_id.clone(), peer);
                } else {
                    error!("internal lock poisoned while updating discovered_peers");
                    break;
                }
            }
        });
    }

    let mut peers_cache = state
        .discovered_peers
        .lock()
        .map_err(|_| anyhow!("internal lock poisoned"))?;
    peers_cache.retain(|_, peer| peer.last_seen.elapsed() < Duration::from_secs(30));
    let peers = peers_cache.values().cloned().collect();
    drop(peers_cache);
    Ok(peers)
}

/// 1. 启动 mDNS + UDP 7843 局域网双通道设备发现（自动过滤本机）并通过 `StreamSink` 流式推送至 Flutter。
pub fn start_mdns_scan(sink: StreamSink<DiscoveredPeer>) -> Result<()> {
    let state = runtime_state()?;
    {
        let mut started = state
            .mdns_browser_started
            .lock()
            .map_err(|_| anyhow!("internal lock poisoned"))?;
        if *started {
            return Ok(());
        }
        *started = true;
    }
    let _runtime_guard = state.runtime.enter();
    let (tx, mut rx) = mpsc::channel::<DiscoveredPeerInfo>(64);
    let browser = match DiscoveryBrowser::start(state.client_device_id.clone(), tx) {
        Ok(browser) => browser,
        Err(err) => {
            if let Ok(mut started) = state.mdns_browser_started.lock() {
                *started = false;
            }
            return Err(err).context("failed to start dual-channel DiscoveryBrowser");
        }
    };
    let peers_cache = Arc::clone(&state.discovered_peers);
    let scan_state = Arc::clone(&state);

    state.runtime.spawn(async move {
        while let Some(info) = rx.recv().await {
            let peer: DiscoveredPeer = info.into();
            if let Ok(mut guard) = peers_cache.lock() {
                guard.insert(peer.device_id.clone(), peer.clone());
            } else {
                error!("internal lock poisoned while updating discovered_peers");
                break;
            }

            if sink.add(peer).is_err() {
                info!("Dart discovery StreamSink closed; stopping DiscoveryBrowser");
                break;
            }
        }
        let _ = browser.shutdown();
        if let Ok(mut started) = scan_state.mdns_browser_started.lock() {
            *started = false;
        }
    });

    Ok(())
}

#[derive(Serialize)]
struct PairPinRequest {
    pin: String,
    client_ed25519_pubkey_hex: String,
    client_x25519_pubkey_hex: String,
    client_ed25519_signature_hex: String,
    device_name: String,
    os_type: String,
}

#[derive(Deserialize)]
struct PairPinResponse {
    device_id: String,
    #[serde(default)]
    device_name: Option<String>,
    #[serde(default)]
    os_type: Option<String>,
    server_ed25519_pubkey_hex: String,
    server_x25519_pubkey_hex: String,
    server_ed25519_signature_hex: String,
    clipboard_key_envelope_hex: String,
    session_cookie: Option<String>,
}

/// 2. 使用 6 位动态 PIN 码执行 X25519 + Ed25519 零信任配对，返回对端 `device_id`。
pub async fn pair_with_pin(ip: String, port: u16, pin: String) -> Result<String> {
    let _probe = lanvault_core::ProbeGuard::start(
        "rust.ffi",
        "ffi.pair_with_pin",
        Some(format!("target={ip}:{port}")),
    );
    let parsed_ip: IpAddr = ip
        .trim()
        .parse()
        .with_context(|| format!("invalid peer IP address: `{ip}`"))?;
    if port == 0 {
        bail!("peer port must be non-zero");
    }
    if pin.len() != 6 || !pin.chars().all(|c| c.is_ascii_digit()) {
        bail!("pairing PIN must be a 6-digit numeric string");
    }

    let state = runtime_state()?;
    let kex = SessionKeyExchange::new();
    let client_ed25519_pubkey = state.client_identity.public_key_bytes();
    let client_x25519_pubkey = kex.public_key_bytes();
    let device_name = current_hostname();
    let os_type = std::env::consts::OS.to_string();
    let identity_message = pairing_identity_message(
        &pin,
        &client_ed25519_pubkey,
        &client_x25519_pubkey,
        &device_name,
        &os_type,
    );
    let client_signature = state.client_identity.sign(&identity_message);

    let socket_addr = SocketAddr::new(parsed_ip, port);
    let endpoint_base_url = format!("http://{socket_addr}");
    let url = format!("{endpoint_base_url}/api/peers/pair");

    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
    let resp = client
        .post(&url)
        .json(&PairPinRequest {
            pin: pin.clone(),
            client_ed25519_pubkey_hex: hex::encode(client_ed25519_pubkey),
            client_x25519_pubkey_hex: hex::encode(client_x25519_pubkey),
            client_ed25519_signature_hex: hex::encode(client_signature),
            device_name,
            os_type,
        })
        .send()
        .await
        .with_context(|| format!("pairing request to {url} failed"))?;

    if !resp.status().is_success() {
        bail!(
            "peer rejected PIN pairing with HTTP {}",
            resp.status().as_u16()
        );
    }

    let body: PairPinResponse = resp.json().await.context("invalid pairing response JSON")?;
    let server_ed25519_pubkey = hex::decode(&body.server_ed25519_pubkey_hex)
        .context("invalid server_ed25519_pubkey_hex")?;
    let peer_pub_bytes = hex::decode(&body.server_x25519_pubkey_hex)
        .context("invalid server_x25519_pubkey_hex")?;
    let server_signature = hex::decode(&body.server_ed25519_signature_hex)
        .context("invalid server_ed25519_signature_hex")?;
    let clipboard_envelope = hex::decode(&body.clipboard_key_envelope_hex)
        .context("invalid clipboard_key_envelope_hex")?;
    let response_message = pairing_response_message(
        &pin,
        &client_ed25519_pubkey,
        &client_x25519_pubkey,
        &server_ed25519_pubkey,
        &peer_pub_bytes,
        &body.device_id,
        &clipboard_envelope,
    );
    verify_ed25519(
        &server_ed25519_pubkey,
        &response_message,
        &server_signature,
    )
    .context("server identity signature verification failed")?;
    let server_public_key: [u8; 32] = server_ed25519_pubkey
        .try_into()
        .map_err(|_| anyhow!("invalid server Ed25519 public key length"))?;
    let expected_server_device_id = format!(
        "lv-{}",
        &lanvault_core::public_key_fingerprint(&server_public_key)[..12]
    );
    if body.device_id != expected_server_device_id {
        bail!("server device id does not match its signing key");
    }
    let session_key = kex
        .agree(&peer_pub_bytes, &identity_message)
        .context("X25519 session key agreement failed")?;
    let clipboard_key = decrypt_aead(
        &session_key,
        &clipboard_envelope,
        b"lanvault.clipboard.key.v1",
    )
    .context("failed to decrypt clipboard group key")?;
    let channel_key = SymmetricKey::from_slice(&clipboard_key)
        .context("server returned an invalid clipboard group key")?;

    let discovered_meta = state
        .discovered_peers
        .lock()
        .map_err(|_| anyhow!("internal lock poisoned"))?
        .get(&body.device_id)
        .map(|peer| (peer.device_name.clone(), peer.os_type.clone()));
    let existing_trusted = state.client_db.get_trusted_peer(&body.device_id)?;
    let peer_device_name = body
        .device_name
        .filter(|name| !name.trim().is_empty())
        .or_else(|| discovered_meta.as_ref().map(|(name, _)| name.clone()))
        .or_else(|| existing_trusted.as_ref().map(|peer| peer.device_name.clone()))
        .unwrap_or_else(|| body.device_id.clone());
    let peer_os_type = body
        .os_type
        .filter(|os| !os.trim().is_empty())
        .or_else(|| discovered_meta.map(|(_, os)| os))
        .or_else(|| existing_trusted.map(|peer| peer.os_type))
        .unwrap_or_else(|| "unknown".to_string());
    let paired_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    state.client_db.upsert_trusted_peer(&TrustedPeer {
        device_id: body.device_id.clone(),
        device_name: peer_device_name,
        os_type: peer_os_type,
        ed25519_pubkey: server_public_key.to_vec(),
        last_ip: Some(parsed_ip.to_string()),
        last_port: Some(port),
        auto_connect: state.reconnect.is_enabled(),
        paired_at,
        revoked_at: None,
    })?;

    let default_vault_id =
        fetch_remote_default_vault_id(&endpoint_base_url, body.session_cookie.as_deref()).await;
    let session = ActivePeerSession {
        device_id: body.device_id.clone(),
        addr: socket_addr,
        endpoint_base_url,
        session_cookie: body.session_cookie,
        channel_key,
        default_vault_id,
    };
    save_persisted_peer(
        &state.client_db,
        &session,
        &state
            .client_identity
            .derive_local_storage_key("client-state"),
    )?;
    *state.active_peer.write().await = Some(session.clone());

    let mut life = state.lifecycle.lock().await;
    start_client_background_tasks(&state, &mut life, Some(session));

    info!(peer_device_id = %body.device_id, addr = %socket_addr, "paired with peer via PIN");
    Ok(body.device_id)
}

/// 3. 开启或暂停后台自愈重连状态机（`2s -> 5s -> 15s -> 30s`）。
pub async fn enable_auto_reconnect(enabled: bool) -> Result<()> {
    let state = runtime_state()?;
    let role_mode = state.lifecycle.lock().await.role_mode;
    if role_mode == RoleMode::ServerOnly {
        bail!(
            "auto-reconnect is unavailable in `server_only` mode; switch role_mode to `client_only` or `dual` first"
        );
    }
    state.reconnect.set_enabled(enabled);
    state.client_db.set_setting(
        SETTING_AUTO_RECONNECT,
        if enabled { "1" } else { "0" },
    )?;
    Ok(())
}

/// 供 Android 特权守护进程（Shizuku / Root）启动本地服务端：
/// 强制进入守护进程服务端模式，不初始化默认同步规则且不启动客户端后台任务。
pub async fn start_local_server_for_daemon(
    vault_path: String,
    quota_gb: u64,
    web_password: String,
    requested_port: Option<u16>,
) -> Result<u16> {
    start_local_server_internal(vault_path, quota_gb, web_password, requested_port, true).await
}

/// 内部复用入口：支持 Android JNI 透传指定监听端口，同时被 `start_local_server` 复用。
pub async fn start_local_server_with_port(
    vault_path: String,
    quota_gb: u64,
    web_password: String,
    requested_port: Option<u16>,
) -> Result<u16> {
    start_local_server_internal(vault_path, quota_gb, web_password, requested_port, false).await
}

async fn start_local_server_internal(
    vault_path: String,
    quota_gb: u64,
    web_password: String,
    requested_port: Option<u16>,
    daemon_mode: bool,
) -> Result<u16> {
    let trimmed_vault = vault_path.trim();
    if trimmed_vault.is_empty() {
        bail!("vault_path must not be empty");
    }
    if quota_gb == 0 {
        bail!("quota_gb must be greater than 0");
    }
    let trimmed_password = web_password.trim();
    if trimmed_password.len() < 8 {
        bail!("web_password must be at least 8 characters");
    }

    let state = runtime_state_with_mode(daemon_mode)?;
    let root = PathBuf::from(trimmed_vault);
    let port = requested_port.unwrap_or(DEFAULT_SERVER_PORT);
    let quota_bytes = quota_gb.saturating_mul(1024 * 1024 * 1024);
    let password_hash = blake3_hash(trimmed_password.as_bytes());
    let mut life = state.lifecycle.lock().await;

    if daemon_mode {
        life.role_mode = RoleMode::ServerOnly;
        state.reconnect.set_enabled(false);
        stop_client_background_tasks(&mut life);
    } else if life.role_mode == RoleMode::ClientOnly {
        bail!(
            "cannot start local server in `client_only` mode; switch role_mode to `server_only` or `dual` first"
        );
    }

    if let Some(existing) = life.server_handle.as_ref() {
        if !existing.task.is_finished() {
            let port_matches = port == 0 || existing.port == port;
            if !port_matches
                || existing.vault_path != root
                || existing.quota_bytes != quota_bytes
                || existing.password_hash != password_hash
            {
                bail!(
                    "a local server is already running on port {} with vault {}",
                    existing.port,
                    existing.vault_path.display()
                );
            }
            return Ok(existing.port);
        }
        life.server_handle = None;
    }

    let db_path = root.join("index.db");
    let config = ServerConfig {
        bind_addr: "0.0.0.0".to_string(),
        port,
        vault_root: root.clone(),
        default_quota_bytes: quota_bytes,
        web_password: trimmed_password.to_string(),
        device_name: current_hostname(),
        ..ServerConfig::default()
    };
    let mut reusable_config = config.clone();

    let (stop_tx, stop_rx) = watch::channel(false);
    let (bound_tx, bound_rx) = oneshot::channel::<SocketAddr>();
    let mut initial_bound_tx = Some(bound_tx);

    let task = state.runtime.spawn(async move {
        let loop_config = config;
        loop {
            let (iter_shutdown_tx, iter_shutdown_rx) = oneshot::channel::<()>();
            let mut stop_rx_iter = stop_rx.clone();
            let stop_bridge = tokio::spawn(async move {
                if *stop_rx_iter.borrow() || stop_rx_iter.changed().await.is_ok() {
                    let _ = iter_shutdown_tx.send(());
                }
            });

            let current_bound_tx = initial_bound_tx.take();
            let exit_res = lanvault_server::serve_with_shutdown(
                loop_config.clone(),
                iter_shutdown_rx,
                current_bound_tx,
            )
            .await;
            stop_bridge.abort();

            match exit_res {
                Ok(ServerExitReason::GracefulRestart) => {
                    if *stop_rx.borrow() {
                        break;
                    }
                    info!("local LanVault server triggered GracefulRestart; restarting instance");
                    continue;
                }
                Ok(ServerExitReason::Stopped) => {
                    info!("local LanVault server stopped");
                    break;
                }
                Err(err) => {
                    error!(error = %err, "local LanVault server exited with error");
                    break;
                }
            }
        }
    });

    let bound_addr = match tokio::time::timeout(SERVER_BIND_TIMEOUT, bound_rx).await {
        Ok(Ok(addr)) => addr,
        Ok(Err(_)) => {
            task.abort();
            bail!("local LanVault server failed to bind or exited during startup");
        }
        Err(_) => {
            let _ = stop_tx.send(true);
            task.abort();
            bail!("timed out waiting for local LanVault server to bind");
        }
    };

    let actual_port = bound_addr.port();
    reusable_config.port = actual_port;
    life.local_admin_cookie = None;
    life.last_server_config = Some(reusable_config);
    life.server_handle = Some(LocalServerHandle {
        port: actual_port,
        vault_path: root.clone(),
        db_path,
        quota_bytes,
        password_hash,
        stop_tx,
        task,
    });

    let persisted_cfg = LastServerConfigDto {
        vault_path: root.to_string_lossy().into_owned(),
        quota_gb,
        port: actual_port,
    };
    let persisted_json = serde_json::to_string(&persisted_cfg)?;
    state
        .client_db
        .set_setting(SETTING_LAST_SERVER_CONFIG_JSON, &persisted_json)?;

    info!(
        port = actual_port,
        quota_gb,
        daemon_mode,
        "started local LanVault server"
    );
    Ok(actual_port)
}

/// 4. 启动本地真实 `lanvault-server` 服务端（由服务端独占初始化存储空间、SQLite WAL、Axum Web/WS 与 mDNS 广播），返回实际监听端口。
pub async fn start_local_server(
    vault_path: String,
    quota_gb: u64,
    web_password: String,
) -> Result<u16> {
    start_local_server_with_port(vault_path, quota_gb, web_password, None).await
}

/// 5. 准备优雅重启：停接新任务 -> 等待 ≤3s 排空（超时强杀防僵尸）-> 执行 `PRAGMA wal_checkpoint(TRUNCATE)` 刷盘。
pub async fn prepare_graceful_restart() -> Result<()> {
    let state = runtime_state()?;
    let mut life = state.lifecycle.lock().await;
    stop_local_server_gracefully(&mut life).await
}

/// 6. 订阅实时同步进度事件流。
pub fn subscribe_sync_progress(sink: StreamSink<SyncProgressEvent>) -> Result<()> {
    let state = runtime_state()?;
    let mut rx = state.progress_tx.subscribe();

    state.runtime.spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if sink.add(event).is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "sync progress subscriber lagged");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    Ok(())
}

/// 7. 手动触发指定同步规则的全量元数据 Diff 扫描与增量上传（独立使用 `client_state.db`，`client_only` 模式下完全可用）。
pub async fn trigger_incremental_scan(rule_id: String) -> Result<()> {
    let state = runtime_state()?;
    if rule_id.trim().is_empty() {
        bail!("rule_id must not be empty");
    }
    {
        let mut in_flight = state.sync_inflight.lock().await;
        if !in_flight.insert(rule_id.clone()) {
            bail!("sync rule `{rule_id}` is already running");
        }
    }
    let result = trigger_incremental_scan_inner(rule_id.clone()).await;
    state.sync_inflight.lock().await.remove(&rule_id);
    if let Err(err) = &result {
        let previous = state
            .latest_progress
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        emit_progress(
            &state,
            SyncProgressEvent {
                rule_id,
                current_file: String::new(),
                bytes_processed: previous.bytes_processed,
                total_bytes: previous.total_bytes,
                chunks_uploaded: previous.chunks_uploaded,
                chunks_deduplicated: previous.chunks_deduplicated,
                throughput_bps: 0,
                status: format!("failed: {err}"),
                bit_rot_health: previous.bit_rot_health,
                corrupted_chunks: previous.corrupted_chunks,
            },
        );
    }
    result
}

async fn trigger_incremental_scan_inner(rule_id: String) -> Result<()> {
    if rule_id.trim().is_empty() {
        bail!("rule_id must not be empty");
    }

    let state = runtime_state()?;
    {
        let life = state.lifecycle.lock().await;
        if life.role_mode == RoleMode::ServerOnly {
            bail!(
                "cannot trigger incremental scan in `server_only` mode; switch role_mode to `client_only` or `dual` first"
            );
        }
    }

    let rule = state
        .client_db
        .get_sync_rule(&rule_id)?
        .filter(|r| r.enabled)
        .ok_or_else(|| anyhow!("sync rule `{rule_id}` not found or disabled"))?;

    let peer = state
        .active_peer
        .read()
        .await
        .clone()
        .ok_or_else(|| anyhow!("no paired peer session active for incremental upload"))?;
    let resolved_vault_id = if rule.target_vault_id == "default"
        || rule.target_vault_id == "vault-primary"
    {
        match peer.default_vault_id.clone() {
            Some(vault_id) => vault_id,
            None => fetch_remote_default_vault_id(
                &peer.endpoint_base_url,
                peer.session_cookie.as_deref(),
            )
            .await
            .ok_or_else(|| anyhow!("remote peer has no available vault"))?,
        }
    } else {
        rule.target_vault_id.clone()
    };

    let diff = reconcile_directory(
        &state.client_db,
        &rule_id,
        &resolved_vault_id,
        Path::new(&rule.local_dir),
    )?;
    let total_bytes: u64 = diff.changed_files().map(|f| f.size).sum();

    emit_progress(
        &state,
        SyncProgressEvent {
            rule_id: rule_id.clone(),
            current_file: String::new(),
            bytes_processed: 0,
            total_bytes,
            chunks_uploaded: 0,
            chunks_deduplicated: 0,
            throughput_bps: 0,
            status: if diff.is_empty() {
                "up_to_date".to_string()
            } else {
                "scanning".to_string()
            },
            ..SyncProgressEvent::default()
        },
    );

    if diff.is_empty() {
        return Ok(());
    }

    let policy = state.policy.read().await.clone();
    let environment = state.environment.read().await.clone();
    let engine = SyncEngine::new(
        peer.endpoint_base_url.clone(),
        peer.session_cookie.clone(),
        state.client_device_id.clone(),
        state.vault_master_key.read().await.clone(),
        policy,
        environment,
        Some(Arc::clone(&state.rate_limiter)),
    )?;

    let mut engine_rx = engine.subscribe_progress();
    let forward_state = Arc::clone(&state);
    let forwarder = tokio::spawn(async move {
        while let Ok(ev) = engine_rx.recv().await {
            let ffi_ev = SyncProgressEvent {
                rule_id: ev.rule_id,
                current_file: ev.current_file,
                bytes_processed: ev.bytes_processed,
                total_bytes: ev.total_bytes,
                chunks_uploaded: ev.chunks_uploaded,
                chunks_deduplicated: ev.chunks_deduplicated,
                throughput_bps: ev.throughput_bps,
                status: ev.status,
                ..SyncProgressEvent::default()
            };
            emit_progress(&forward_state, ffi_ev);
        }
    });

    let sync_result = engine
        .sync_diff(&state.client_db, &rule_id, &resolved_vault_id, &diff)
        .await;
    forwarder.abort();
    let summary = sync_result?;

    emit_progress(
        &state,
        SyncProgressEvent {
            rule_id,
            current_file: String::new(),
            bytes_processed: summary.bytes_processed,
            total_bytes,
            chunks_uploaded: summary.chunks_uploaded,
            chunks_deduplicated: summary.chunks_deduplicated,
            throughput_bps: 0,
            status: "completed".to_string(),
            ..SyncProgressEvent::default()
        },
    );

    Ok(())
}

/// 8. 更新场景化省电、Wi-Fi SSID 白名单与令牌桶限速策略（持久化至 `client_state.db`，`client_only` 模式可用）。
pub async fn update_power_network_policy(
    wifi_only: bool,
    allowed_ssids: Vec<String>,
    require_charging: bool,
    max_kbps: u32,
) -> Result<()> {
    let state = runtime_state()?;
    let new_policy = PowerNetworkPolicy {
        wifi_only,
        allowed_ssids: allowed_ssids.clone(),
        require_charging,
        max_kbps,
    };

    let policy_json = serde_json::to_string(&new_policy)?;
    state
        .client_db
        .set_setting(SETTING_POWER_POLICY_JSON, &policy_json)?;

    let ssids_json = if allowed_ssids.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&allowed_ssids)?)
    };
    for mut rule in state.client_db.list_sync_rules()? {
        rule.wifi_only = wifi_only;
        rule.allowed_ssids = ssids_json.clone();
        rule.require_charging = require_charging;
        rule.max_kbps = max_kbps;
        state.client_db.upsert_sync_rule(&rule)?;
    }

    state.rate_limiter.update_max_kbps(max_kbps).await;
    *state.policy.write().await = new_policy;

    info!(
        wifi_only,
        require_charging, max_kbps, "power/network sync policy updated"
    );
    Ok(())
}

pub async fn update_device_environment(
    on_wifi: bool,
    current_ssid: Option<String>,
    is_charging: bool,
) -> Result<()> {
    let state = runtime_state()?;
    *state.environment.write().await = DeviceEnvironment {
        on_wifi,
        current_ssid,
        is_charging,
    };
    state.reconnect.notify_network_restored();
    Ok(())
}

fn derive_key_backup_key(passphrase: &str, salt: &[u8]) -> Result<SymmetricKey> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|err| anyhow!("failed to derive key-backup encryption key: {err}"))?;
    Ok(SymmetricKey::from_bytes(key))
}

pub async fn export_vault_key_backup(passphrase: String) -> Result<String> {
    if passphrase.len() < 12 {
        bail!("key-backup passphrase must be at least 12 characters");
    }
    let state = runtime_state()?;
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let backup_key = derive_key_backup_key(&passphrase, &salt)?;
    let vault_key = state.vault_master_key.read().await.clone();
    let ciphertext = encrypt_aead(
        &backup_key,
        vault_key.as_bytes(),
        b"lanvault.key-backup.v1",
    )?;
    Ok(serde_json::to_string(&VaultKeyBackup {
        version: 1,
        salt_hex: hex::encode(salt),
        ciphertext_hex: hex::encode(ciphertext),
    })?)
}

pub async fn import_vault_key_backup(passphrase: String, backup_json: String) -> Result<()> {
    let backup: VaultKeyBackup =
        serde_json::from_str(&backup_json).context("invalid vault key backup")?;
    if backup.version != 1 {
        bail!("unsupported vault key backup version");
    }
    let salt = hex::decode(backup.salt_hex).context("invalid key-backup salt")?;
    let ciphertext =
        hex::decode(backup.ciphertext_hex).context("invalid key-backup ciphertext")?;
    let backup_key = derive_key_backup_key(&passphrase, &salt)?;
    let plaintext = decrypt_aead(
        &backup_key,
        &ciphertext,
        b"lanvault.key-backup.v1",
    )
    .context("key-backup passphrase is incorrect or backup is corrupted")?;
    let key = SymmetricKey::from_slice(&plaintext)
        .context("vault key backup contains an invalid key")?;
    let state = runtime_state()?;
    if !state.sync_inflight.lock().await.is_empty() {
        bail!("cannot replace the vault key while sync jobs are running");
    }
    let protection_key = state
        .client_identity
        .derive_local_storage_key("client-state");
    state.client_db.set_setting(
        SETTING_VAULT_MASTER_KEY_HEX,
        &encrypt_setting(
            key.as_bytes(),
            &protection_key,
            b"lanvault.local.vault-master-key.v1",
        )?,
    )?;
    state.client_db.clear_local_file_states()?;
    *state.vault_master_key.write().await = key;
    Ok(())
}

/// 9. 复用加密信道推送局域网剪贴板文本至目标设备（或全体在线设备）。
pub async fn push_clipboard_text(target_device_id: Option<String>, text: String) -> Result<()> {
    let mut _probe = lanvault_core::ProbeGuard::start(
        "rust.ffi",
        "ffi.push_clipboard_text",
        Some(format!("target={}", target_device_id.as_deref().unwrap_or("broadcast"))),
    );
    _probe.add_bytes(text.len() as u64);
    if text.is_empty() {
        bail!("clipboard text must not be empty");
    }

    let state = runtime_state()?;
    let peer = state
        .active_peer
        .read()
        .await
        .clone()
        .ok_or_else(|| anyhow!("no paired peer session active for clipboard push"))?;

    let client = ClipboardClient::new(
        peer.endpoint_base_url,
        peer.session_cookie,
        peer.channel_key,
    )?;
    let target_label = target_device_id
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| peer.device_id.clone());
    let resp = client.push_text(target_device_id, &text).await?;
    let now_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let preview: String = text.chars().take(120).collect();
    let _ = state.client_db.insert_transfer_record(
        "sent",
        "clipboard",
        &preview,
        &format!("剪贴板文本 · 发送至 {target_label}"),
        &target_label,
        Some(&text),
        text.len() as u64,
        now_ts,
    );
    info!(
        delivered_to = resp.delivered_to,
        peer = %peer.device_id,
        "clipboard text pushed via encrypted channel"
    );
    Ok(())
}

/// 10. 切换统一三态运行模式并持久化至 `client_state.db`。
///
/// 取值：`client_only`（停本地服+启用客户端）、`server_only`（停客户端自动重连+维持服务端）、
/// `dual`（双端均允许运行）。
pub async fn set_role_mode(mode: String) -> anyhow::Result<String> {
    let _probe = lanvault_core::ProbeGuard::start(
        "rust.ffi",
        "ffi.set_role_mode",
        Some(format!("target_mode={mode}")),
    );
    let target_mode = parse_role_mode(&mode)?;
    let state = runtime_state()?;
    let mut life = state.lifecycle.lock().await;
    let previous_mode = life.role_mode;
    let previous_reconnect = state.reconnect.is_enabled();
    let persisted_auto_reconnect = state
        .client_db
        .get_setting(SETTING_AUTO_RECONNECT)?
        .as_deref()
        != Some("0");

    if target_mode == RoleMode::ClientOnly {
        stop_local_server_gracefully(&mut life).await?;
    }

    state
        .client_db
        .set_setting(SETTING_ROLE_MODE, target_mode.as_str())?;
    life.role_mode = target_mode;

    let active_peer = state.active_peer.read().await.clone();
    match target_mode {
        RoleMode::ClientOnly => {
            state.reconnect.set_enabled(persisted_auto_reconnect);
            start_client_background_tasks(&state, &mut life, active_peer);
        }
        RoleMode::ServerOnly => {
            state.reconnect.set_enabled(false);
            stop_client_background_tasks(&mut life);
        }
        RoleMode::Dual => {
            state.reconnect.set_enabled(persisted_auto_reconnect);
            start_client_background_tasks(&state, &mut life, active_peer);
        }
    }
    let restart_config = if target_mode != RoleMode::ClientOnly
        && life
            .server_handle
            .as_ref()
            .is_none_or(|handle| handle.task.is_finished())
    {
        life.last_server_config.clone()
    } else {
        None
    };
    drop(life);

    if let Some(config) = restart_config {
        let quota_gb = (config.default_quota_bytes / (1024 * 1024 * 1024)).max(1);
        if let Err(err) = start_local_server_with_port(
            config.vault_root.to_string_lossy().into_owned(),
            quota_gb,
            config.web_password,
            Some(config.port),
        )
        .await
        {
            state
                .client_db
                .set_setting(SETTING_ROLE_MODE, previous_mode.as_str())?;
            let mut rollback_life = state.lifecycle.lock().await;
            rollback_life.role_mode = previous_mode;
            state.reconnect.set_enabled(previous_reconnect);
            if previous_mode == RoleMode::ServerOnly {
                stop_client_background_tasks(&mut rollback_life);
            } else {
                let rollback_peer = state.active_peer.read().await.clone();
                start_client_background_tasks(&state, &mut rollback_life, rollback_peer);
            }
            return Err(err);
        }
    }

    info!(role_mode = target_mode.as_str(), "runtime role_mode updated and persisted");
    Ok(target_mode.as_str().to_string())
}

/// 11. 获取当前统一三态运行模式（`client_only` / `server_only` / `dual`）。
pub async fn get_role_mode() -> anyhow::Result<String> {
    let state = runtime_state()?;
    let mode = state.lifecycle.lock().await.role_mode;
    Ok(mode.as_str().to_string())
}

fn current_hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "lanvault-node".to_string())
}


/// 12. 列出本地持久化的全部增量同步规则。
pub async fn list_sync_rules() -> Result<Vec<SyncRuleDto>> {
    let state = runtime_state()?;
    let rules = state.client_db.list_sync_rules()?;
    let mut out = Vec::with_capacity(rules.len());
    for r in rules {
        let sched = state
            .client_db
            .get_setting(&format!("{SETTING_RULE_SCHEDULE_PREFIX}{}", r.rule_id))?
            .unwrap_or_else(|| "realtime".to_string());
        let mut dto = SyncRuleDto::from(r);
        dto.schedule_mode = sched;
        out.push(dto);
    }
    Ok(out)
}

/// 13. 新增或更新增量同步规则并刷新后台实时目录监听器。
pub async fn upsert_sync_rule(
    rule_id: String,
    local_dir: String,
    target_vault_id: String,
    enabled: bool,
) -> Result<SyncRuleDto> {
    if rule_id.trim().is_empty() || local_dir.trim().is_empty() {
        bail!("rule_id and local_dir must not be empty");
    }
    let state = runtime_state()?;
    let policy = state.policy.read().await.clone();
    let ssids_json = if policy.allowed_ssids.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&policy.allowed_ssids)?)
    };
    let record = SyncRuleRecord {
        rule_id: rule_id.trim().to_string(),
        local_dir: local_dir.trim().to_string(),
        target_vault_id: if target_vault_id.trim().is_empty() {
            "default".to_string()
        } else {
            target_vault_id.trim().to_string()
        },
        wifi_only: policy.wifi_only,
        allowed_ssids: ssids_json,
        require_charging: policy.require_charging,
        max_kbps: policy.max_kbps,
        enabled,
    };
    state.client_db.upsert_sync_rule(&record)?;

    let active_peer = state.active_peer.read().await.clone();
    let mut life = state.lifecycle.lock().await;
    start_client_background_tasks(&state, &mut life, active_peer);

    Ok(SyncRuleDto::from(record))
}

/// 14. 删除指定增量同步规则并刷新后台实时目录监听器。
pub async fn delete_sync_rule(rule_id: String) -> Result<bool> {
    if rule_id.trim().is_empty() {
        bail!("rule_id must not be empty");
    }
    let state = runtime_state()?;
    let deleted = state.client_db.delete_sync_rule(rule_id.trim())?;
    let active_peer = state.active_peer.read().await.clone();
    let mut life = state.lifecycle.lock().await;
    start_client_background_tasks(&state, &mut life, active_peer);
    Ok(deleted)
}


/// 15. 开启或关闭全局 Debug 调试与微秒级性能探针模式。
pub async fn set_debug_mode(enabled: bool) -> Result<bool> {
    let state = runtime_state()?;
    lanvault_core::set_debug_mode(enabled);
    state
        .client_db
        .set_setting(SETTING_DEBUG_MODE, if enabled { "1" } else { "0" })?;
    Ok(lanvault_core::is_debug_enabled())
}

/// 16. 查询当前是否开启 Debug 模式。
pub async fn get_debug_mode() -> Result<bool> {
    let _ = runtime_state()?;
    Ok(lanvault_core::is_debug_enabled())
}

/// 17. 拉取统一调试与性能探针日志（含 Rust 原生错误/Panic 与前端上报错误）。
pub async fn get_debug_logs(limit: u32) -> Result<Vec<lanvault_core::DebugLogEntry>> {
    let _ = runtime_state()?;
    Ok(lanvault_core::get_debug_logs(limit as usize))
}

/// 18. 清空内存日志环形缓冲。
pub async fn clear_debug_logs() -> Result<()> {
    let _ = runtime_state()?;
    lanvault_core::clear_debug_logs();
    Ok(())
}

/// 19. 接收 Flutter / Web 前端原生未捕获异常与诊断日志。
pub async fn report_client_log(
    level: String,
    source: String,
    message: String,
    detail: Option<String>,
) -> Result<()> {
    let _ = runtime_state()?;
    lanvault_core::record_frontend_log(&level, &source, message, detail);
    Ok(())
}

/// 为 Android Level 1/2 守护进程生成并持久化高熵随机管理密码（取代弱默认密码兜底）。
#[cfg(target_os = "android")]
pub(crate) fn load_or_generate_jni_web_password(daemon_mode: bool) -> Result<String> {
    const KEY: &str = "jni_generated_web_password";
    const AAD: &[u8] = b"lanvault.local.jni-web-password.v1";
    let state = runtime_state_with_mode(daemon_mode)?;
    let protection_key = state
        .client_identity
        .derive_local_storage_key("client-state");
    if let Some(raw) = state.client_db.get_setting(KEY)? {
        if raw.trim_start().starts_with('{') {
            let decrypted = decrypt_setting(&raw, &protection_key, AAD)?;
            let existing = String::from_utf8(decrypted)
                .context("stored JNI web password is not valid UTF-8")?;
            if existing.len() >= 12 {
                return Ok(existing);
            }
        } else if raw.len() >= 12 {
            state.client_db.set_setting(
                KEY,
                &encrypt_setting(raw.as_bytes(), &protection_key, AAD)?,
            )?;
            return Ok(raw);
        }
    }
    let key = SymmetricKey::generate();
    let generated = hex::encode(&key.as_bytes()[..12]);
    state.client_db.set_setting(
        KEY,
        &encrypt_setting(generated.as_bytes(), &protection_key, AAD)?,
    )?;
    Ok(generated)
}


/// FFI DTO: 已配对的可信设备（供快传下拉选择）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedPeerDto {
    pub device_id: String,
    pub device_name: String,
    pub os_type: String,
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub online: bool,
    pub last_seen_secs: Option<i64>,
}

/// FFI DTO: 主备份文件夹与备用备份文件夹配置（跨盘符冗余）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupStorageConfigDto {
    pub primary_dir: String,
    pub fallback_dir: Option<String>,
}

fn normalized_drive_key(raw_path: &str) -> Option<String> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(unc_rest) = trimmed
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| trimmed.strip_prefix(r"\\?\unc\"))
    {
        let mut parts = unc_rest.split('\\').filter(|p| !p.is_empty());
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) {
            return Some(format!(
                r"\\{}\{}",
                server.to_ascii_lowercase(),
                share.to_ascii_lowercase()
            ));
        }
    }

    let stripped = trimmed.strip_prefix(r"\\?\").unwrap_or(trimmed);
    let bytes = stripped.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Some(format!("{}:", (bytes[0] as char).to_ascii_uppercase()));
    }
    if stripped.starts_with(r"\\") {
        let rest = stripped.trim_start_matches('\\');
        let mut parts = rest.split('\\').filter(|p| !p.is_empty());
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) {
            return Some(format!(
                r"\\{}\{}",
                server.to_ascii_lowercase(),
                share.to_ascii_lowercase()
            ));
        }
    }
    if stripped.starts_with('/') {
        let parts: Vec<&str> = stripped.split('/').filter(|p| !p.is_empty()).collect();
        return match parts.as_slice() {
            [] => Some("/".to_string()),
            ["storage", "emulated", user, ..] => Some(format!("/storage/emulated/{user}")),
            ["storage", vol, ..] => Some(format!("/storage/{vol}")),
            [mount_root @ ("mnt" | "media" | "Volumes"), vol, ..] => {
                Some(format!("/{mount_root}/{vol}"))
            }
            [first, ..] => Some(format!("/{first}")),
        };
    }
    None
}

/// 20. 列出已配对设备（优先展示持久化可信列表中的真实设备名与系统类型，并合并去重当前活跃会话）。
pub async fn list_paired_peers() -> Result<Vec<PairedPeerDto>> {
    let state = runtime_state()?;
    let active = state.active_peer.read().await.clone();
    let trusted_list = state.client_db.list_trusted_peers()?;

    let mut online_ids: HashSet<String> = HashSet::new();
    let mut last_seen_map: HashMap<String, i64> = HashMap::new();
    {
        let peers_cache = state
            .discovered_peers
            .lock()
            .map_err(|_| anyhow!("internal lock poisoned"))?;
        for (id, cached) in peers_cache.iter() {
            let elapsed = cached.last_seen.elapsed().as_secs() as i64;
            last_seen_map.insert(id.clone(), elapsed);
            if elapsed < 30 {
                online_ids.insert(id.clone());
            }
        }
    }
    if let Some((_, true, port)) = resolve_local_server_db_path(&state).await? {
        if let Ok(live) = fetch_local_server_peers_live(&state, port).await {
            for entry in live.peers {
                if entry.online {
                    online_ids.insert(entry.device_id);
                }
            }
        }
    }

    let mut peers = Vec::with_capacity(trusted_list.len() + usize::from(active.is_some()));
    let mut seen = HashSet::new();

    for trusted in trusted_list {
        let is_active = active
            .as_ref()
            .is_some_and(|a| a.device_id == trusted.device_id);
        let (ip, port) = match active.as_ref() {
            Some(a) if a.device_id == trusted.device_id => {
                (Some(a.addr.ip().to_string()), Some(a.addr.port()))
            }
            _ => (trusted.last_ip, trusted.last_port),
        };
        let online = is_active || online_ids.contains(&trusted.device_id);
        let last_seen_secs = if online {
            Some(0)
        } else {
            last_seen_map.get(&trusted.device_id).copied()
        };
        seen.insert(trusted.device_id.clone());
        peers.push(PairedPeerDto {
            device_id: trusted.device_id,
            device_name: trusted.device_name,
            os_type: trusted.os_type,
            ip,
            port,
            online,
            last_seen_secs,
        });
    }

    if let Some(active) = active {
        if seen.insert(active.device_id.clone()) {
            peers.push(PairedPeerDto {
                device_id: active.device_id.clone(),
                device_name: active.device_id,
                os_type: "peer".to_string(),
                ip: Some(active.addr.ip().to_string()),
                port: Some(active.addr.port()),
                online: true,
                last_seen_secs: Some(0),
            });
        }
    }

    Ok(peers)
}

/// 21. 读取当前持久化的省电与网络流控策略。
pub async fn get_power_network_policy() -> Result<PowerNetworkPolicy> {
    let state = runtime_state()?;
    let policy = state.policy.read().await.clone();
    Ok(policy)
}

/// 22. 读取当前是否开启断线自动重连。
pub async fn get_auto_reconnect() -> Result<bool> {
    let state = runtime_state()?;
    Ok(state.reconnect.is_enabled())
}

/// 23. 读取主备份文件夹与备用备份文件夹配置。
pub async fn get_backup_storage_config() -> Result<BackupStorageConfigDto> {
    let state = runtime_state()?;
    if let Some(raw) = state.client_db.get_setting(SETTING_BACKUP_STORAGE_JSON)? {
        let cfg: BackupStorageConfigDto = serde_json::from_str(&raw)
            .context("invalid persisted backup_storage_config_json")?;
        return Ok(cfg);
    }
    Ok(BackupStorageConfigDto {
        primary_dir: String::new(),
        fallback_dir: None,
    })
}

/// 24. 保存主备份文件夹与备用备份文件夹配置，强制校验两者不能位于同一盘符。
pub async fn set_backup_storage_config(
    primary_dir: String,
    fallback_dir: Option<String>,
) -> Result<BackupStorageConfigDto> {
    let primary = primary_dir.trim().to_string();
    if primary.is_empty() {
        bail!("主备份文件夹路径不能为空");
    }
    let fallback = fallback_dir
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());

    if let Some(ref fb) = fallback {
        if let (Some(p_drive), Some(f_drive)) = (
            normalized_drive_key(&primary),
            normalized_drive_key(fb),
        ) {
            if p_drive == f_drive {
                bail!(
                    "主备份文件夹与备用备份文件夹不能位于同一盘符（当前同为 {p_drive}），主盘空间不足时备用盘将同样无剩余空间"
                );
            }
        }
    }

    let state = runtime_state()?;
    let cfg = BackupStorageConfigDto {
        primary_dir: primary,
        fallback_dir: fallback,
    };
    let json = serde_json::to_string(&cfg)?;
    state
        .client_db
        .set_setting(SETTING_BACKUP_STORAGE_JSON, &json)?;
    Ok(cfg)
}


/// FFI DTO: 最近一次成功启动的本地服务端配置（不含明文密码）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastServerConfigDto {
    pub vault_path: String,
    pub quota_gb: u64,
    pub port: u16,
}

/// 25. 读取最近一次成功启动的本地服务端配置（Windows 与 Android C-ABI 共用，不含明文密码）。
pub async fn get_last_server_config() -> Result<Option<LastServerConfigDto>> {
    let state = runtime_state()?;
    if let Some(raw) = state.client_db.get_setting(SETTING_LAST_SERVER_CONFIG_JSON)? {
        let cfg: LastServerConfigDto = serde_json::from_str(&raw)
            .context("invalid persisted last_server_config_json")?;
        return Ok(Some(cfg));
    }
    let life = state.lifecycle.lock().await;
    Ok(life.last_server_config.as_ref().map(|cfg| LastServerConfigDto {
        vault_path: cfg.vault_root.to_string_lossy().into_owned(),
        quota_gb: (cfg.default_quota_bytes / (1024 * 1024 * 1024)).max(1),
        port: cfg.port,
    }))
}

/// FFI DTO: 内置探针自检诊断报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfDiagnosticsReport {
    pub version: String,
    pub device_id: String,
    pub role_mode: String,
    pub debug_enabled: bool,
    pub log_file_path: Option<String>,
    pub auto_reconnect: bool,
    pub sync_rule_count: usize,
    pub paired_peer_count: usize,
    pub backup_primary_dir: String,
    pub backup_fallback_dir: Option<String>,
    pub server_running: bool,
    pub server_port: Option<u16>,
    pub checks_passed: bool,
    pub issues: Vec<String>,
}

/// 26. 获取当前磁盘 Debug 日志文件路径。
pub async fn get_debug_log_path() -> Result<Option<String>> {
    let _ = runtime_state()?;
    Ok(lanvault_core::get_debug_log_path())
}

/// 27. 运行内置全链路探针自检（校验数据目录可写性与主/备备份目录有效性，供 UI 与自动化脚本调用）。
pub async fn run_self_diagnostics() -> Result<SelfDiagnosticsReport> {
    let _probe = lanvault_core::ProbeGuard::start(
        "rust.debug",
        "debug.self_diagnostics",
        None,
    );
    let state = runtime_state()?;
    let life = state.lifecycle.lock().await;
    let role_mode = life.role_mode.as_str().to_string();
    let (server_running, server_port) = match life.server_handle.as_ref() {
        Some(h) if !h.task.is_finished() => (true, Some(h.port)),
        _ => (false, None),
    };
    drop(life);

    let rules = state.client_db.list_sync_rules()?;
    let peers = list_paired_peers().await?;
    let storage = get_backup_storage_config().await?;
    let auto_reconnect = state.reconnect.is_enabled();
    let log_file_path = lanvault_core::get_debug_log_path();
    let debug_enabled = lanvault_core::is_debug_enabled();

    let mut issues = Vec::new();
    let data_dir = client_data_dir();
    let probe_path = data_dir.join(format!(".lanvault_diag_probe_{}", std::process::id()));
    match fs::write(&probe_path, b"ok") {
        Ok(()) => {
            if let Err(err) = fs::remove_file(&probe_path) {
                issues.push(format!(
                    "数据目录探测文件清理失败 ({}): {err}",
                    probe_path.display()
                ));
            }
        }
        Err(err) => {
            issues.push(format!("数据目录不可写 ({}): {err}", data_dir.display()));
        }
    }

    if !storage.primary_dir.is_empty() && !Path::new(&storage.primary_dir).is_dir() {
        issues.push(format!("主备份目录不存在或非目录: {}", storage.primary_dir));
    }
    if let Some(ref fb) = storage.fallback_dir {
        if !Path::new(fb).is_dir() {
            issues.push(format!("备用备份目录不存在或非目录: {fb}"));
        } else if !storage.primary_dir.is_empty() {
            if let (Some(p_drive), Some(f_drive)) = (
                normalized_drive_key(&storage.primary_dir),
                normalized_drive_key(fb),
            ) {
                if p_drive == f_drive {
                    issues.push(format!(
                        "主备份目录与备用备份目录位于同一盘符: {p_drive}"
                    ));
                }
            }
        }
    }

    let checks_passed = issues.is_empty();
    let report = SelfDiagnosticsReport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        device_id: state.client_device_id.clone(),
        role_mode,
        debug_enabled,
        log_file_path,
        auto_reconnect,
        sync_rule_count: rules.len(),
        paired_peer_count: peers.len(),
        backup_primary_dir: storage.primary_dir,
        backup_fallback_dir: storage.fallback_dir,
        server_running,
        server_port,
        checks_passed,
        issues,
    };

    lanvault_core::record_event_always(
        if report.checks_passed { "INFO" } else { "WARN" },
        "rust.debug",
        Some("debug.self_diagnostics"),
        None,
        format!(
            "self-diagnostics {}: version={} mode={} rules={} peers={} server_running={} issues={}",
            if report.checks_passed { "passed" } else { "failed" },
            report.version,
            report.role_mode,
            report.sync_rule_count,
            report.paired_peer_count,
            report.server_running,
            report.issues.len()
        ),
        serde_json::to_string(&report).ok(),
    );

    Ok(report)
}
// =============================================================================
// WebUI 核心管理能力整合（Windows & Android 客户端原生 FFI 导出，28–36）
// =============================================================================

/// FFI DTO: 系统总览中的近期活动条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityItem {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub kind: String,
    pub timestamp: i64,
}

/// FFI DTO: 系统总览聚合快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemOverviewDto {
    pub server_running: bool,
    pub server_port: u16,
    pub lan_ip: String,
    pub pairing_pin: String,
    pub pairing_qr_uri: String,
    pub vault_used_bytes: u64,
    pub vault_quota_bytes: u64,
    pub vault_count: usize,
    pub paired_peer_count: usize,
    pub online_peer_count: usize,
    pub sync_rule_count: usize,
    pub bit_rot_health: String,
    pub corrupted_chunks: u64,
    pub dedup_ratio: f64,
    pub compression_ratio: f64,
    pub sync_paused: bool,
    pub bit_rot_auto_scrub: bool,
    pub recent_activities: Vec<ActivityItem>,
}

/// FFI DTO: 存储池下的物理卷信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultVolumeDto {
    pub volume_id: String,
    pub mount_path: String,
    pub volume_guid: Option<String>,
    pub role: String,
    pub is_online: bool,
}

/// FFI DTO: 存储池摘要信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultSummaryDto {
    pub vault_id: String,
    pub name: String,
    pub pool_mode: String,
    pub quota_bytes: u64,
    pub used_bytes: u64,
    pub e2ee_enabled: bool,
    pub volumes: Vec<VaultVolumeDto>,
}

/// FFI DTO: 历史快照摘要。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSummaryDto {
    pub snapshot_id: String,
    pub vault_id: String,
    pub device_id: String,
    pub retention_tier: String,
    pub file_count: u64,
    pub total_bytes: u64,
    pub created_at: i64,
}

/// FFI DTO: Bit-Rot 完整性巡检结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrubResultDto {
    pub scrub_id: String,
    pub checked_chunks: u64,
    pub corrupted_chunks: u64,
    pub healed_chunks: u64,
    pub status: String,
}

/// FFI DTO: 吊销已配对设备结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokePeerResultDto {
    pub revoked: bool,
    pub device_id: String,
}

/// FFI DTO: 本地服务端配对信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingInfoDto {
    pub server_running: bool,
    pub lan_ip: String,
    pub port: u16,
    pub pairing_pin: String,
    pub pairing_qr_uri: String,
}

/// FFI DTO: 切换同步规则启停状态结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToggleSyncRuleResultDto {
    pub rule_id: String,
    pub enabled: bool,
}

/// FFI DTO: 临时文件加密快传结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushClipboardFileResultDto {
    pub sent: bool,
    pub file_path: String,
}

#[derive(Deserialize)]
struct LocalServerPeerEntry {
    device_id: String,
    online: bool,
}

#[derive(Deserialize)]
struct LocalServerPeersResponse {
    peers: Vec<LocalServerPeerEntry>,
    pairing_pin: String,
    pairing_qr_uri: String,
}

fn detect_lan_ipv4() -> String {
    if let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        if sock.connect((Ipv4Addr::new(192, 168, 1, 1), 80)).is_ok() {
            if let Ok(SocketAddr::V4(v4)) = sock.local_addr() {
                return v4.ip().to_string();
            }
        }
    }
    Ipv4Addr::LOCALHOST.to_string()
}

fn extract_ip_from_qr_uri(uri: &str) -> Option<String> {
    let query = uri.split_once('?')?.1;
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            if key == "ip" && !value.trim().is_empty() {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

async fn resolve_local_server_db_path(
    state: &FfiRuntimeState,
) -> Result<Option<(PathBuf, bool, u16)>> {
    {
        let life = state.lifecycle.lock().await;
        if let Some(handle) = life.server_handle.as_ref() {
            if !handle.task.is_finished() {
                return Ok(Some((handle.db_path.clone(), true, handle.port)));
            }
        }
        if let Some(cfg) = life.last_server_config.as_ref() {
            return Ok(Some((cfg.vault_root.join("index.db"), false, cfg.port)));
        }
    }

    if let Some(raw) = state.client_db.get_setting(SETTING_LAST_SERVER_CONFIG_JSON)? {
        let cfg: LastServerConfigDto = serde_json::from_str(&raw)
            .context("invalid persisted last_server_config_json")?;
        return Ok(Some((
            PathBuf::from(cfg.vault_path).join("index.db"),
            false,
            cfg.port,
        )));
    }

    Ok(None)
}

async fn login_local_admin_cookie(port: u16, web_password: &str) -> Result<String> {
    let client = Client::builder().timeout(Duration::from_secs(5)).build()?;
    let url = format!("http://127.0.0.1:{port}/api/auth/login");
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "password": web_password }))
        .send()
        .await
        .with_context(|| format!("local server admin login request to {url} failed"))?;

    if !resp.status().is_success() {
        bail!(
            "local server admin login rejected with HTTP {}",
            resp.status().as_u16()
        );
    }

    for header in resp.headers().get_all(reqwest::header::SET_COOKIE) {
        if let Ok(raw) = header.to_str() {
            let first = raw.split(';').next().unwrap_or("");
            if let Some(token) = first.strip_prefix("lv_session=") {
                if !token.is_empty() {
                    return Ok(token.to_string());
                }
            }
        }
    }

    bail!("local server admin login response missing lv_session cookie")
}

async fn fetch_local_server_peers_live(
    state: &FfiRuntimeState,
    port: u16,
) -> Result<LocalServerPeersResponse> {
    let (cached_cookie, web_password) = {
        let life = state.lifecycle.lock().await;
        (
            life.local_admin_cookie.clone(),
            life.last_server_config
                .as_ref()
                .map(|cfg| cfg.web_password.clone()),
        )
    };

    let client = Client::builder().timeout(Duration::from_secs(5)).build()?;
    let url = format!("http://127.0.0.1:{port}/api/peers");

    if let Some(cookie) = cached_cookie {
        let resp = client
            .get(&url)
            .header("Cookie", format!("lv_session={cookie}"))
            .send()
            .await
            .with_context(|| format!("request to {url} failed"))?;
        if resp.status().is_success() {
            return resp
                .json::<LocalServerPeersResponse>()
                .await
                .context("invalid local server /api/peers JSON response");
        }
    }

    let password = web_password
        .ok_or_else(|| anyhow!("local server credentials unavailable for active server"))?;
    let fresh_cookie = login_local_admin_cookie(port, &password).await?;
    {
        let mut life = state.lifecycle.lock().await;
        life.local_admin_cookie = Some(fresh_cookie.clone());
    }

    let resp = client
        .get(&url)
        .header("Cookie", format!("lv_session={fresh_cookie}"))
        .send()
        .await
        .with_context(|| format!("request to {url} failed"))?;
    if !resp.status().is_success() {
        bail!(
            "local server /api/peers returned HTTP {}",
            resp.status().as_u16()
        );
    }

    resp.json::<LocalServerPeersResponse>()
        .await
        .context("invalid local server /api/peers JSON response")
}

fn build_vault_summary_dto(
    vault: lanvault_core::VaultRecord,
    volumes: Vec<lanvault_core::VaultVolumeRecord>,
) -> VaultSummaryDto {
    VaultSummaryDto {
        vault_id: vault.vault_id,
        name: vault.name,
        pool_mode: vault.pool_mode.as_str().to_string(),
        quota_bytes: vault.quota_bytes,
        used_bytes: vault.used_bytes,
        e2ee_enabled: vault.e2ee_enabled,
        volumes: volumes
            .into_iter()
            .map(|vol| VaultVolumeDto {
                volume_id: vol.volume_id,
                mount_path: vol.mount_path,
                volume_guid: vol.volume_guid,
                role: vol.role.as_str().to_string(),
                is_online: vol.is_online,
            })
            .collect(),
    }
}

/// 28. 获取系统总览聚合状态（本地服务、存储容量、设备配对、Bit-Rot 健康度与最近最多 10 条真实活动）。
pub async fn get_system_overview() -> Result<SystemOverviewDto> {
    let state = runtime_state()?;
    let server_loc = resolve_local_server_db_path(&state).await?;
    let (db_path_opt, server_running, server_port) = match server_loc {
        Some((path, running, port)) => (Some(path), running, port),
        None => (None, false, DEFAULT_SERVER_PORT),
    };

    let (lan_ip, pairing_pin, pairing_qr_uri, server_online_ids) = if server_running {
        let live = fetch_local_server_peers_live(&state, server_port).await?;
        let ip = extract_ip_from_qr_uri(&live.pairing_qr_uri)
            .unwrap_or_else(detect_lan_ipv4);
        let online_ids: HashSet<String> = live
            .peers
            .into_iter()
            .filter(|p| p.online)
            .map(|p| p.device_id)
            .collect();
        (ip, live.pairing_pin, live.pairing_qr_uri, online_ids)
    } else {
        (
            detect_lan_ipv4(),
            String::new(),
            String::new(),
            HashSet::new(),
        )
    };

    let mut vault_used_bytes = 0u64;
    let mut vault_quota_bytes = 0u64;
    let mut vault_count = 0usize;
    let mut bit_rot_health = "unknown".to_string();
    let mut corrupted_chunks = 0u64;
    let mut dedup_ratio = 0.0f64;
    let mut compression_ratio = 1.0f64;
    let mut recent_activities: Vec<ActivityItem> = Vec::new();
    let mut server_trusted_peers: Vec<TrustedPeer> = Vec::new();

    if let Some(ref db_path) = db_path_opt {
        if db_path.exists() {
            let db = Database::open(db_path)?;
            let vaults = db.list_vaults()?;
            vault_count = vaults.len();
            let mut any_volume_offline = false;
            for v in &vaults {
                vault_used_bytes = vault_used_bytes.saturating_add(v.used_bytes);
                vault_quota_bytes = vault_quota_bytes.saturating_add(v.quota_bytes);
                let vols = db.list_vault_volumes(&v.vault_id)?;
                if vols.iter().any(|vol| !vol.is_online) {
                    any_volume_offline = true;
                }
            }
            corrupted_chunks = db.count_corrupted_chunks()?;
            if let Ok((raw_sum, stored_sum, logical_sum)) = db.get_chunk_storage_stats() {
                if logical_sum > 0 && raw_sum <= logical_sum {
                    dedup_ratio = (logical_sum - raw_sum) as f64 / logical_sum as f64;
                }
                if stored_sum > 0 && raw_sum > 0 {
                    compression_ratio = raw_sum as f64 / stored_sum as f64;
                }
            }
            bit_rot_health = if corrupted_chunks > 0 {
                "corrupted".to_string()
            } else if any_volume_offline {
                "degraded".to_string()
            } else {
                "healthy".to_string()
            };

            let snapshots = db.list_snapshots(None)?;
            for snap in snapshots.into_iter().take(10) {
                recent_activities.push(ActivityItem {
                    id: format!("snap-{}", snap.snapshot_id),
                    title: format!("快照备份 ({})", snap.retention_tier.as_str()),
                    detail: format!(
                        "{} 个文件 · {} 字节 · 设备 {}",
                        snap.file_count, snap.total_bytes, snap.device_id
                    ),
                    kind: "backup".to_string(),
                    timestamp: snap.created_at,
                });
            }

            server_trusted_peers = db.list_trusted_peers()?;
        }
    }

    let client_trusted_peers = state.client_db.list_trusted_peers()?;
    let active_peer = state.active_peer.read().await.clone();
    let mut all_paired_ids: HashSet<String> = HashSet::new();
    let mut seen_peer_activity: HashSet<String> = HashSet::new();

    for peer in client_trusted_peers
        .into_iter()
        .chain(server_trusted_peers.into_iter())
    {
        all_paired_ids.insert(peer.device_id.clone());
        if seen_peer_activity.insert(peer.device_id.clone()) {
            recent_activities.push(ActivityItem {
                id: format!("peer-{}", peer.device_id),
                title: format!("设备已配对 · {}", peer.device_name),
                detail: format!(
                    "{} ({})",
                    peer.os_type,
                    peer.last_ip.as_deref().unwrap_or("LAN")
                ),
                kind: "device".to_string(),
                timestamp: peer.paired_at,
            });
        }
    }
    if let Some(ref active) = active_peer {
        all_paired_ids.insert(active.device_id.clone());
    }

    let mut online_ids = server_online_ids;
    {
        let peers_cache = state
            .discovered_peers
            .lock()
            .map_err(|_| anyhow!("internal lock poisoned"))?;
        for (id, peer) in peers_cache.iter() {
            if peer.last_seen.elapsed() < Duration::from_secs(30) && all_paired_ids.contains(id) {
                online_ids.insert(id.clone());
            }
        }
    }

    let sync_rule_count = state.client_db.list_sync_rules()?.len();

    for entry in lanvault_core::get_debug_logs(50) {
        let probe = entry.probe.as_deref().unwrap_or("");
        let (kind, title) = if probe == "scrub.completed" || entry.source == "server.scrub" {
            ("scrub", "Bit-Rot 完整性巡检")
        } else if probe == "clipboard.push_file"
            || probe == "ffi.push_clipboard_text"
            || entry.source == "rust.transfer"
        {
            ("transfer", "局域网加密快传")
        } else if probe == "ffi.pair_with_pin" || probe == "peer.revoked" {
            ("device", "设备信任变更")
        } else if entry.source == "client.sync" || probe == "sync.completed" {
            ("backup", "增量同步任务")
        } else {
            continue;
        };
        recent_activities.push(ActivityItem {
            id: format!("log-{}", entry.seq),
            title: title.to_string(),
            detail: entry.message,
            kind: kind.to_string(),
            timestamp: (entry.timestamp_ms / 1000) as i64,
        });
    }

    recent_activities.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    recent_activities.truncate(10);

    let sync_paused = state
        .client_db
        .get_setting(SETTING_SYNC_PAUSED)?
        .as_deref()
        == Some("1");
    let bit_rot_auto_scrub = state
        .client_db
        .get_setting(SETTING_BIT_ROT_AUTO_SCRUB)?
        .as_deref()
        != Some("0");

    Ok(SystemOverviewDto {
        server_running,
        server_port,
        lan_ip,
        pairing_pin,
        pairing_qr_uri,
        vault_used_bytes,
        vault_quota_bytes,
        vault_count,
        paired_peer_count: all_paired_ids.len(),
        online_peer_count: online_ids.len(),
        sync_rule_count,
        bit_rot_health,
        corrupted_chunks,
        dedup_ratio,
        compression_ratio,
        sync_paused,
        bit_rot_auto_scrub,
        recent_activities,
    })
}

/// 29. 列出本地服务端 `index.db` 中的全部存储池与物理卷；若本地尚未初始化服务端库则返回空列表。
pub async fn list_vaults() -> Result<Vec<VaultSummaryDto>> {
    let state = runtime_state()?;
    let Some((db_path, _, _)) = resolve_local_server_db_path(&state).await? else {
        return Ok(Vec::new());
    };
    if !db_path.exists() {
        return Ok(Vec::new());
    }

    tokio::task::spawn_blocking(move || -> Result<Vec<VaultSummaryDto>> {
        let db = Database::open(&db_path)?;
        let vaults = db.list_vaults()?;
        let mut result = Vec::with_capacity(vaults.len());
        for v in vaults {
            let volumes = db.list_vault_volumes(&v.vault_id)?;
            result.push(build_vault_summary_dto(v, volumes));
        }
        Ok(result)
    })
    .await
    .context("list_vaults worker panicked")?
}

/// 30. 在本地服务端 `index.db` / `VaultStore` 中创建新的存储池并返回新建的 `VaultSummaryDto`。
pub async fn create_vault(
    name: String,
    mount_path: String,
    pool_mode: String,
    quota_gb: u64,
    e2ee_enabled: bool,
    extra_mount_path: Option<String>,
) -> Result<VaultSummaryDto> {
    let trimmed_name = name.trim().to_string();
    if trimmed_name.is_empty() {
        bail!("vault name must not be empty");
    }
    let trimmed_mount = mount_path.trim().to_string();
    if trimmed_mount.is_empty() {
        bail!("mount_path must not be empty");
    }
    if quota_gb == 0 {
        bail!("quota_gb must be greater than 0");
    }
    let parsed_mode = PoolMode::parse(pool_mode.trim())?;
    let quota_bytes = quota_gb.saturating_mul(1024 * 1024 * 1024);

    let extra_volumes = match extra_mount_path
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
    {
        Some(extra_path) => {
            let role = match parsed_mode {
                PoolMode::MirrorRaid1 => VolumeRole::Mirror,
                PoolMode::Striped => VolumeRole::PoolExt,
                PoolMode::Single => {
                    bail!("single pool mode does not accept extra_mount_path");
                }
            };
            vec![ExtraVolumeSpec {
                mount_path: extra_path,
                volume_guid: None,
                role,
            }]
        }
        None => Vec::new(),
    };

    let state = runtime_state()?;
    let (db_path, _, _) = resolve_local_server_db_path(&state)
        .await?
        .ok_or_else(|| anyhow!("local server database is not initialized; start local server first"))?;

    let db = Arc::new(Database::open(&db_path)?);
    let store = VaultStore::new(db);
    let (vault, volumes) = store.create_vault(
        trimmed_name,
        trimmed_mount,
        None,
        parsed_mode,
        quota_bytes,
        e2ee_enabled,
        extra_volumes,
    )?;

    Ok(build_vault_summary_dto(vault, volumes))
}

/// 31. 列出本地服务端历史快照（按 `created_at` 倒序，可按 `vault_id` 过滤）。
pub async fn list_snapshots(vault_id: Option<String>) -> Result<Vec<SnapshotSummaryDto>> {
    let state = runtime_state()?;
    let Some((db_path, _, _)) = resolve_local_server_db_path(&state).await? else {
        return Ok(Vec::new());
    };
    if !db_path.exists() {
        return Ok(Vec::new());
    }

    let filter = vault_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    tokio::task::spawn_blocking(move || -> Result<Vec<SnapshotSummaryDto>> {
        let db = Database::open(&db_path)?;
        let snapshots = db.list_snapshots(filter.as_deref())?;
        Ok(snapshots
            .into_iter()
            .map(|s| SnapshotSummaryDto {
                snapshot_id: s.snapshot_id,
                vault_id: s.vault_id,
                device_id: s.device_id,
                retention_tier: s.retention_tier.as_str().to_string(),
                file_count: s.file_count,
                total_bytes: s.total_bytes,
                created_at: s.created_at,
            })
            .collect())
    })
    .await
    .context("list_snapshots worker panicked")?
}

/// 32. 在本地服务端全部存储库上执行真实 `BitRotScrubber` 完整性巡检并返回聚合报告。
pub async fn trigger_bit_rot_scrub() -> Result<ScrubResultDto> {
    let state = runtime_state()?;
    let (db_path, _, _) = resolve_local_server_db_path(&state)
        .await?
        .ok_or_else(|| anyhow!("local server database is not initialized"))?;
    if !db_path.exists() {
        bail!(
            "local server database does not exist at {}",
            db_path.display()
        );
    }

    let result = tokio::task::spawn_blocking(move || -> Result<ScrubResultDto> {
        let db = Arc::new(Database::open(&db_path)?);
        let vaults = db.list_vaults()?;
        if vaults.is_empty() {
            bail!("no vaults found in local server database");
        }

        let vault_store = Arc::new(VaultStore::new(Arc::clone(&db)));
        let (alert_tx, _) = broadcast::channel::<BitRotAlertEvent>(64);
        let scrubber = BitRotScrubber::new(Arc::clone(&db), vault_store, alert_tx);

        let mut scrub_id = String::new();
        let mut checked_chunks = 0u64;
        let mut corrupted_chunks = 0u64;
        let mut healed_chunks = 0u64;
        let mut status = "completed".to_string();

        for vault in vaults {
            let report = scrubber.run_scrub(&vault.vault_id)?;
            if scrub_id.is_empty() {
                scrub_id = report.scrub_id;
            }
            checked_chunks = checked_chunks.saturating_add(report.checked_chunks);
            corrupted_chunks = corrupted_chunks.saturating_add(report.corrupted_chunks);
            healed_chunks = healed_chunks.saturating_add(report.healed_chunks);
            status = report.status;
        }

        Ok(ScrubResultDto {
            scrub_id,
            checked_chunks,
            corrupted_chunks,
            healed_chunks,
            status,
        })
    })
    .await
    .context("bit-rot scrub worker task panicked")??;

    lanvault_core::record_event_always(
        "INFO",
        "rust.scrub",
        Some("scrub.completed"),
        None,
        format!(
            "bit-rot scrub completed: checked={}, corrupted={}, healed={}",
            result.checked_chunks, result.corrupted_chunks, result.healed_chunks
        ),
        None,
    );

    Ok(result)
}

/// 33. 吊销已配对设备（同步清除活跃会话与持久化项，并在本地服务端库存在该设备时同步吊销）。
pub async fn revoke_paired_peer(device_id: String) -> Result<RevokePeerResultDto> {
    let target_id = device_id.trim().to_string();
    if target_id.is_empty() {
        bail!("device_id must not be empty");
    }

    let state = runtime_state()?;
    let client_revoked = state.client_db.delete_trusted_peer(&target_id)?;

    let cleared_active = {
        let mut active_guard = state.active_peer.write().await;
        if active_guard
            .as_ref()
            .is_some_and(|peer| peer.device_id == target_id)
        {
            *active_guard = None;
            true
        } else {
            false
        }
    };

    if cleared_active {
        state.client_db.set_setting(SETTING_ACTIVE_PEER_JSON, "")?;
        let mut life = state.lifecycle.lock().await;
        start_client_background_tasks(&state, &mut life, None);
    }

    let mut server_revoked = false;
    if let Some((db_path, _, _)) = resolve_local_server_db_path(&state).await? {
        if db_path.exists() {
            let server_db = Database::open(&db_path)?;
            server_revoked = server_db.delete_trusted_peer(&target_id)?;
        }
    }

    if !client_revoked && !cleared_active && !server_revoked {
        bail!("paired peer `{target_id}` not found");
    }

    lanvault_core::record_event_always(
        "INFO",
        "rust.peer",
        Some("peer.revoked"),
        None,
        format!("revoked paired peer {target_id}"),
        None,
    );

    Ok(RevokePeerResultDto {
        revoked: true,
        device_id: target_id,
    })
}

/// 34. 获取本地服务端真实配对信息（运行状态、局域网 IP、端口、动态 PIN 与配对二维码 URI）。
pub async fn get_pairing_info() -> Result<PairingInfoDto> {
    let state = runtime_state()?;
    let server_loc = resolve_local_server_db_path(&state).await?;
    let (server_running, port) = match server_loc {
        Some((_, running, port)) => (running, port),
        None => (false, DEFAULT_SERVER_PORT),
    };

    if !server_running {
        return Ok(PairingInfoDto {
            server_running: false,
            lan_ip: detect_lan_ipv4(),
            port,
            pairing_pin: String::new(),
            pairing_qr_uri: String::new(),
        });
    }

    let live = fetch_local_server_peers_live(&state, port).await?;
    let lan_ip = extract_ip_from_qr_uri(&live.pairing_qr_uri).unwrap_or_else(detect_lan_ipv4);

    Ok(PairingInfoDto {
        server_running: true,
        lan_ip,
        port,
        pairing_pin: live.pairing_pin,
        pairing_qr_uri: live.pairing_qr_uri,
    })
}

/// 35. 切换指定同步规则的 `enabled` 状态并同步刷新本地目录监听器。
pub async fn toggle_sync_rule(rule_id: String, enabled: bool) -> Result<ToggleSyncRuleResultDto> {
    let trimmed_id = rule_id.trim();
    if trimmed_id.is_empty() {
        bail!("rule_id must not be empty");
    }

    let state = runtime_state()?;
    let mut rule = state
        .client_db
        .get_sync_rule(trimmed_id)?
        .ok_or_else(|| anyhow!("sync rule `{trimmed_id}` not found"))?;
    rule.enabled = enabled;
    state.client_db.upsert_sync_rule(&rule)?;

    let active_peer = state.active_peer.read().await.clone();
    let mut life = state.lifecycle.lock().await;
    start_client_background_tasks(&state, &mut life, active_peer);

    Ok(ToggleSyncRuleResultDto {
        rule_id: rule.rule_id,
        enabled: rule.enabled,
    })
}

/// 36. 通过加密信道将指定本地文件快传至已配对设备。
pub async fn push_clipboard_file(
    target_device_id: Option<String>,
    file_path: String,
) -> Result<PushClipboardFileResultDto> {
    let trimmed_path = file_path.trim().to_string();
    if trimmed_path.is_empty() {
        bail!("file_path must not be empty");
    }

    let _probe = lanvault_core::ProbeGuard::start(
        "rust.ffi",
        "ffi.push_clipboard_file",
        Some(format!(
            "target={}",
            target_device_id.as_deref().unwrap_or("broadcast")
        )),
    );

    let state = runtime_state()?;
    let peer = state
        .active_peer
        .read()
        .await
        .clone()
        .ok_or_else(|| anyhow!("no paired peer session active for clipboard file push"))?;

    let client = ClipboardClient::new(
        peer.endpoint_base_url,
        peer.session_cookie,
        peer.channel_key,
    )?;
    let target_label = target_device_id
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| peer.device_id.clone());
    let resp = client
        .push_temp_file(target_device_id, Path::new(&trimmed_path))
        .await?;
    let file_size = tokio::fs::metadata(&trimmed_path).await.map(|m| m.len()).unwrap_or(0);
    let file_name = Path::new(&trimmed_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&trimmed_path)
        .to_string();
    let now_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let _ = state.client_db.insert_transfer_record(
        "sent",
        "file",
        &file_name,
        &format!("发送至 {target_label}"),
        &target_label,
        None,
        file_size,
        now_ts,
    );

    lanvault_core::record_event_always(
        "INFO",
        "rust.transfer",
        Some("clipboard.push_file"),
        None,
        format!(
            "pushed temp file {} to {} peer(s)",
            trimmed_path, resp.delivered_to
        ),
        None,
    );

    info!(
        delivered_to = resp.delivered_to,
        peer = %peer.device_id,
        file_path = %trimmed_path,
        "clipboard temp file pushed via encrypted channel"
    );

    Ok(PushClipboardFileResultDto {
        sent: true,
        file_path: trimmed_path,
    })
}


/// FFI DTO: 快照文件恢复结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreSnapshotResultDto {
    pub snapshot_id: String,
    pub restored_chunks: u64,
    pub restored_bytes: u64,
    pub output_path: String,
}

/// 37. 从本地存储池解密、校验并还原指定快照数据块至目标目录。
pub async fn restore_snapshot(
    snapshot_id: String,
    target_dir: String,
) -> Result<RestoreSnapshotResultDto> {
    let snap_id = snapshot_id.trim().to_string();
    let target_trim = target_dir.trim().to_string();
    if snap_id.is_empty() || target_trim.is_empty() {
        bail!("snapshot_id and target_dir must not be empty");
    }

    let state = runtime_state()?;
    let (db_path, _, _) = resolve_local_server_db_path(&state)
        .await?
        .ok_or_else(|| anyhow!("local server database is not initialized"))?;
    if !db_path.exists() {
        bail!("local server database does not exist at {}", db_path.display());
    }

    let vault_key = state.vault_master_key.read().await.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<RestoreSnapshotResultDto> {
        let db = Arc::new(Database::open(&db_path)?);
        let snap = db
            .get_snapshot(&snap_id)?
            .ok_or_else(|| anyhow!("snapshot `{snap_id}` not found"))?;
        let hashes = db.list_snapshot_chunks(&snap.snapshot_id)?;
        let vault_store = VaultStore::new(Arc::clone(&db));
        let chunk_key = vault_key.derive_vault_chunk_key(&snap.vault_id);

        let out_dir = PathBuf::from(&target_trim);
        std::fs::create_dir_all(&out_dir)
            .with_context(|| format!("failed to create restore directory {}", out_dir.display()))?;
        let out_file_path = out_dir.join(format!("restore_{}.bin", snap.snapshot_id));
        let mut out_file = std::fs::File::create(&out_file_path)
            .with_context(|| format!("failed to create restore output {}", out_file_path.display()))?;

        let mut restored_chunks = 0u64;
        let mut restored_bytes = 0u64;
        for hash in &hashes {
            let stored_bytes = vault_store.get_chunk(&snap.vault_id, hash)?;
            let plain = unpack_chunk(
                &stored_bytes,
                hash,
                Some(&chunk_key),
                snap.vault_id.as_bytes(),
                CHUNK_MAX_SIZE as usize,
            )?;
            std::io::Write::write_all(&mut out_file, &plain)?;
            restored_chunks += 1;
            restored_bytes += plain.len() as u64;
        }
        std::io::Write::flush(&mut out_file)?;

        Ok(RestoreSnapshotResultDto {
            snapshot_id: snap.snapshot_id,
            restored_chunks,
            restored_bytes,
            output_path: out_file_path.to_string_lossy().into_owned(),
        })
    })
    .await
    .context("snapshot restore worker panicked")??;

    lanvault_core::record_event_always(
        "INFO",
        "rust.restore",
        Some("snapshot.restored"),
        None,
        format!(
            "restored snapshot {} ({} chunks, {} bytes) -> {}",
            result.snapshot_id, result.restored_chunks, result.restored_bytes, result.output_path
        ),
        None,
    );

    Ok(result)
}

/// 38. 查询持久化传输历史（包含发送与接收记录）。
pub async fn list_transfer_history(limit: Option<usize>) -> Result<Vec<TransferHistoryRecord>> {
    let state = runtime_state()?;
    state
        .client_db
        .list_transfer_records(limit.unwrap_or(50))
        .map_err(Into::into)
}

/// 39. 全局暂停或恢复自动增量备份监听。
pub async fn set_sync_paused(paused: bool) -> Result<bool> {
    let state = runtime_state()?;
    state
        .client_db
        .set_setting(SETTING_SYNC_PAUSED, if paused { "1" } else { "0" })?;
    let active_peer = state.active_peer.read().await.clone();
    let mut life = state.lifecycle.lock().await;
    if paused {
        stop_client_background_tasks(&mut life);
    } else {
        start_client_background_tasks(&state, &mut life, active_peer);
    }
    Ok(paused)
}

/// 40. 查询当前是否处于全局暂停备份状态。
pub async fn get_sync_paused() -> Result<bool> {
    let state = runtime_state()?;
    Ok(state
        .client_db
        .get_setting(SETTING_SYNC_PAUSED)?
        .as_deref()
        == Some("1"))
}

/// 41. 设置是否启用 Bit-Rot 定时自动巡检。
pub async fn set_bit_rot_auto_scrub(enabled: bool) -> Result<bool> {
    let state = runtime_state()?;
    state
        .client_db
        .set_setting(SETTING_BIT_ROT_AUTO_SCRUB, if enabled { "1" } else { "0" })?;
    Ok(enabled)
}

/// 42. 查询 Bit-Rot 定时自动巡检开关状态。
pub async fn get_bit_rot_auto_scrub() -> Result<bool> {
    let state = runtime_state()?;
    Ok(state
        .client_db
        .get_setting(SETTING_BIT_ROT_AUTO_SCRUB)?
        .as_deref()
        != Some("0"))
}

/// 43. 设置指定同步规则的调度策略（`realtime` / `daily` / `manual`）。
pub async fn set_sync_rule_schedule(rule_id: String, schedule_mode: String) -> Result<String> {
    let trimmed_id = rule_id.trim();
    if trimmed_id.is_empty() {
        bail!("rule_id must not be empty");
    }
    let normalized = match schedule_mode.trim().to_lowercase().as_str() {
        "realtime" | "daily" | "manual" => schedule_mode.trim().to_lowercase(),
        other => bail!("unsupported schedule_mode `{other}`"),
    };
    let state = runtime_state()?;
    if state.client_db.get_sync_rule(trimmed_id)?.is_none() {
        bail!("sync rule `{trimmed_id}` not found");
    }
    state.client_db.set_setting(
        &format!("{SETTING_RULE_SCHEDULE_PREFIX}{trimmed_id}"),
        &normalized,
    )?;
    Ok(normalized)
}
