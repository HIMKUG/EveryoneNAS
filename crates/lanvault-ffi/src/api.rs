//! `flutter_rust_bridge` v2 FFI 暴露层，严格对齐 `ARCHITECTURE_CONTRACTS.md` 第 4 节全部函数及 DTO，
//! 并扩展统一三态运行模式（`client_only` / `server_only` / `dual`）控制接口。
//!
//! 生命周期与状态治理：
//! - 单一 `tokio::sync::Mutex<LifecycleState>` 串行化模式切换、启服与停服，彻底消除 TOCTOU 竞态。
//! - 独立维护用户数据目录 `client_state.db` (`Arc<lanvault_core::Database>`)，与本地服务端完全解耦，
//!   保障 `client_only` 模式下 `trigger_incremental_scan` 与 `update_power_network_policy` 独立可用。
//! - `RoleMode`、`DeviceIdentity` 与 `vault_master_key` 全部持久化，重启不漂移。

use std::collections::HashMap;
use std::fs;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, watch, Mutex as AsyncMutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use lanvault_client::{
    reconcile_directory, ClipboardClient, DirectoryWatcher, PowerNetworkPolicy,
    ReconnectController, ReconnectEvent, ReconnectTarget, SyncEngine, WatchEvent,
};
use lanvault_core::{
    parse_role_mode, Database, DeviceIdentity, DiscoveredPeerInfo, DiscoveryBrowser, RoleMode,
    SessionKeyExchange, SymmetricKey, SyncRuleRecord,
};
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
const PAIR_KEX_TRANSCRIPT: &[u8] = b"lanvault.pair.v1";

/// FFI DTO: mDNS / 局域网发现的对端设备。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredPeer {
    pub device_id: String,
    pub device_name: String,
    pub os_type: String,
    pub ip: String,
    pub port: u16,
    pub fingerprint: String,
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
        }
    }
}

struct LocalServerHandle {
    port: u16,
    vault_path: PathBuf,
    db_path: PathBuf,
    stop_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

/// 受单一异步互斥锁保护的三态模式与本地服务生命周期状态，杜绝并发竞态。
struct LifecycleState {
    role_mode: RoleMode,
    server_handle: Option<LocalServerHandle>,
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
}

impl From<SyncRuleRecord> for SyncRuleDto {
    fn from(r: SyncRuleRecord) -> Self {
        Self {
            rule_id: r.rule_id,
            local_dir: r.local_dir,
            target_vault_id: r.target_vault_id,
            enabled: r.enabled,
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

/// 全局 FFI 异步运行时与服务句柄容器。
struct FfiRuntimeState {
    runtime: tokio::runtime::Runtime,
    client_db: Arc<Database>,
    client_identity: DeviceIdentity,
    client_device_id: String,
    vault_master_key: SymmetricKey,
    lifecycle: AsyncMutex<LifecycleState>,
    reconnect: ReconnectController,
    policy: Arc<RwLock<PowerNetworkPolicy>>,
    progress_tx: broadcast::Sender<SyncProgressEvent>,
    latest_progress: Arc<Mutex<SyncProgressEvent>>,
    discovered_peers: Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    mdns_browser_started: Mutex<bool>,
    active_peer: Arc<RwLock<Option<ActivePeerSession>>>,
}

static GLOBAL_STATE: OnceLock<Arc<FfiRuntimeState>> = OnceLock::new();

fn client_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LANVAULT_DATA_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
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

fn load_or_generate_vault_master_key(db: &Database) -> Result<SymmetricKey> {
    if let Some(hex_key) = db.get_setting(SETTING_VAULT_MASTER_KEY_HEX)? {
        let bytes = hex::decode(hex_key.trim())
            .context("persisted vault_master_key_hex is not valid hex")?;
        return SymmetricKey::from_slice(&bytes)
            .context("persisted vault_master_key_hex has invalid length");
    }

    let key = SymmetricKey::generate();
    db.set_setting(SETTING_VAULT_MASTER_KEY_HEX, &hex::encode(key.as_bytes()))?;
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
    if !db.list_sync_rules()?.is_empty() {
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
    Ok(())
}

fn load_persisted_peer(db: &Database) -> Result<Option<ActivePeerSession>> {
    let Some(raw) = db.get_setting(SETTING_ACTIVE_PEER_JSON)? else {
        return Ok(None);
    };
    let persisted: PersistedPeerSession =
        serde_json::from_str(&raw).context("invalid persisted active_peer_session_json")?;
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

fn save_persisted_peer(db: &Database, peer: &ActivePeerSession) -> Result<()> {
    let persisted = PersistedPeerSession {
        device_id: peer.device_id.clone(),
        addr: peer.addr.to_string(),
        endpoint_base_url: peer.endpoint_base_url.clone(),
        session_cookie: peer.session_cookie.clone(),
        channel_key_hex: hex::encode(peer.channel_key.as_bytes()),
        default_vault_id: peer.default_vault_id.clone(),
    };
    let json = serde_json::to_string(&persisted)?;
    db.set_setting(SETTING_ACTIVE_PEER_JSON, &json)?;
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
        life.reconnect_event_task = Some(state.runtime.spawn(async move {
            while let Ok(ev) = rx.recv().await {
                if matches!(ev, ReconnectEvent::PeerConnected { .. }) {
                    if let Ok(rules) = db.list_sync_rules() {
                        for rule in rules.into_iter().filter(|r| r.enabled) {
                            if let Err(err) = trigger_incremental_scan(rule.rule_id.clone()).await {
                                warn!(rule_id = %rule.rule_id, "reconnect reconciliation failed: {err}");
                            }
                        }
                    }
                }
            }
        }));
    }
}

fn runtime_state() -> Result<Arc<FfiRuntimeState>> {
    if let Some(state) = GLOBAL_STATE.get() {
        return Ok(Arc::clone(state));
    }

    let data_dir = client_data_dir();
    fs::create_dir_all(&data_dir).with_context(|| {
        format!(
            "failed to create LanVault client data directory at {}",
            data_dir.display()
        )
    })?;

    lanvault_core::install_rust_panic_hook();
    lanvault_core::configure_debug_log_file(&data_dir.join("lanvault_debug.log"));

    let client_db = Arc::new(
        Database::open(data_dir.join("client_state.db"))
            .context("failed to open client_state.db")?,
    );
    let client_identity = DeviceIdentity::load_or_generate(&data_dir.join("client_device.key"))
        .context("failed to load or generate client DeviceIdentity")?;
    let client_device_id = format!("lv-{}", &client_identity.fingerprint_hex()[..12]);
    let vault_master_key = load_or_generate_vault_master_key(&client_db)?;
    let persisted_role = load_persisted_role_mode(&client_db)?;
    let persisted_policy = load_persisted_policy(&client_db)?;
    ensure_default_sync_rules(&client_db, &persisted_policy)?;
    let persisted_peer = load_persisted_peer(&client_db)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("lanvault-ffi-rt")
        .build()
        .context("failed to initialize global Tokio runtime for LanVault FFI")?;

    let (progress_tx, _) = broadcast::channel(256);
    let reconnect_enabled = persisted_role != RoleMode::ServerOnly;
    let state = Arc::new(FfiRuntimeState {
        runtime,
        client_db,
        client_identity,
        client_device_id,
        vault_master_key,
        lifecycle: AsyncMutex::new(LifecycleState {
            role_mode: persisted_role,
            server_handle: None,
            watchers: Vec::new(),
            watcher_dispatch_task: None,
            reconnect_task: None,
            reconnect_event_task: None,
        }),
        reconnect: ReconnectController::new(reconnect_enabled),
        policy: Arc::new(RwLock::new(persisted_policy)),
        progress_tx,
        latest_progress: Arc::new(Mutex::new(SyncProgressEvent::default())),
        discovered_peers: Arc::new(Mutex::new(HashMap::new())),
        mdns_browser_started: Mutex::new(false),
        active_peer: Arc::new(RwLock::new(persisted_peer.clone())),
    });

    if persisted_role != RoleMode::ServerOnly {
        let mut life = state.lifecycle.blocking_lock();
        start_client_background_tasks(&state, &mut life, persisted_peer);
    }

    let _ = GLOBAL_STATE.set(Arc::clone(&state));
    Ok(Arc::clone(
        GLOBAL_STATE.get().expect("GLOBAL_STATE initialized"),
    ))
}

/// 供 `c_abi` 与 Android JNI 同步桥接复用的全局 Tokio 执行入口。
pub(crate) fn block_on<F: Future>(future: F) -> Result<F::Output> {
    let state = runtime_state()?;
    Ok(state.runtime.block_on(future))
}

fn emit_progress(state: &FfiRuntimeState, event: SyncProgressEvent) {
    *state
        .latest_progress
        .lock()
        .expect("latest_progress mutex poisoned") = event.clone();
    let _ = state.progress_tx.send(event);
}

async fn stop_local_server_gracefully(lifecycle: &mut LifecycleState) -> Result<()> {
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
    let snapshot = state
        .latest_progress
        .lock()
        .expect("latest_progress mutex poisoned")
        .clone();
    Ok(snapshot)
}

/// 供 `c_abi` 启动（幂等）后台 mDNS + UDP 7843 双通道发现并返回当前已发现的合法对端列表。
pub(crate) fn poll_mdns_peers() -> Result<Vec<DiscoveredPeer>> {
    let state = runtime_state()?;
    let should_start = {
        let mut started = state
            .mdns_browser_started
            .lock()
            .expect("mdns_browser_started mutex poisoned");
        if *started {
            false
        } else {
            *started = true;
            true
        }
    };

    if should_start {
        let (tx, mut rx) = mpsc::channel::<DiscoveredPeerInfo>(64);
        let browser = DiscoveryBrowser::start(state.client_device_id.clone(), tx)
            .context("failed to start dual-channel DiscoveryBrowser")?;
        let peers_cache = Arc::clone(&state.discovered_peers);

        state.runtime.spawn(async move {
            let _browser_guard = browser;
            while let Some(info) = rx.recv().await {
                let peer: DiscoveredPeer = info.into();
                peers_cache
                    .lock()
                    .expect("discovered_peers mutex poisoned")
                    .insert(peer.device_id.clone(), peer);
            }
        });
    }

    let peers = state
        .discovered_peers
        .lock()
        .expect("discovered_peers mutex poisoned")
        .values()
        .cloned()
        .collect();
    Ok(peers)
}

/// 1. 启动 mDNS + UDP 7843 局域网双通道设备发现（自动过滤本机）并通过 `StreamSink` 流式推送至 Flutter。
pub fn start_mdns_scan(sink: StreamSink<DiscoveredPeer>) -> Result<()> {
    let state = runtime_state()?;
    let (tx, mut rx) = mpsc::channel::<DiscoveredPeerInfo>(64);
    let browser = DiscoveryBrowser::start(state.client_device_id.clone(), tx)
        .context("failed to start dual-channel DiscoveryBrowser")?;
    let peers_cache = Arc::clone(&state.discovered_peers);

    state.runtime.spawn(async move {
        while let Some(info) = rx.recv().await {
            let peer: DiscoveredPeer = info.into();
            peers_cache
                .lock()
                .expect("discovered_peers mutex poisoned")
                .insert(peer.device_id.clone(), peer.clone());

            if sink.add(peer).is_err() {
                info!("Dart discovery StreamSink closed; stopping DiscoveryBrowser");
                break;
            }
        }
        let _ = browser.shutdown();
    });

    Ok(())
}

#[derive(Serialize)]
struct PairPinRequest {
    pin: String,
    client_ed25519_pubkey_hex: String,
    client_x25519_pubkey_hex: String,
    device_name: String,
    os_type: String,
}

#[derive(Deserialize)]
struct PairPinResponse {
    device_id: String,
    server_x25519_pubkey_hex: String,
    session_cookie: Option<String>,
}

/// 2. 使用 6 位动态 PIN 码执行 X25519 + Ed25519 零信任配对，返回对端 `device_id`。
pub async fn pair_with_pin(ip: String, port: u16, pin: String) -> Result<String> {
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

    let socket_addr = SocketAddr::new(parsed_ip, port);
    let endpoint_base_url = format!("http://{socket_addr}");
    let url = format!("{endpoint_base_url}/api/peers/pair");

    let client = Client::builder().timeout(Duration::from_secs(10)).build()?;
    let resp = client
        .post(&url)
        .json(&PairPinRequest {
            pin,
            client_ed25519_pubkey_hex: hex::encode(state.client_identity.public_key_bytes()),
            client_x25519_pubkey_hex: hex::encode(kex.public_key_bytes()),
            device_name: current_hostname(),
            os_type: std::env::consts::OS.to_string(),
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
    let peer_pub_bytes = hex::decode(&body.server_x25519_pubkey_hex)
        .context("invalid server_x25519_pubkey_hex")?;
    let channel_key = kex
        .agree(&peer_pub_bytes, PAIR_KEX_TRANSCRIPT)
        .context("X25519 session key agreement failed")?;

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
    save_persisted_peer(&state.client_db, &session)?;
    *state.active_peer.write().await = Some(session.clone());

    let mut life = state.lifecycle.lock().await;
    start_client_background_tasks(&state, &mut life, Some(session));

    info!(peer_device_id = %body.device_id, addr = %socket_addr, "paired with peer via PIN");
    Ok(body.device_id)
}

/// 3. 开启或暂停后台自愈重连状态机（`2s -> 5s -> 15s -> 30s`）。
pub async fn enable_auto_reconnect(enabled: bool) -> Result<()> {
    let state = runtime_state()?;
    let life = state.lifecycle.lock().await;
    if life.role_mode == RoleMode::ServerOnly {
        bail!(
            "auto-reconnect is unavailable in `server_only` mode; switch role_mode to `client_only` or `dual` first"
        );
    }
    state.reconnect.set_enabled(enabled);
    Ok(())
}

/// 内部复用入口：支持 Android JNI 透传指定监听端口，同时被 `start_local_server` 复用。
pub(crate) async fn start_local_server_with_port(
    vault_path: String,
    quota_gb: u64,
    web_password: String,
    requested_port: Option<u16>,
) -> Result<u16> {
    if vault_path.trim().is_empty() {
        bail!("vault_path must not be empty");
    }
    if quota_gb == 0 {
        bail!("quota_gb must be greater than 0");
    }
    if web_password.len() < 6 {
        bail!("web_password must be at least 6 characters");
    }

    let state = runtime_state()?;
    let mut life = state.lifecycle.lock().await;

    if life.role_mode == RoleMode::ClientOnly {
        bail!(
            "cannot start local server in `client_only` mode; switch role_mode to `server_only` or `dual` first"
        );
    }

    if let Some(existing) = life.server_handle.as_ref() {
        if !existing.task.is_finished() {
            return Ok(existing.port);
        }
        life.server_handle = None;
    }

    let root = PathBuf::from(vault_path.trim());
    let db_path = root.join("index.db");
    let quota_bytes = quota_gb.saturating_mul(1024 * 1024 * 1024);
    let port = requested_port.filter(|&p| p > 0).unwrap_or(DEFAULT_SERVER_PORT);

    let config = ServerConfig {
        port,
        vault_root: root.clone(),
        default_quota_bytes: quota_bytes,
        web_password,
        device_name: current_hostname(),
        ..ServerConfig::default()
    };

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
    life.server_handle = Some(LocalServerHandle {
        port: actual_port,
        vault_path: root,
        db_path,
        stop_tx,
        task,
    });

    info!(port = actual_port, quota_gb, "started local LanVault server");
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

    let diff = reconcile_directory(&state.client_db, &rule_id, Path::new(&rule.local_dir))?;
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
        },
    );

    if diff.is_empty() {
        return Ok(());
    }

    let peer = state
        .active_peer
        .read()
        .await
        .clone()
        .ok_or_else(|| anyhow!("no paired peer session active for incremental upload"))?;

    let policy = state.policy.read().await.clone();
    let engine = SyncEngine::new(
        peer.endpoint_base_url.clone(),
        peer.session_cookie.clone(),
        state.client_device_id.clone(),
        state.vault_master_key.clone(),
        policy,
    )?;

    let mut engine_rx = engine.subscribe_progress();
    let progress_tx = state.progress_tx.clone();
    let latest_progress = Arc::clone(&state.latest_progress);
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
            };
            *latest_progress
                .lock()
                .expect("latest_progress mutex poisoned") = ffi_ev.clone();
            let _ = progress_tx.send(ffi_ev);
        }
    });

    let resolved_vault_id = if rule.target_vault_id == "default"
        || rule.target_vault_id == "vault-primary"
    {
        match peer.default_vault_id.clone() {
            Some(vid) => vid,
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

    *state.policy.write().await = new_policy;

    info!(
        wifi_only,
        require_charging, max_kbps, "power/network sync policy updated"
    );
    Ok(())
}

/// 9. 复用加密信道推送局域网剪贴板文本至目标设备（或全体在线设备）。
pub async fn push_clipboard_text(target_device_id: Option<String>, text: String) -> Result<()> {
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
    let resp = client.push_text(target_device_id, &text).await?;
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
    let target_mode = parse_role_mode(&mode)?;
    let state = runtime_state()?;
    let mut life = state.lifecycle.lock().await;

    state
        .client_db
        .set_setting(SETTING_ROLE_MODE, target_mode.as_str())?;
    life.role_mode = target_mode;

    let active_peer = state.active_peer.read().await.clone();
    match target_mode {
        RoleMode::ClientOnly => {
            stop_local_server_gracefully(&mut life).await?;
            state.reconnect.set_enabled(true);
            start_client_background_tasks(&state, &mut life, active_peer);
        }
        RoleMode::ServerOnly => {
            state.reconnect.set_enabled(false);
            stop_client_background_tasks(&mut life);
        }
        RoleMode::Dual => {
            state.reconnect.set_enabled(true);
            start_client_background_tasks(&state, &mut life, active_peer);
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
    Ok(rules.into_iter().map(SyncRuleDto::from).collect())
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
        .set_setting("debug_mode", if enabled { "1" } else { "0" })?;
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
pub(crate) fn load_or_generate_jni_web_password() -> Result<String> {
    const KEY: &str = "jni_generated_web_password";
    let state = runtime_state()?;
    if let Some(existing) = state.client_db.get_setting(KEY)? {
        if existing.len() >= 12 {
            return Ok(existing);
        }
    }
    let key = SymmetricKey::generate();
    let generated = hex::encode(&key.as_bytes()[..12]);
    state.client_db.set_setting(KEY, &generated)?;
    Ok(generated)
}
