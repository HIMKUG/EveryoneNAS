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
use tracing::{debug, error, info, warn};

use lanvault_client::transport::{peer_http_builder, PeerFingerprint};
use lanvault_client::{
    reconcile_directory, ClipboardClient, DirectoryWatcher, PowerNetworkPolicy,
    DeviceEnvironment, ReconnectController, ReconnectEvent, ReconnectTarget, SyncEngine,
    TokenBucket, WatchEvent,
};
use lanvault_core::{
    unpack_chunk, CHUNK_MAX_SIZE,
    blake3_hash, decrypt_aead, encrypt_aead, pairing_identity_message, pairing_response_message,
    session_renew_message,
    parse_role_mode, verify_ed25519, Database, DeviceIdentity, DiscoveredPeerInfo,
    DiscoveryBrowser, PoolMode, RoleMode, SessionKeyExchange, SymmetricKey, SyncRuleRecord,
    TransferHistoryRecord, TrustedPeer, VolumeRole,
};
use lanvault_server::scrub::{BitRotAlertEvent, BitRotScrubber};
use lanvault_server::vault::{ExtraVolumeSpec, VaultStore};
use lanvault_server::web::PendingPairRequestDto;
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
const SETTING_AUTOSTART_SERVER: &str = "autostart_server";
/// 本地服务管理密码的加密副本，仅用于"启动时自动拉起服务 + 自动登录管理员会话"。
const SETTING_SERVER_PASSWORD_ENC: &str = "server_web_password_protected";

/// 已配对设备显式握手间隔。服务端租约为 45s，15s 握手留三次容错余量。
const PEER_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// 与 `lanvault-server` 的在线租约保持一致：超过这个时长没有成功握手即视为离线。
const PEER_PRESENCE_LEASE_SECS: u64 = 45;
/// 备份目录隔离子目录名：用户选择的目录下总是再建一层，避免与他人文件混放。
pub const BACKUP_ISOLATION_DIR: &str = "everyonenasbackup";

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
    app_state: Option<Arc<lanvault_server::web::AppState>>,
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
    addr_sync_task: Option<JoinHandle<()>>,
    reconnect_event_task: Option<JoinHandle<()>>,
    heartbeat_task: Option<JoinHandle<()>>,
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
    fingerprint: PeerFingerprint,
    http: Client,
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
    http_client: Client,
    client_db: Arc<Database>,
    client_identity: DeviceIdentity,
    client_device_id: String,
    vault_master_key: RwLock<SymmetricKey>,
    lifecycle: AsyncMutex<LifecycleState>,
    reconnect: ReconnectController,
    policy: Arc<RwLock<PowerNetworkPolicy>>,
    rate_limiter: Arc<TokenBucket>,
    progress_tx: broadcast::Sender<SyncProgressEvent>,
    latest_progress: Arc<Mutex<HashMap<String, SyncProgressEvent>>>,
    discovered_peers: Arc<Mutex<HashMap<String, DiscoveredPeer>>>,
    /// `device_id -> 最近一次握手成功的时刻`。UI 的"在线"唯一来源：
    /// 只有双方真的互相握过手，才会显示在线（不再用 mDNS 缓存或残留会话推断）。
    peer_last_heartbeat: Arc<Mutex<HashMap<String, Instant>>>,
    mdns_browser_started: Mutex<bool>,
    active_peer: Arc<RwLock<Option<ActivePeerSession>>>,
    environment: Arc<RwLock<DeviceEnvironment>>,
    sync_inflight: Arc<AsyncMutex<HashSet<String>>>,
    sync_engine_cache: AsyncMutex<Option<(SocketAddr, PeerFingerprint, Arc<SyncEngine>)>>,
    fast_backup_mode: AtomicBool,
}

static GLOBAL_STATE: OnceLock<Arc<FfiRuntimeState>> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());
static DAEMON_MODE: AtomicBool = AtomicBool::new(false);

impl FfiRuntimeState {
    /// 本机服务端当前监听端口（未启动时回退到最近一次配置的端口），
    /// 用于握手时告知对端"到我这个端口来连"，使对端的地址簿在 DHCP 变更后自愈。
    async fn own_server_port(&self) -> Option<u16> {
        let life = self.lifecycle.lock().await;
        if let Some(handle) = life.server_handle.as_ref() {
            if !handle.task.is_finished() {
                return Some(handle.port);
            }
        }
        life.last_server_config.as_ref().map(|cfg| cfg.port)
    }
}

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
    // 默认不选择任何文件夹：备份范围必须由用户显式指定，
    // 避免首次启动就把 图片/文档 目录整体上传到局域网存储端。
    Vec::new()
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
    // 旧版本留下的"自动替你选好图片/文档"的默认规则必须收回，否则老用户升级后
    // 仍然会在未授权的情况下备份整目录。
    for legacy_rule_id in ["rule-photos", "rule-docs"] {
        if let Ok(Some(rule)) = db.get_sync_rule(legacy_rule_id) {
            let isolated = Path::new(&rule.local_dir).join(BACKUP_ISOLATION_DIR);
            if rule.target_vault_id == "default" && !isolated.exists() {
                db.delete_sync_rule(legacy_rule_id)?;
                info!(
                    rule_id = legacy_rule_id,
                    dir = %rule.local_dir,
                    "removed legacy auto-selected backup rule"
                );
            }
        }
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
    let Some(trusted) = db.get_trusted_peer(&persisted.device_id)?.filter(|peer| peer.revoked_at.is_none()) else {
        return Ok(None);
    };
    let fingerprint = trusted_peer_fingerprint(&trusted)?;
    let key_bytes = hex::decode(&persisted.channel_key_hex)
        .context("invalid persisted channel_key_hex")?;
    let key_arr: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| anyhow!("persisted channel_key must be 32 bytes"))?;
    Ok(Some(ActivePeerSession {
        device_id: persisted.device_id,
        addr,
        endpoint_base_url: format!("https://{addr}"),
        fingerprint,
        http: peer_http_client(fingerprint)?,
        session_cookie: persisted.session_cookie,
        channel_key: SymmetricKey::from_bytes(key_arr),
        default_vault_id: persisted.default_vault_id,
    }))
}

/// 清除持久化的活跃对端会话（配对被对端撤销或本地解绑后调用）。
fn clear_persisted_peer(db: &Database) -> Result<()> {
    db.set_setting(SETTING_ACTIVE_PEER_JSON, "")?;
    Ok(())
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
    client: &Client,
    endpoint_base_url: &str,
    session_cookie: Option<&str>,
) -> Option<String> {
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
    if let Some(t) = life.heartbeat_task.take() {
        t.abort();
    }
}

/// 已配对设备的显式双向握手循环。
///
/// 每 15s 向对端 `POST /api/peers/heartbeat`（携带 `lv_session` Cookie）。对端据此续期
/// 在线租约并回传它看到的在线设备集合；本端同时刷新 `active_peer` 的 IP/端口（DHCP、
/// 路由器重启后 IP 变化可自愈）。这是"手机端显示电脑在线"的唯一凭据——
/// 没有握手成功，就没有在线。
fn spawn_peer_heartbeat_task(
    state: &Arc<FfiRuntimeState>,
    peer: ActivePeerSession,
) -> JoinHandle<()> {
    let state_for_task = Arc::clone(state);
    state.runtime.spawn(async move {
        let mut interval = tokio::time::interval(PEER_HEARTBEAT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let current = state_for_task.active_peer.read().await.clone();
            let Some(current) = current else {
                // 配对已被解除，握手循环自然结束。
                break;
            };
            if current.device_id != peer.device_id {
                break;
            }

            let Some(cookie) = current.session_cookie.clone() else {
                continue;
            };
            let url = format!("{}/api/peers/heartbeat", current.endpoint_base_url);
            let result = current
                .http
                .post(&url)
                .header("Cookie", format!("lv_session={cookie}"))
                .json(&serde_json::json!({
                    "last_port": state_for_task.own_server_port().await,
                }))
                .send()
                .await;

            match result {
                Ok(resp) if resp.status().is_success() => {
                    let body: HeartbeatAck = match resp.json().await {
                        Ok(body) => body,
                        Err(err) => {
                            warn!(peer = %current.device_id, "peer heartbeat response invalid JSON: {err}");
                            continue;
                        }
                    };
                    handle_heartbeat_ack(&state_for_task, &current, body).await;
                }
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if status == 401 || status == 403 {
                        // 服务端重启后内存会话失效，并不代表配对被撤销。先用设备私钥
                        // 换发新会话，成功了就继续心跳；只有续期也被拒绝时才认定被解绑。
                        match renew_peer_session(&state_for_task, &current).await {
                            Ok(_) => {
                                info!(peer = %current.device_id, "peer session renewed after 401");
                                continue;
                            }
                            Err(err) => {
                                let denied = err.downcast_ref::<reqwest::Error>()
                                    .and_then(reqwest::Error::status)
                                    .is_some_and(|status| status.as_u16() == 401 || status.as_u16() == 403);
                                warn!(peer = %current.device_id, "peer session renewal failed ({err:#})");
                                if denied {
                                    abandon_peer_session(&state_for_task, &current.device_id).await;
                                    break;
                                }
                            }
                        }
                    }
                    warn!(peer = %current.device_id, status, "peer heartbeat rejected");
                }
                Err(err) => {
                    debug!(peer = %current.device_id, "peer heartbeat failed: {err}");
                }
            }
        }
    })
}

#[derive(Deserialize)]
struct HeartbeatAck {
    #[serde(default)]
    self_device_id: String,
    #[serde(default)]
    peers: Vec<HeartbeatPeer>,
}

#[derive(Deserialize)]
struct HeartbeatPeer {
    device_id: String,
    #[serde(default)]
    device_name: String,
    #[serde(default)]
    os_type: String,
    #[serde(default)]
    ip: Option<String>,
    #[serde(default)]
    port: Option<u16>,
}

/// 应用对端握手回执：把对端权威的在线视图写回本地 `client_state.db`，
/// 并校正 `active_peer` 的地址，保证两端状态机看到同一份事实。
async fn handle_heartbeat_ack(
    state: &Arc<FfiRuntimeState>,
    current: &ActivePeerSession,
    ack: HeartbeatAck,
) {
    if ack.self_device_id.is_empty() {
        return;
    }
    // 记录一次成功的双向握手：这是 UI 判定"对端在线"的唯一依据。
    if let Ok(mut guard) = state.peer_last_heartbeat.lock() {
        guard.insert(current.device_id.clone(), Instant::now());
    }
    // 注意：ack.peers 中 device_id == state.client_device_id 的条目记录的是“客户端自身地址”，
    // 绝不能用客户端自身 IP/Port 覆盖远端 active_peer.addr，否则双端模式下 15s 心跳后会把后续上传打向本机。

    // 对端把我们解绑时不再回传本设备条目；此时只保留本地记录（用户可手动在 UI 解绑），
    // 绝不在网络抖动时静默删除合法配对。
    for peer in ack.peers {
        if peer.device_id == state.client_device_id {
            continue;
        }
        let Some(existing) = state.client_db.get_trusted_peer(&peer.device_id).ok().flatten() else {
            continue;
        };
        let device_name = if peer.device_name.trim().is_empty() {
            existing.device_name.clone()
        } else {
            peer.device_name
        };
        let os_type = if peer.os_type.trim().is_empty() {
            existing.os_type.clone()
        } else {
            peer.os_type
        };
        let updated = TrustedPeer {
            device_id: existing.device_id,
            device_name,
            os_type,
            ed25519_pubkey: existing.ed25519_pubkey,
            last_ip: peer.ip.or(existing.last_ip),
            last_port: peer.port.or(existing.last_port),
            auto_connect: existing.auto_connect,
            paired_at: existing.paired_at,
            revoked_at: existing.revoked_at,
        };
        if let Err(err) = state.client_db.upsert_trusted_peer(&updated) {
            warn!(peer = %updated.device_id, "failed to persist peer heartbeat metadata: {err}");
        }
    }
}

#[derive(Debug, Deserialize)]
struct SessionRenewChallengeResp {
    challenge: String,
    server_device_id: String,
}

/// 用本机设备私钥向对端换发新的 `lv_session`，并把新 Cookie 写回活跃会话。
async fn renew_peer_session(
    state: &Arc<FfiRuntimeState>,
    current: &ActivePeerSession,
) -> Result<ActivePeerSession> {
    let challenge_url = format!("{}/api/peers/session/challenge", current.endpoint_base_url);
    let challenge_resp = current
        .http
        .post(&challenge_url)
        .json(&serde_json::json!({
            "device_id": state.client_device_id,
        }))
        .send()
        .await
        .with_context(|| format!("session renewal challenge request to {challenge_url} failed"))?;
    if !challenge_resp.status().is_success() {
        challenge_resp.error_for_status_ref()?;
        bail!(
            "session renewal challenge rejected with HTTP {}",
            challenge_resp.status().as_u16()
        );
    }
    let challenge_body: SessionRenewChallengeResp = challenge_resp
        .json()
        .await
        .context("invalid session renewal challenge JSON")?;
    if challenge_body.challenge.trim().is_empty() {
        bail!("session renewal challenge was empty");
    }
    if challenge_body.server_device_id != current.device_id {
        bail!(
            "session renewal server_device_id mismatch: expected `{}`, got `{}`",
            current.device_id,
            challenge_body.server_device_id
        );
    }

    let message = session_renew_message(
        &challenge_body.server_device_id,
        &state.client_device_id,
        challenge_body.challenge.trim(),
    );
    let signature = state.client_identity.sign(&message);
    let url = format!("{}/api/peers/session", current.endpoint_base_url);
    let resp = current
        .http
        .post(&url)
        .json(&serde_json::json!({
            "device_id": state.client_device_id,
            "client_ed25519_pubkey_hex": hex::encode(state.client_identity.public_key_bytes()),
            "client_ed25519_signature_hex": hex::encode(signature),
            "nonce": challenge_body.challenge.trim(),
        }))
        .send()
        .await
        .with_context(|| format!("session renewal request to {url} failed"))?;

    if !resp.status().is_success() {
        resp.error_for_status_ref()?;
        bail!(
            "session renewal rejected with HTTP {}",
            resp.status().as_u16()
        );
    }
    let body: SessionRenewAck = resp
        .json()
        .await
        .context("invalid session renewal response JSON")?;
    let cookie = body
        .session_cookie
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| anyhow!("session renewal response missing session cookie"))?;

    let mut refreshed_out = current.clone();
    refreshed_out.session_cookie = Some(cookie.clone());
    let mut guard = state.active_peer.write().await;
    if let Some(session) = guard.as_mut() {
        if session.device_id == current.device_id {
            session.session_cookie = Some(cookie);
            refreshed_out = session.clone();
            save_persisted_peer(
                &state.client_db,
                &refreshed_out,
                &state.client_identity.derive_local_storage_key("client-state"),
            )?;
        }
    }
    Ok(refreshed_out)
}

#[derive(Deserialize)]
struct SessionRenewAck {
    #[serde(default)]
    session_cookie: Option<String>,
}

/// 对端已撤销本设备会话：清空活跃会话与持久化记录，避免继续向死会话上传。
async fn abandon_peer_session(state: &Arc<FfiRuntimeState>, device_id: &str) {
    let mut guard = state.active_peer.write().await;
    if guard.as_ref().is_some_and(|peer| peer.device_id == device_id) {
        *guard = None;
        if let Err(err) = clear_persisted_peer(&state.client_db) {
            warn!("failed to clear persisted peer session: {err}");
        }
        info!(peer = %device_id, "active peer session abandoned after remote revocation");
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
            let sched = state
                .client_db
                .get_setting(&format!("{SETTING_RULE_SCHEDULE_PREFIX}{}", rule.rule_id))
                .ok()
                .flatten()
                .unwrap_or_else(|| "realtime".to_string());
            if sched == "manual" {
                continue;
            }
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
        life.heartbeat_task = Some(spawn_peer_heartbeat_task(state, peer.clone()));
        let dynamic_addr = Arc::new(std::sync::RwLock::new(peer.addr));
        let addr_sync_state = Arc::clone(state);
        let addr_sync_target = Arc::clone(&dynamic_addr);
        life.addr_sync_task = Some(state.runtime.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                if let Some(active) = addr_sync_state.active_peer.read().await.as_ref() {
                    if let Ok(mut g) = addr_sync_target.write() {
                        *g = active.addr;
                    }
                }
            }
        }));
        life.reconnect_task = Some(state.reconnect.spawn_peer_loop(ReconnectTarget {
            device_id: peer.device_id,
            addr: peer.addr,
            dynamic_addr: Some(dynamic_addr),
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
    let http_client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .context("failed to initialize shared FFI reqwest client")?;
    let rate_limiter = Arc::new(TokenBucket::new(persisted_policy.max_kbps));
    let state = Arc::new(FfiRuntimeState {
        runtime,
        http_client,
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
            addr_sync_task: None,
            reconnect_event_task: None,
            heartbeat_task: None,
        }),
        reconnect: ReconnectController::new(reconnect_enabled),
        policy: Arc::new(RwLock::new(persisted_policy)),
        rate_limiter,
        progress_tx,
        latest_progress: Arc::new(Mutex::new(HashMap::new())),
        discovered_peers: Arc::new(Mutex::new(HashMap::new())),
        peer_last_heartbeat: Arc::new(Mutex::new(HashMap::new())),
        mdns_browser_started: Mutex::new(false),
        active_peer: Arc::new(RwLock::new(persisted_peer.clone())),
        environment: Arc::new(RwLock::new(DeviceEnvironment {
            on_wifi: !cfg!(target_os = "android"),
            current_ssid: None,
            is_charging: !cfg!(target_os = "android"),
        })),
        sync_inflight: Arc::new(AsyncMutex::new(HashSet::new())),
        sync_engine_cache: AsyncMutex::new(None),
        fast_backup_mode: AtomicBool::new(false),
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

    // 首次配置之后（存在 last_server_config + 加密密码）自动拉起存储服务，
    // 用户不需要每次打开软件再手动点一次"启动"。失败只记日志，不影响客户端能力。
    if !daemon_mode && persisted_role != RoleMode::ClientOnly {
        match state.runtime.block_on(autostart_local_server()) {
            Ok(Some(port)) => info!(port, "storage service restored automatically on startup"),
            Ok(None) => {}
            Err(err) => warn!("autostart local storage server skipped: {err:#}"),
        }
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
    let Ok(db) = Database::shared(db_path) else {
        return ("unknown".to_string(), 0);
    };
    let Ok(corrupted_chunks) = db.count_corrupted_chunks() else {
        return ("unknown".to_string(), 0);
    };
    if corrupted_chunks > 0 {
        return ("corrupted".to_string(), corrupted_chunks);
    }
    let any_volume_offline = db
        .list_all_volumes()
        .ok()
        .map(|vols| vols.iter().any(|vol| !vol.is_online))
        .unwrap_or(false);
    if any_volume_offline {
        ("degraded".to_string(), 0)
    } else {
        ("healthy".to_string(), 0)
    }
}

fn query_local_bit_rot_health(state: &FfiRuntimeState) -> (String, u64) {
    let (app_state, db_path) = {
        let Ok(life) = state.lifecycle.try_lock() else {
            return ("unknown".to_string(), 0);
        };
        match life.server_handle.as_ref() {
            Some(handle) if !handle.task.is_finished() => {
                (handle.app_state.clone(), Some(handle.db_path.clone()))
            }
            _ => (None, None),
        }
    };
    if let Some(app_state) = app_state {
        if let Ok(status) = app_state.build_system_status() {
            return (status.bit_rot_health, status.corrupted_chunks);
        }
    }
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
        Ok(mut guard) => {
            guard.insert(event.rule_id.clone(), event.clone());
        }
        Err(_) => error!("internal lock poisoned while updating latest_progress"),
    }
    let _ = state.progress_tx.send(event);
}

/// 把各规则的最新进度聚合成一个对外视图。
///
/// - 只要有规则处于传输态，就返回那个规则（并累加所有规则的已处理字节，速度取传输态的值），
///   这样"正在传输"绝不会被另一条规则的空闲/完成事件顶掉成 `0 B`。
/// - 全部空闲时返回最近一次事件，保证 UI 仍能显示"已完成 / 已是最新"。
fn aggregate_latest_progress(state: &FfiRuntimeState) -> Result<SyncProgressEvent> {
    let entries: Vec<SyncProgressEvent> = {
        let guard = state
            .latest_progress
            .lock()
            .map_err(|_| anyhow!("internal lock poisoned"))?;
        guard.values().cloned().collect()
    };
    let is_active = |status: &str| {
        let lower = status.to_ascii_lowercase();
        lower.starts_with("sync") || lower.starts_with("upload") || lower == "scanning"
    };

    let active: Vec<SyncProgressEvent> = entries
        .iter()
        .filter(|event| is_active(&event.status))
        .cloned()
        .collect();

    let picked = if !active.is_empty() {
        // 以"已传输最多"的那条规则作为主视图：并发时另一条规则可能刚进入 scanning
        // (0 字节)，若按 HashMap 任意顺序取第一条，界面又会回到 `0 B`。
        let mut merged = active
            .iter()
            .max_by_key(|event| event.bytes_processed)
            .cloned()
            .unwrap_or_default();
        merged.bytes_processed = active.iter().map(|e| e.bytes_processed).sum();
        merged.total_bytes = active
            .iter()
            .filter(|e| e.total_bytes > 0)
            .map(|e| e.total_bytes)
            .sum();
        merged.chunks_uploaded = active.iter().map(|e| e.chunks_uploaded).sum();
        merged.chunks_deduplicated = active.iter().map(|e| e.chunks_deduplicated).sum();
        merged.throughput_bps = active.iter().map(|e| e.throughput_bps).sum();
        // 只要有规则在真正传数据，就不要让"正在扫描"盖掉传输状态。
        if let Some(uploading) = active.iter().find(|e| {
            let lower = e.status.to_ascii_lowercase();
            lower.starts_with("sync") || lower.starts_with("upload")
        }) {
            merged.status = uploading.status.clone();
        }
        merged
    } else {
        entries
            .into_iter()
            .max_by_key(|event| event.bytes_processed)
            .unwrap_or_default()
    };

    let mut picked = picked;
    let (bit_rot_health, corrupted_chunks) = query_local_bit_rot_health(state);
    picked.bit_rot_health = bit_rot_health;
    picked.corrupted_chunks = corrupted_chunks;
    Ok(picked)
}

async fn stop_local_server_gracefully(lifecycle: &mut LifecycleState) -> Result<()> {
    // 先清缓存：服务端一停，内存会话表随之销毁，缓存 Cookie 已经无意义。
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
            let db = Database::shared(&handle.db_path).with_context(|| {
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
    aggregate_latest_progress(&state)
}

const MAX_DISCOVERED_PEERS: usize = 256;

fn insert_discovered_peer_bounded(
    guard: &mut HashMap<String, DiscoveredPeer>,
    peer: DiscoveredPeer,
) {
    if guard.len() >= MAX_DISCOVERED_PEERS {
        guard.retain(|_, existing| existing.last_seen.elapsed() < Duration::from_secs(30));
    }
    if guard.len() >= MAX_DISCOVERED_PEERS {
        if let Some(oldest_id) = guard
            .iter()
            .min_by_key(|(_, entry)| entry.last_seen)
            .map(|(id, _)| id.clone())
        {
            guard.remove(&oldest_id);
        }
    }
    guard.insert(peer.device_id.clone(), peer);
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
        let scan_state = Arc::clone(&state);

        state.runtime.spawn(async move {
            let _browser_guard = browser;
            while let Some(info) = rx.recv().await {
                let peer: DiscoveredPeer = info.into();
                if let Ok(mut guard) = peers_cache.lock() {
                    insert_discovered_peer_bounded(&mut guard, peer);
                } else {
                    error!("internal lock poisoned while updating discovered_peers");
                    break;
                }
            }
            if let Ok(mut started) = scan_state.mdns_browser_started.lock() {
                *started = false;
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
    let _runtime_guard = state.runtime.enter();
    let (tx, mut rx) = mpsc::channel::<DiscoveredPeerInfo>(64);
    let browser = DiscoveryBrowser::start(state.client_device_id.clone(), tx)
        .context("failed to start dual-channel DiscoveryBrowser")?;
    let peers_cache = Arc::clone(&state.discovered_peers);

    state.runtime.spawn(async move {
        while let Some(info) = rx.recv().await {
            let peer: DiscoveredPeer = info.into();
            if let Ok(mut guard) = peers_cache.lock() {
                insert_discovered_peer_bounded(&mut guard, peer.clone());
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

#[derive(Serialize)]
struct PairRequestInitiatePayload {
    nonce: String,
    client_ed25519_pubkey_hex: String,
    client_x25519_pubkey_hex: String,
    client_ed25519_signature_hex: String,
    device_name: String,
    os_type: String,
}

#[derive(Deserialize)]
struct PairRequestInitiateResponse {
    request_id: String,
    poll_token: String,
    expires_in_secs: u64,
}

#[derive(Serialize)]
struct PairRequestStatusPayload {
    request_id: String,
    poll_token: String,
}

#[derive(Deserialize)]
struct PairRequestStatusResponse {
    status: String,
    response: Option<PairPinResponse>,
}

#[derive(Deserialize)]
struct PendingPairRequestsResponse {
    requests: Vec<PendingPairRequestDto>,
}

fn peer_http_client(fingerprint: PeerFingerprint) -> Result<Client> {
    Ok(peer_http_builder(fingerprint)?.timeout(Duration::from_secs(10)).build()?)
}

fn trusted_peer_fingerprint(peer: &TrustedPeer) -> Result<PeerFingerprint> {
    let key = peer.ed25519_pubkey.as_slice().try_into()
        .map_err(|_| anyhow!("invalid public key for trusted device {}", peer.device_id))?;
    Ok(PeerFingerprint::from_public_key(key))
}

fn pairing_target(state: &FfiRuntimeState, input: &str, port: u16) -> Result<(SocketAddr, PeerFingerprint)> {
    let (host, supplied_pin) = match input.trim().split_once('#') {
        Some((host, pin)) => (host, Some(pin.parse::<PeerFingerprint>()?)),
        None => (input.trim(), None),
    };
    let ip: IpAddr = host.parse().context("invalid peer IP address")?;
    if port == 0 {
        bail!("peer port must be non-zero");
    }
    let addr = SocketAddr::new(ip, port);
    if let Some(pin) = supplied_pin {
        return Ok((addr, pin));
    }
    let mut matching = state.client_db.list_trusted_peers()?.into_iter().filter(|peer| {
        peer.revoked_at.is_none()
            && peer.last_ip.as_deref().and_then(|value| value.parse::<IpAddr>().ok()) == Some(ip)
            && peer.last_port == Some(port)
    });
    match (matching.next(), matching.next()) {
        (Some(peer), None) => Ok((addr, trusted_peer_fingerprint(&peer)?)),
        _ => bail!("verify the fingerprint displayed on the other device before pairing"),
    }
}

async fn finalize_paired_peer(
    state: &Arc<FfiRuntimeState>,
    addr: SocketAddr,
    fingerprint: PeerFingerprint,
    http: Client,
    challenge: &str,
    kex: SessionKeyExchange,
    client_ed25519_pubkey: &[u8; 32],
    client_x25519_pubkey: &[u8; 32],
    identity_message: &[u8],
    body: PairPinResponse,
) -> Result<String> {
    let server_ed25519_pubkey = hex::decode(&body.server_ed25519_pubkey_hex)
        .context("invalid server_ed25519_pubkey_hex")?;
    let peer_pub_bytes = hex::decode(&body.server_x25519_pubkey_hex)
        .context("invalid server_x25519_pubkey_hex")?;
    let server_signature = hex::decode(&body.server_ed25519_signature_hex)
        .context("invalid server_ed25519_signature_hex")?;
    let clipboard_envelope = hex::decode(&body.clipboard_key_envelope_hex)
        .context("invalid clipboard_key_envelope_hex")?;
    let response_message = pairing_response_message(
        challenge,
        client_ed25519_pubkey,
        client_x25519_pubkey,
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
    if PeerFingerprint::from_public_key(&server_public_key) != fingerprint {
        bail!("pairing identity does not match the TLS device fingerprint");
    }
    let expected_server_device_id = format!(
        "lv-{}",
        &lanvault_core::public_key_fingerprint(&server_public_key)[..12]
    );
    if body.device_id != expected_server_device_id {
        bail!("server device id does not match its signing key");
    }
    let session_key = kex
        .agree(&peer_pub_bytes, identity_message)
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
        last_ip: Some(addr.ip().to_string()),
        last_port: Some(addr.port()),
        auto_connect: state.reconnect.is_enabled(),
        paired_at,
        revoked_at: None,
    })?;

    let endpoint_base_url = format!("https://{addr}");
    let default_vault_id = fetch_remote_default_vault_id(
        &http,
        &endpoint_base_url,
        body.session_cookie.as_deref(),
    )
    .await;
    let session = ActivePeerSession {
        device_id: body.device_id.clone(),
        addr,
        endpoint_base_url,
        fingerprint,
        http,
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
    // 配对成功的瞬间即视为一次握手：两端状态机从这一刻起就一致。
    if let Ok(mut guard) = state.peer_last_heartbeat.lock() {
        guard.insert(session.device_id.clone(), Instant::now());
    }

    let mut life = state.lifecycle.lock().await;
    start_client_background_tasks(state, &mut life, Some(session));

    Ok(body.device_id)
}

/// 2. 使用 6 位动态 PIN 码执行 X25519 + Ed25519 零信任配对，返回对端 `device_id`。
pub async fn pair_with_pin(ip: String, port: u16, pin: String) -> Result<String> {
    let _probe = lanvault_core::ProbeGuard::start(
        "rust.ffi",
        "ffi.pair_with_pin",
        None,
    );
    if pin.len() != 6 || !pin.chars().all(|c| c.is_ascii_digit()) {
        bail!("pairing PIN must be a 6-digit numeric string");
    }

    let state = runtime_state()?;
    let (socket_addr, fingerprint) = pairing_target(&state, &ip, port)?;
    let http = peer_http_client(fingerprint)?;
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

    let url = format!("https://{socket_addr}/api/peers/pair");

    let resp = http
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
    let device_id = finalize_paired_peer(
        &state,
        socket_addr,
        fingerprint,
        http,
        &pin,
        kex,
        &client_ed25519_pubkey,
        &client_x25519_pubkey,
        &identity_message,
        body,
    )
    .await?;

    info!(peer_device_id = %device_id, addr = %socket_addr, "paired with peer via PIN");
    Ok(device_id)
}

/// 2b. 向对端发起交互式配对请求（接收方弹窗点击“同意配对”后自动完成密钥协商，无需输入 PIN）。
pub async fn request_pairing(ip: String, port: u16) -> Result<String> {
    let _probe = lanvault_core::ProbeGuard::start(
        "rust.ffi",
        "ffi.request_pairing",
        None,
    );

    let state = runtime_state()?;
    let (socket_addr, fingerprint) = pairing_target(&state, &ip, port)?;
    let http = peer_http_client(fingerprint)?;
    let kex = SessionKeyExchange::new();
    let client_ed25519_pubkey = state.client_identity.public_key_bytes();
    let client_x25519_pubkey = kex.public_key_bytes();
    let device_name = current_hostname();
    let os_type = std::env::consts::OS.to_string();
    let mut nonce_bytes = [0u8; 16];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = hex::encode(nonce_bytes);
    let identity_message = pairing_identity_message(
        &nonce,
        &client_ed25519_pubkey,
        &client_x25519_pubkey,
        &device_name,
        &os_type,
    );
    let client_signature = state.client_identity.sign(&identity_message);

    let init_url = format!("https://{socket_addr}/api/peers/pair-request");
    let init_resp = http
        .post(&init_url)
        .json(&PairRequestInitiatePayload {
            nonce: nonce.clone(),
            client_ed25519_pubkey_hex: hex::encode(client_ed25519_pubkey),
            client_x25519_pubkey_hex: hex::encode(client_x25519_pubkey),
            client_ed25519_signature_hex: hex::encode(client_signature),
            device_name,
            os_type,
        })
        .send()
        .await
        .with_context(|| format!("pairing request to {init_url} failed"))?;

    if !init_resp.status().is_success() {
        bail!(
            "peer rejected pairing initiation with HTTP {}",
            init_resp.status().as_u16()
        );
    }

    let init_body: PairRequestInitiateResponse = init_resp
        .json()
        .await
        .context("invalid pair-request response JSON")?;
    let status_url = format!("https://{socket_addr}/api/peers/pair-status");
    let deadline = Instant::now() + Duration::from_secs(init_body.expires_in_secs.clamp(10, 90));

    loop {
        if Instant::now() >= deadline {
            bail!("pairing request timed out waiting for peer approval");
        }
        tokio::time::sleep(Duration::from_millis(750)).await;

        let poll_resp = http
            .post(&status_url)
            .json(&PairRequestStatusPayload {
                request_id: init_body.request_id.clone(),
                poll_token: init_body.poll_token.clone(),
            })
            .send()
            .await
            .with_context(|| format!("pairing status poll to {status_url} failed"))?;

        if !poll_resp.status().is_success() {
            bail!(
                "pairing status poll rejected with HTTP {}",
                poll_resp.status().as_u16()
            );
        }

        let status_body: PairRequestStatusResponse = poll_resp
            .json()
            .await
            .context("invalid pair-status response JSON")?;

        match status_body.status.as_str() {
            "pending" => continue,
            "rejected" => bail!("peer rejected pairing request"),
            "expired" => bail!("pairing request expired"),
            "approved" => {
                let pair_resp = status_body
                    .response
                    .ok_or_else(|| anyhow!("approved pairing status missing response payload"))?;
                let device_id = finalize_paired_peer(
                    &state,
                    socket_addr,
                    fingerprint,
                    http,
                    &nonce,
                    kex,
                    &client_ed25519_pubkey,
                    &client_x25519_pubkey,
                    &identity_message,
                    pair_resp,
                )
                .await?;
                info!(peer_device_id = %device_id, addr = %socket_addr, "paired with peer via interactive approval");
                return Ok(device_id);
            }
            other => bail!("unexpected pairing status `{other}`"),
        }
    }
}

/// 2c. 列出当前等待本机确认的入站配对请求。
pub async fn list_pending_pair_requests() -> Result<Vec<PendingPairRequestDto>> {
    let state = runtime_state()?;
    let (app_state, port) = {
        let life = state.lifecycle.lock().await;
        match life.server_handle.as_ref() {
            Some(handle) if !handle.task.is_finished() => {
                (handle.app_state.clone(), Some(handle.port))
            }
            _ => (None, None),
        }
    };
    if let Some(app_state) = app_state {
        return Ok(app_state.list_pending_pair_requests());
    }
    let Some(port) = port else {
        return Ok(Vec::new());
    };

    let cookie = ensure_local_admin_cookie(&state, port).await?;
    let url = format!("http://127.0.0.1:{port}/api/peers/pair-requests");
    let resp = state
        .http_client
        .get(&url)
        .header("Cookie", format!("lv_session={cookie}"))
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("failed to query pending pair requests: HTTP {}", resp.status().as_u16());
    }
    let parsed: PendingPairRequestsResponse = resp.json().await?;
    Ok(parsed.requests)
}

/// 2d. 同意或拒绝入站配对请求。
pub async fn respond_pair_request(request_id: String, accept: bool) -> Result<bool> {
    let trimmed = request_id.trim().to_string();
    if trimmed.is_empty() {
        bail!("request_id must not be empty");
    }
    let state = runtime_state()?;
    let (app_state, port) = {
        let life = state.lifecycle.lock().await;
        match life.server_handle.as_ref() {
            Some(handle) if !handle.task.is_finished() => {
                (handle.app_state.clone(), Some(handle.port))
            }
            _ => (None, None),
        }
    };
    let accepted = if let Some(app_state) = app_state {
        app_state.respond_pair_request(&trimmed, accept)?
    } else if let Some(port) = port {
        let cookie = ensure_local_admin_cookie(&state, port).await?;
        let url = format!("http://127.0.0.1:{port}/api/peers/pair-requests/{trimmed}/respond");
        let resp = state
            .http_client
            .post(&url)
            .header("Cookie", format!("lv_session={cookie}"))
            .json(&serde_json::json!({ "accept": accept }))
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("respond_pair_request failed with HTTP {}", resp.status().as_u16());
        }
        accept
    } else {
        bail!("local server is not running");
    };

    lanvault_core::record_event_always(
        "INFO",
        "rust.peer",
        Some("pairing.responded"),
        None,
        format!(
            "{} pairing request {}",
            if accepted { "accepted" } else { "rejected" },
            trimmed
        ),
        None,
    );
    Ok(accepted)
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
    let (app_state_tx, app_state_rx) =
        oneshot::channel::<Arc<lanvault_server::web::AppState>>();
    let mut initial_bound_tx = Some(bound_tx);
    let mut initial_app_state_tx = Some(app_state_tx);

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
            let current_app_state_tx = initial_app_state_tx.take();
            let exit_res = lanvault_server::serve_with_shutdown_and_state(
                loop_config.clone(),
                iter_shutdown_rx,
                current_bound_tx,
                current_app_state_tx,
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
    let app_state = app_state_rx.await.ok();

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
        app_state,
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

    // 记住"这台机器已经配置过存储服务"，并把管理密码加密留档：
    // 之后每次启动都据此自动拉起服务，用户不需要再手动点一次"启动"。
    const AUTOSTART_AAD: &[u8] = b"lanvault.local.server-password.v1";
    let protection_key = state
        .client_identity
        .derive_local_storage_key("client-state");
    let encrypted_password =
        encrypt_setting(trimmed_password.as_bytes(), &protection_key, AUTOSTART_AAD)?;
    state
        .client_db
        .set_setting(SETTING_SERVER_PASSWORD_ENC, &encrypted_password)?;
    if state
        .client_db
        .get_setting(SETTING_AUTOSTART_SERVER)?
        .is_none()
    {
        state.client_db.set_setting(SETTING_AUTOSTART_SERVER, "1")?;
    }

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

/// 启动时自动拉起上次配置好的本地存储服务。
///
/// 只在"这台机器已完成过一次配置"时动作（`last_server_config` + 加密留档的密码都在），
/// 因此新装用户不会莫名多出一个监听端口，而老用户每次打开软件都能直接处于存储服务在线状态。
/// 返回自动启动后的端口；未满足条件时返回 `None`。
pub async fn autostart_local_server() -> Result<Option<u16>> {
    let state = runtime_state()?;

    {
        let autostart = state
            .client_db
            .get_setting(SETTING_AUTOSTART_SERVER)?
            .as_deref()
            != Some("0");
        if !autostart {
            return Ok(None);
        }
        let life = state.lifecycle.lock().await;
        if life.role_mode == RoleMode::ClientOnly {
            return Ok(None);
        }
        if let Some(handle) = life.server_handle.as_ref() {
            if !handle.task.is_finished() {
                return Ok(Some(handle.port));
            }
        }
    }

    let Some(raw) = state
        .client_db
        .get_setting(SETTING_LAST_SERVER_CONFIG_JSON)?
    else {
        return Ok(None);
    };
    let cfg: LastServerConfigDto =
        serde_json::from_str(&raw).context("invalid persisted last_server_config_json")?;
    if !Path::new(&cfg.vault_path).is_dir() {
        warn!(
            vault = %cfg.vault_path,
            "autostart skipped: previous vault path is no longer reachable"
        );
        return Ok(None);
    }

    const AUTOSTART_AAD: &[u8] = b"lanvault.local.server-password.v1";
    let protection_key = state
        .client_identity
        .derive_local_storage_key("client-state");
    let Some(encrypted) = state
        .client_db
        .get_setting(SETTING_SERVER_PASSWORD_ENC)?
    else {
        return Ok(None);
    };
    let password_bytes = decrypt_setting(&encrypted, &protection_key, AUTOSTART_AAD)?;
    let password = String::from_utf8(password_bytes)
        .context("persisted server password is not valid UTF-8")?;

    match start_local_server_with_port(
        cfg.vault_path.clone(),
        cfg.quota_gb,
        password,
        Some(cfg.port),
    )
    .await
    {
        Ok(port) => {
            info!(port, "local storage server autostarted from previous configuration");
            Ok(Some(port))
        }
        Err(err) => {
            // 自动启动失败不能阻塞 UI：日志留痕，用户在"存储"页仍可手动启动。
            warn!(error = %err, "local storage server autostart failed");
            Ok(None)
        }
    }
}

/// 读取"启动时自动开启存储服务"开关。
pub async fn get_autostart_server() -> Result<bool> {
    let state = runtime_state()?;
    Ok(state
        .client_db
        .get_setting(SETTING_AUTOSTART_SERVER)?
        .as_deref()
        != Some("0"))
}

/// 设置"启动时自动开启存储服务"开关。
pub async fn set_autostart_server(enabled: bool) -> Result<bool> {
    let state = runtime_state()?;
    state.client_db.set_setting(
        SETTING_AUTOSTART_SERVER,
        if enabled { "1" } else { "0" },
    )?;
    info!(enabled, "autostart local storage server preference updated");
    Ok(enabled)
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
        let previous = {
            let guard = state
                .latest_progress
                .lock()
                .map_err(|_| anyhow!("internal lock poisoned"))?;
            guard.get(&rule_id).cloned().unwrap_or_default()
        };
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
                &peer.http,
                &peer.endpoint_base_url,
                peer.session_cookie.as_deref(),
            )
            .await
            .ok_or_else(|| anyhow!("remote peer has no available vault"))?,
        }
    } else {
        rule.target_vault_id.clone()
    };

    let diff_db = Arc::clone(&state.client_db);
    let diff_rule_id = rule_id.clone();
    let diff_vault_id = resolved_vault_id.clone();
    let diff_dir = PathBuf::from(&rule.local_dir);
    let diff = tokio::task::spawn_blocking(move || {
        reconcile_directory(&diff_db, &diff_rule_id, &diff_vault_id, &diff_dir)
    })
    .await
    .context("reconcile_directory worker panicked")??;
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
    let engine = {
        let mut cache = state.sync_engine_cache.lock().await;
        if let Some((addr, pin, existing)) = cache.as_ref() {
            if *addr == peer.addr && *pin == peer.fingerprint {
                existing.update_session_cookie(peer.session_cookie.clone()).await;
                existing.update_policy(policy.clone()).await;
                existing.update_environment(environment.clone()).await;
                existing.set_fast_mode(state.fast_backup_mode.load(Ordering::Relaxed));
                Arc::clone(existing)
            } else {
                let created = Arc::new(SyncEngine::new(
                    peer.endpoint_base_url.clone(),
                    peer.session_cookie.clone(),
                    state.client_device_id.clone(),
                    state.vault_master_key.read().await.clone(),
                    policy.clone(),
                    environment.clone(),
                    Some(Arc::clone(&state.rate_limiter)),
                    peer.fingerprint,
                )?);
                created.set_fast_mode(state.fast_backup_mode.load(Ordering::Relaxed));
                *cache = Some((peer.addr, peer.fingerprint, Arc::clone(&created)));
                created
            }
        } else {
            let created = Arc::new(SyncEngine::new(
                peer.endpoint_base_url.clone(),
                peer.session_cookie.clone(),
                state.client_device_id.clone(),
                state.vault_master_key.read().await.clone(),
                policy.clone(),
                environment.clone(),
                Some(Arc::clone(&state.rate_limiter)),
                peer.fingerprint,
            )?);
            created.set_fast_mode(state.fast_backup_mode.load(Ordering::Relaxed));
            *cache = Some((peer.addr, peer.fingerprint, Arc::clone(&created)));
            created
        }
    };

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

    let mut sync_result = engine
        .sync_diff(&state.client_db, &rule_id, &resolved_vault_id, &diff)
        .await;
    if matches!(
        sync_result,
        Err(lanvault_client::ClientError::RemoteRejected { status: 401, .. })
    ) {
        if let Ok(renewed) = renew_peer_session(&state, &peer).await {
            engine.update_session_cookie(renewed.session_cookie).await;
            sync_result = engine
                .sync_diff(&state.client_db, &rule_id, &resolved_vault_id, &diff)
                .await;
        }
    }
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
    let vault_key = state.vault_master_key.read().await.clone();
    tokio::task::spawn_blocking(move || -> Result<String> {
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let backup_key = derive_key_backup_key(&passphrase, &salt)?;
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
    })
    .await
    .context("export_vault_key_backup worker panicked")?
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
    let key = tokio::task::spawn_blocking(move || -> Result<SymmetricKey> {
        let backup_key = derive_key_backup_key(&passphrase, &salt)?;
        let plaintext = decrypt_aead(
            &backup_key,
            &ciphertext,
            b"lanvault.key-backup.v1",
        )
        .context("key-backup passphrase is incorrect or backup is corrupted")?;
        SymmetricKey::from_slice(&plaintext)
            .context("vault key backup contains an invalid key")
    })
    .await
    .context("import_vault_key_backup worker panicked")??;
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
        peer.fingerprint,
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
        delivered_to = 1usize,
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
///
/// `primary_dir` / `fallback_dir` 是**用户选择的目录**（UI 直接展示、可编辑）；
/// `primary_root` / `fallback_root` 是后端在该目录下自动创建的隔离子目录
/// `<所选目录>/everyonenasbackup`，所有备份产物只写进它，绝不与用户自己的文件混放。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupStorageConfigDto {
    pub primary_dir: String,
    #[serde(default)]
    pub primary_root: String,
    pub fallback_dir: Option<String>,
    #[serde(default)]
    pub fallback_root: Option<String>,
}

/// 把用户选择的目录规范化成隔离后的备份根：`<selected>/everyonenasbackup`。
fn backup_isolation_root(selected: &str) -> PathBuf {
    let trimmed = selected.trim().trim_end_matches(['/', '\\']);
    let base = if trimmed.is_empty() {
        PathBuf::from(selected.trim())
    } else {
        PathBuf::from(trimmed)
    };
    // 用户可能直接选到隔离目录本身，此时不要重复嵌套。
    if base
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(BACKUP_ISOLATION_DIR))
    {
        base
    } else {
        base.join(BACKUP_ISOLATION_DIR)
    }
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
///
/// 关键修正：一次配对会**同时**写两端各自的一份记录，但本机如果同时是服务端，
/// 对端是"在它的服务端库里"登记本机的，本机 `client_state.db` 里可能还没有对端条目。
/// 因此这里把「本机服务端 index.db 的可信列表」和「本机客户端 client_state.db」的列表
/// 取并集，再用握手术语判定在线，避免出现"手机端显示电脑在线、电脑端没有这台手机"。
pub async fn list_paired_peers() -> Result<Vec<PairedPeerDto>> {
    let state = runtime_state()?;
    let active = state.active_peer.read().await.clone();
    let mut trusted_list = state.client_db.list_trusted_peers()?;

    // 合并本机服务端侧的可信设备（可能是对端在配对时写入我们服务端 index.db 的那一份）。
    let mut server_online_ids: HashSet<String> = HashSet::new();
    if let Some((db_path, server_running, port)) = resolve_local_server_db_path(&state).await? {
        if db_path.exists() {
            let server_peers = tokio::task::spawn_blocking(move || -> Result<Vec<TrustedPeer>> {
                let db = Database::shared(&db_path)?;
                Ok(db.list_trusted_peers()?)
            })
            .await
            .context("list_paired_peers worker panicked")??;

            for peer in server_peers {
                if let Some(existing) = trusted_list
                    .iter_mut()
                    .find(|candidate| candidate.device_id == peer.device_id)
                {
                    // 服务端侧的 IP/端口是真实观测值，比客户端缓存更可信。
                    if existing.last_ip.is_none() {
                        existing.last_ip = peer.last_ip.clone();
                    }
                    if existing.last_port.is_none() {
                        existing.last_port = peer.last_port;
                    }
                    if existing.device_name == existing.device_id
                        && peer.device_name != peer.device_id
                    {
                        existing.device_name = peer.device_name.clone();
                        existing.os_type = peer.os_type.clone();
                    }
                } else {
                    trusted_list.push(peer);
                }
            }

            // 本机服务端观察到的在线（对端主动握手到我们）。
            if server_running {
                if let Ok(live) = fetch_local_server_peers_live(&state, port).await {
                    for entry in live.peers {
                        if entry.online {
                            server_online_ids.insert(entry.device_id);
                        }
                    }
                }
            }
        }
    }

    let mut online_ids: HashSet<String> = server_online_ids;
    let mut last_seen_map: HashMap<String, i64> = HashMap::new();
    // 只信任"双方握手成功"这一件事推导在线状态：先取本地记录的成功握手时刻。
    {
        let heartbeats = state
            .peer_last_heartbeat
            .lock()
            .map_err(|_| anyhow!("internal lock poisoned"))?;
        for (id, at) in heartbeats.iter() {
            let elapsed = at.elapsed().as_secs() as i64;
            last_seen_map.insert(id.clone(), elapsed);
            if at.elapsed() < Duration::from_secs(PEER_PRESENCE_LEASE_SECS) {
                online_ids.insert(id.clone());
            }
        }
    }

    let mut peers = Vec::with_capacity(trusted_list.len() + usize::from(active.is_some()));
    let mut seen = HashSet::new();
    let active_online = active
        .as_ref()
        .is_some_and(|a| online_ids.contains(&a.device_id));

    for trusted in trusted_list {
        let (ip, port) = match active.as_ref() {
            Some(a) if a.device_id == trusted.device_id => {
                (Some(a.addr.ip().to_string()), Some(a.addr.port()))
            }
            _ => (trusted.last_ip, trusted.last_port),
        };
        // 有活跃会话但握手失败时不算在线：避免"手机端显示电脑在线、电脑端却没这台手机"。
        let online = online_ids.contains(&trusted.device_id);
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
                online: active_online,
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
        let mut cfg: BackupStorageConfigDto = serde_json::from_str(&raw)
            .context("invalid persisted backup_storage_config_json")?;
        // 兼容升级前只存 `primary_dir` 的旧配置：隔离根在读取时补齐。
        if cfg.primary_root.trim().is_empty() && !cfg.primary_dir.trim().is_empty() {
            cfg.primary_root = backup_isolation_root(&cfg.primary_dir)
                .to_string_lossy()
                .into_owned();
        }
        if cfg.fallback_root.is_none() {
            cfg.fallback_root = cfg
                .fallback_dir
                .as_deref()
                .map(|dir| backup_isolation_root(dir).to_string_lossy().into_owned());
        }
        return Ok(cfg);
    }
    Ok(BackupStorageConfigDto {
        primary_dir: String::new(),
        primary_root: String::new(),
        fallback_dir: None,
        fallback_root: None,
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
    // 隔离根必须真实存在：用户只选一次目录，之后备份只写进 everyonenasbackup。
    let primary_root = backup_isolation_root(&primary);
    fs::create_dir_all(&primary_root).with_context(|| {
        format!("无法创建备份隔离子目录 {}", primary_root.display())
    })?;
    let fallback_root = match fallback {
        Some(ref fb) => {
            let root = backup_isolation_root(fb);
            fs::create_dir_all(&root)
                .with_context(|| format!("无法创建备用备份隔离子目录 {}", root.display()))?;
            Some(root.to_string_lossy().into_owned())
        }
        None => None,
    };

    let cfg = BackupStorageConfigDto {
        primary_dir: primary,
        primary_root: primary_root.to_string_lossy().into_owned(),
        fallback_dir: fallback,
        fallback_root,
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

fn is_usable_lan_ipv4(ip: Ipv4Addr) -> bool {
    if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    let octets = ip.octets();
    // Exclude CGNAT / Tailscale (100.64.0.0/10) and Clash/Mihomo Fake-IP TUN (198.18.0.0/15)
    if octets[0] == 100 && (octets[1] & 0b1100_0000) == 64 {
        return false;
    }
    if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) {
        return false;
    }
    true
}

fn is_rfc1918_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 192 && o[1] == 168
        || o[0] == 10
        || (o[0] == 172 && (16..=31).contains(&o[1]))
}

fn detect_lan_ipv4() -> String {
    // 1. Try local LAN gateway probes without hitting TUN/proxy default routes
    for target in [
        Ipv4Addr::new(192, 168, 1, 1),
        Ipv4Addr::new(192, 168, 0, 1),
        Ipv4Addr::new(10, 0, 0, 1),
        Ipv4Addr::new(172, 16, 0, 1),
    ] {
        if let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
            if sock.connect((target, 80)).is_ok() {
                if let Ok(SocketAddr::V4(v4)) = sock.local_addr() {
                    let ip = *v4.ip();
                    if is_usable_lan_ipv4(ip) && is_rfc1918_ipv4(ip) {
                        return ip.to_string();
                    }
                }
            }
        }
    }
    // 2. Fallback: parse OS interface list on Windows/Unix to skip virtual TUN adapters
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if let Ok(out) = std::process::Command::new("ipconfig")
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                if line.contains("IPv4") {
                    if let Some(raw_ip) = line.rsplit(':').next() {
                        if let Ok(ip) = raw_ip.trim().parse::<Ipv4Addr>() {
                            if is_usable_lan_ipv4(ip) && is_rfc1918_ipv4(ip) {
                                return ip.to_string();
                            }
                        }
                    }
                }
            }
        }
    }
    Ipv4Addr::LOCALHOST.to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowsNetworkScanDto {
    pub has_wifi_adapter: bool,
    pub is_wired_ethernet: bool,
    pub current_ssid: Option<String>,
    pub available_ssids: Vec<String>,
}

/// 扫描本机网络环境（使用 `CREATE_NO_WINDOW` 隐藏控制台调用 `netsh`，消除 Windows 黑框闪烁）。
pub async fn scan_windows_networks() -> Result<WindowsNetworkScanDto> {
    #[cfg(not(target_os = "windows"))]
    {
        Ok(WindowsNetworkScanDto {
            has_wifi_adapter: true,
            is_wired_ethernet: false,
            current_ssid: None,
            available_ssids: Vec::new(),
        })
    }
    #[cfg(target_os = "windows")]
    {
        tokio::task::spawn_blocking(move || -> Result<WindowsNetworkScanDto> {
            use std::collections::BTreeSet;
            use std::os::windows::process::CommandExt;
            use std::process::Command;

            const CREATE_NO_WINDOW: u32 = 0x0800_0000;

            let mut current_ssid: Option<String> = None;
            let mut has_wifi_adapter = false;
            let mut ssids = BTreeSet::new();

            if let Ok(iface_out) = Command::new("netsh")
                .creation_flags(CREATE_NO_WINDOW)
                .args(["wlan", "show", "interfaces"])
                .output()
            {
                let text = String::from_utf8_lossy(&iface_out.stdout);
                let err_text = String::from_utf8_lossy(&iface_out.stderr);
                let combined = format!("{text}\n{err_text}");
                let no_iface = combined.contains("There is no wireless interface")
                    || combined.contains("系统上没有无线接口");
                if iface_out.status.success() && !no_iface {
                    has_wifi_adapter = true;
                    for line in text.lines() {
                        let trimmed = line.trim();
                        if trimmed.starts_with("SSID") && !trimmed.starts_with("BSSID") {
                            if let Some((_, val)) = trimmed.split_once(':') {
                                let v = val.trim();
                                if !v.is_empty() {
                                    current_ssid = Some(v.to_string());
                                    ssids.insert(v.to_string());
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            if has_wifi_adapter {
                if let Ok(net_out) = Command::new("netsh")
                    .creation_flags(CREATE_NO_WINDOW)
                    .args(["wlan", "show", "networks"])
                    .output()
                {
                    let text = String::from_utf8_lossy(&net_out.stdout);
                    if net_out.status.success() {
                        for line in text.lines() {
                            let trimmed = line.trim();
                            if trimmed.starts_with("SSID") && !trimmed.starts_with("BSSID") {
                                if let Some((_, val)) = trimmed.split_once(':') {
                                    let v = val.trim();
                                    if !v.is_empty() {
                                        ssids.insert(v.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let mut is_wired_ethernet = false;
            if current_ssid.is_none() {
                if let Ok(if_out) = Command::new("netsh")
                    .creation_flags(CREATE_NO_WINDOW)
                    .args(["interface", "show", "interface"])
                    .output()
                {
                    let text = String::from_utf8_lossy(&if_out.stdout);
                    for line in text.lines() {
                        let lower = line.to_lowercase();
                        let connected = lower.contains("connected") || line.contains("已连接");
                        let eth_like = lower.contains("ethernet")
                            || line.contains("以太网")
                            || lower.contains("lan");
                        if connected && eth_like {
                            is_wired_ethernet = true;
                            break;
                        }
                    }
                }
            }

            Ok(WindowsNetworkScanDto {
                has_wifi_adapter,
                is_wired_ethernet,
                current_ssid,
                available_ssids: ssids.into_iter().collect(),
            })
        })
        .await
        .context("scan_windows_networks worker panicked")?
    }
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

async fn login_local_admin_cookie(
    client: &Client,
    port: u16,
    web_password: &str,
) -> Result<String> {
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

async fn ensure_local_admin_cookie(state: &FfiRuntimeState, port: u16) -> Result<String> {
    let (cached_cookie, web_password) = {
        let life = state.lifecycle.lock().await;
        (
            life.local_admin_cookie.clone(),
            life.last_server_config
                .as_ref()
                .map(|cfg| cfg.web_password.clone()),
        )
    };
    if let Some(cookie) = cached_cookie {
        return Ok(cookie);
    }
    let password = web_password
        .ok_or_else(|| anyhow!("local server credentials unavailable for active server"))?;
    let fresh_cookie = login_local_admin_cookie(&state.http_client, port, &password).await?;
    {
        let mut life = state.lifecycle.lock().await;
        life.local_admin_cookie = Some(fresh_cookie.clone());
    }
    Ok(fresh_cookie)
}

/// 作废缓存的本地管理会话。
///
/// 本地服务端重启后内存里的会话表会被清空，缓存的 Cookie 就永久失效。若不主动清除，
/// 每个 UI 轮询都会先发一次注定 401 的请求再重新登录，白跑两倍往返。服务端停止与
/// 收到 401/403 时都必须调用这里。
async fn invalidate_local_admin_cookie(state: &FfiRuntimeState) {
    let mut life = state.lifecycle.lock().await;
    life.local_admin_cookie = None;
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

    let url = format!("http://127.0.0.1:{port}/api/peers");

    if let Some(cookie) = cached_cookie {
        let resp = state
            .http_client
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
        // 401/403 说明这个缓存会话已随服务端重启失效，清掉它再重新登录，
        // 否则后续每次轮询都会重复"发一次 401 + 再登录"的双倍往返。
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED
            || resp.status() == reqwest::StatusCode::FORBIDDEN
        {
            invalidate_local_admin_cookie(state).await;
        }
    }

    let password = web_password
        .ok_or_else(|| anyhow!("local server credentials unavailable for active server"))?;
    let fresh_cookie = login_local_admin_cookie(&state.http_client, port, &password).await?;
    {
        let mut life = state.lifecycle.lock().await;
        life.local_admin_cookie = Some(fresh_cookie.clone());
    }

    let resp = state
        .http_client
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
        let ip = match extract_ip_from_qr_uri(&live.pairing_qr_uri) {
            Some(ip) => ip,
            None => tokio::task::spawn_blocking(detect_lan_ipv4)
                .await
                .unwrap_or_else(|_| Ipv4Addr::LOCALHOST.to_string()),
        };
        let online_ids: HashSet<String> = live
            .peers
            .into_iter()
            .filter(|p| p.online)
            .map(|p| p.device_id)
            .collect();
        (ip, live.pairing_pin, live.pairing_qr_uri, online_ids)
    } else {
        let ip = tokio::task::spawn_blocking(detect_lan_ipv4)
            .await
            .unwrap_or_else(|_| Ipv4Addr::LOCALHOST.to_string());
        (ip, String::new(), String::new(), HashSet::new())
    };

    let (
        vault_used_bytes,
        vault_quota_bytes,
        vault_count,
        bit_rot_health,
        corrupted_chunks,
        dedup_ratio,
        compression_ratio,
        mut recent_activities,
        server_trusted_peers,
    ) = tokio::task::spawn_blocking(move || -> Result<_> {
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
                let db = Database::shared(db_path)?;
                let vaults = db.list_vaults()?;
                vault_count = vaults.len();
                for v in &vaults {
                    vault_used_bytes = vault_used_bytes.saturating_add(v.used_bytes);
                    vault_quota_bytes = vault_quota_bytes.saturating_add(v.quota_bytes);
                }
                let any_volume_offline = db.list_all_volumes()?.iter().any(|vol| !vol.is_online);
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

                let snapshots = db.list_snapshots_paginated(None, 10, 0)?;
                for snap in snapshots {
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
        Ok((
            vault_used_bytes,
            vault_quota_bytes,
            vault_count,
            bit_rot_health,
            corrupted_chunks,
            dedup_ratio,
            compression_ratio,
            recent_activities,
            server_trusted_peers,
        ))
    })
    .await
    .context("get_system_overview DB worker panicked")??;

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
        } else if probe == "ffi.pair_with_pin"
            || probe == "ffi.request_pairing"
            || probe == "pairing.responded"
            || probe == "peer.revoked"
        {
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

    let vaults = tokio::task::spawn_blocking(move || -> Result<Vec<VaultSummaryDto>> {
        let db = Database::shared(&db_path)?;
        let vaults = db.list_vaults()?;
        let mut volumes_by_vault: HashMap<String, Vec<lanvault_core::VaultVolumeRecord>> =
            HashMap::new();
        for vol in db.list_all_volumes()? {
            volumes_by_vault.entry(vol.vault_id.clone()).or_default().push(vol);
        }
        let mut result = Vec::with_capacity(vaults.len());
        for v in vaults {
            let volumes = volumes_by_vault.remove(&v.vault_id).unwrap_or_default();
            result.push(build_vault_summary_dto(v, volumes));
        }
        Ok(result)
    })
    .await
    .context("list_vaults worker panicked")??;
    Ok(vaults)
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
    let (db_path, server_running, app_state) = {
        let life = state.lifecycle.lock().await;
        match life.server_handle.as_ref() {
            Some(handle) if !handle.task.is_finished() => (
                Some(handle.db_path.clone()),
                true,
                handle.app_state.clone(),
            ),
            _ => (None, false, None),
        }
    };
    if !server_running {
        bail!("local server is not running; start local server first");
    }
    let db_path = db_path
        .ok_or_else(|| anyhow!("local server database is not initialized; start local server first"))?;

    tokio::task::spawn_blocking(move || -> Result<VaultSummaryDto> {
        let (vault, volumes) = if let Some(app_state) = app_state {
            app_state.vault_store.create_vault(
                trimmed_name,
                trimmed_mount,
                None,
                parsed_mode,
                quota_bytes,
                e2ee_enabled,
                extra_volumes,
            )?
        } else {
            let db = Database::shared(&db_path)?;
            let store = VaultStore::new(db);
            store.create_vault(
                trimmed_name,
                trimmed_mount,
                None,
                parsed_mode,
                quota_bytes,
                e2ee_enabled,
                extra_volumes,
            )?
        };
        Ok(build_vault_summary_dto(vault, volumes))
    })
    .await
    .context("create_vault worker panicked")?
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
        let db = Database::shared(&db_path)?;
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
pub async fn set_fast_backup_mode(enabled: bool) -> Result<bool> {
    let state = runtime_state()?;
    state.fast_backup_mode.store(enabled, Ordering::Relaxed);
    if let Some((_, _, engine)) = state.sync_engine_cache.lock().await.as_ref() {
        engine.set_fast_mode(enabled);
    }
    Ok(enabled)
}

pub async fn trigger_bit_rot_scrub() -> Result<ScrubResultDto> {
    let state = runtime_state()?;
    let local_db = resolve_local_server_db_path(&state).await?;
    let local_available = local_db.as_ref().map(|(p, _, _)| p.exists()).unwrap_or(false);
    if !local_available {
        let peer = state
            .active_peer
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow!("local server database is not initialized and no paired peer connected"))?;
        let url = format!("{}/api/scrub/trigger", peer.endpoint_base_url);
        let mut req = peer.http.post(&url);
        if let Some(ref c) = peer.session_cookie {
            req = req.header("Cookie", format!("lv_session={c}"));
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            bail!("remote integrity scrub failed: HTTP {}", resp.status());
        }
        let dto: ScrubResultDto = resp.json().await?;
        return Ok(dto);
    }
    let (db_path, _, _) = local_db.unwrap();

    let result = tokio::task::spawn_blocking(move || -> Result<ScrubResultDto> {
        let db = Database::shared(&db_path)?;
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
    let app_state = {
        let life = state.lifecycle.lock().await;
        life.server_handle
            .as_ref()
            .filter(|h| !h.task.is_finished())
            .and_then(|h| h.app_state.clone())
    };
    if let Some(app_state) = app_state {
        app_state.online_peers.remove(&target_id);
        app_state.auth.revoke_device_sessions(&target_id);
    }
    if let Some((db_path, _, _)) = resolve_local_server_db_path(&state).await? {
        if db_path.exists() {
            let tid = target_id.clone();
            server_revoked = tokio::task::spawn_blocking(move || -> Result<bool> {
                let server_db = Database::shared(&db_path)?;
                Ok(server_db.delete_trusted_peer(&tid)?)
            })
            .await
            .context("revoke_paired_peer worker panicked")??;
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
        let lan_ip = tokio::task::spawn_blocking(detect_lan_ipv4)
            .await
            .unwrap_or_else(|_| Ipv4Addr::LOCALHOST.to_string());
        return Ok(PairingInfoDto {
            server_running: false,
            lan_ip,
            port,
            pairing_pin: String::new(),
            pairing_qr_uri: String::new(),
        });
    }

    let live = fetch_local_server_peers_live(&state, port).await?;
    let lan_ip = match extract_ip_from_qr_uri(&live.pairing_qr_uri) {
        Some(ip) => ip,
        None => tokio::task::spawn_blocking(detect_lan_ipv4)
            .await
            .unwrap_or_else(|_| Ipv4Addr::LOCALHOST.to_string()),
    };

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

    let target_label = target_device_id
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(|| peer.device_id.clone());
    let transfer_client = lanvault_client::ResumableFileTransferClient::new(
        peer.endpoint_base_url.clone(),
        peer.session_cookie.clone(),
        peer.fingerprint,
    )?;
    let send_res = transfer_client
        .send_file_resumable(Path::new(&trimmed_path))
        .await;
    if matches!(
        send_res,
        Err(lanvault_client::ClientError::RemoteRejected { status: 401, .. })
    ) {
        let renewed = renew_peer_session(&state, &peer).await?;
        let retry_client = lanvault_client::ResumableFileTransferClient::new(
            renewed.endpoint_base_url,
            renewed.session_cookie,
            renewed.fingerprint,
        )?;
        retry_client
            .send_file_resumable(Path::new(&trimmed_path))
            .await?;
    } else {
        send_res?;
    }
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
            "pushed resumable file {} to {}",
            trimmed_path, target_label
        ),
        None,
    );

    info!(
        delivered_to = 1usize,
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

#[derive(Debug, Deserialize)]
struct RestoreManifestFile {
    rel_path: String,
    chunk_hashes_hex: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RestoreSnapshotManifest {
    files: Vec<RestoreManifestFile>,
}

fn sanitize_restore_rel_path(raw: &str) -> Result<PathBuf> {
    use std::path::Component;
    let p = Path::new(raw);
    if raw.trim().is_empty() || raw.contains('\0') || p.is_absolute() {
        bail!("invalid snapshot manifest path `{raw}`");
    }
    let mut clean = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::Normal(seg) => clean.push(seg),
            Component::CurDir => {}
            _ => bail!("unsafe snapshot manifest path `{raw}`"),
        }
    }
    if clean.as_os_str().is_empty() {
        bail!("empty snapshot manifest path");
    }
    Ok(clean)
}

/// 37. 从本地存储池解密、校验并还原指定快照数据块至目标目录（优先按 manifest 重建原始目录树）。
pub async fn restore_snapshot(
    snapshot_id: String,
    target_dir: String,
) -> Result<RestoreSnapshotResultDto> {
    let snap_id = snapshot_id.trim().to_string();
    let target_trim = target_dir.trim().to_string();
    if snap_id.is_empty()
        || snap_id.len() > 128
        || !snap_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || target_trim.is_empty()
    {
        bail!("invalid snapshot_id or empty target_dir");
    }

    let state = runtime_state()?;
    let vault_key = state.vault_master_key.read().await.clone();
    let local_db_opt = resolve_local_server_db_path(&state).await?;
    let has_local_snap = if let Some((ref db_path, _, _)) = local_db_opt {
        if db_path.exists() {
            Database::shared(db_path)
                .ok()
                .and_then(|db| db.get_snapshot(&snap_id).ok().flatten())
                .is_some()
        } else {
            false
        }
    } else {
        false
    };

    let result = if has_local_snap {
        let (db_path, _, _) = local_db_opt.unwrap();
        let snap_id_local = snap_id.clone();
        let target_local = target_trim.clone();
        let key_local = vault_key.clone();
        tokio::task::spawn_blocking(move || -> Result<RestoreSnapshotResultDto> {
            let db = Database::shared(&db_path)?;
            let snap = db
                .get_snapshot(&snap_id_local)?
                .ok_or_else(|| anyhow!("snapshot `{snap_id_local}` not found"))?;
            let vault_store = VaultStore::new(Arc::clone(&db));
            let chunk_key = key_local.derive_vault_chunk_key(&snap.vault_id);

            let out_dir = PathBuf::from(&target_local);
            std::fs::create_dir_all(&out_dir)
                .with_context(|| format!("failed to create restore directory {}", out_dir.display()))?;

            let mut loaded_manifest: Option<RestoreSnapshotManifest> = None;
            if let Ok(roots) = vault_store.manifest_backup_roots(&snap.vault_id) {
                for root in roots {
                    let candidate = root.join("manifests").join(format!("{}.json", snap.snapshot_id));
                    if candidate.is_file() {
                        let bytes = std::fs::read(&candidate)?;
                        loaded_manifest = Some(serde_json::from_slice::<RestoreSnapshotManifest>(&bytes)?);
                        break;
                    }
                }
            }

            let mut restored_chunks = 0u64;
            let mut restored_bytes = 0u64;
            let output_path_str: String;

            if let Some(manifest) = loaded_manifest {
                let restore_root = out_dir.join(format!("restore_{}", snap.snapshot_id));
                std::fs::create_dir_all(&restore_root)?;
                for file in &manifest.files {
                    let rel = sanitize_restore_rel_path(&file.rel_path)?;
                    let dest_path = restore_root.join(rel);
                    if let Some(parent) = dest_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let part_path = PathBuf::from(format!("{}.lanvault.part", dest_path.display()));
                    {
                        let mut dest_file = std::fs::File::create(&part_path)?;
                        for hash_hex in &file.chunk_hashes_hex {
                            let hash_bytes = hex::decode(hash_hex)?;
                            let hash: [u8; 32] = hash_bytes
                                .try_into()
                                .map_err(|_| anyhow!("invalid chunk hash length"))?;
                            let stored_bytes = vault_store.get_chunk(&snap.vault_id, &hash)?;
                            let comp = lanvault_core::ChunkCompression::from_db_tag(
                                &db.get_chunk_compression(&snap.vault_id, &hash),
                            );
                            let plain = lanvault_core::unpack_chunk_with_compression(
                                &stored_bytes,
                                &hash,
                                Some(&chunk_key),
                                snap.vault_id.as_bytes(),
                                CHUNK_MAX_SIZE as usize,
                                comp,
                            )?;
                            std::io::Write::write_all(&mut dest_file, &plain)?;
                            restored_chunks += 1;
                            restored_bytes += plain.len() as u64;
                        }
                        dest_file.sync_all()?;
                    }
                    std::fs::rename(&part_path, &dest_path)?;
                }
                output_path_str = restore_root.to_string_lossy().into_owned();
            } else {
                let hashes = db.list_snapshot_chunks(&snap.snapshot_id)?;
                let out_file_path = out_dir.join(format!("restore_{}.bin", snap.snapshot_id));
                let part_path = PathBuf::from(format!("{}.lanvault.part", out_file_path.display()));
                {
                    let mut out_file = std::fs::File::create(&part_path)?;
                    for hash in &hashes {
                        let stored_bytes = vault_store.get_chunk(&snap.vault_id, hash)?;
                        let comp = lanvault_core::ChunkCompression::from_db_tag(
                            &db.get_chunk_compression(&snap.vault_id, hash),
                        );
                        let plain = lanvault_core::unpack_chunk_with_compression(
                            &stored_bytes,
                            hash,
                            Some(&chunk_key),
                            snap.vault_id.as_bytes(),
                            CHUNK_MAX_SIZE as usize,
                            comp,
                        )?;
                        std::io::Write::write_all(&mut out_file, &plain)?;
                        restored_chunks += 1;
                        restored_bytes += plain.len() as u64;
                    }
                    out_file.sync_all()?;
                }
                std::fs::rename(&part_path, &out_file_path)?;
                output_path_str = out_file_path.to_string_lossy().into_owned();
            }

            Ok(RestoreSnapshotResultDto {
                snapshot_id: snap.snapshot_id,
                restored_chunks,
                restored_bytes,
                output_path: output_path_str,
            })
        })
        .await
        .context("snapshot restore worker panicked")??
    } else {
        // 远端 HTTP 回传恢复：纯客户端（如手机）从对端 NAS 拉取 manifest 与块并在本地校验解密落盘
        let peer = state
            .active_peer
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow!("local snapshot not found and no paired peer connected for remote restore"))?;
        let http = peer_http_builder(peer.fingerprint)?
            .read_timeout(Duration::from_secs(30))
            .build()?;
        let manifest_url = format!("{}/api/snapshots/{}/manifest", peer.endpoint_base_url, snap_id);
        let mut req = http.get(&manifest_url);
        if let Some(ref c) = peer.session_cookie {
            req = req.header("Cookie", format!("lv_session={c}"));
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            bail!("failed to fetch remote snapshot manifest: HTTP {}", resp.status());
        }
        #[derive(Deserialize)]
        struct RemoteManifestResp {
            vault_id: String,
            snapshot_id: String,
            manifest: RestoreSnapshotManifest,
        }
        let remote: RemoteManifestResp = resp.json().await?;
        let chunk_key = vault_key.derive_vault_chunk_key(&remote.vault_id);
        if remote.snapshot_id.is_empty()
            || remote.snapshot_id.len() > 128
            || !remote.snapshot_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("invalid remote snapshot_id");
        }
        let restore_root = PathBuf::from(&target_trim).join(format!("restore_{}", remote.snapshot_id));
        tokio::fs::create_dir_all(&restore_root).await?;

        let mut restored_chunks = 0u64;
        let mut restored_bytes = 0u64;
        for file in &remote.manifest.files {
            let rel = sanitize_restore_rel_path(&file.rel_path)?;
            let dest_path = restore_root.join(rel);
            if let Some(parent) = dest_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let part_path = PathBuf::from(format!("{}.lanvault.part", dest_path.display()));
            let mut out_file = std::fs::File::create(&part_path)?;
            for hash_hex in &file.chunk_hashes_hex {
                let hash_bytes = hex::decode(hash_hex)?;
                let hash: [u8; 32] = hash_bytes
                    .try_into()
                    .map_err(|_| anyhow!("invalid chunk hash length"))?;
                let chunk_url = format!(
                    "{}/api/vaults/{}/chunks/{}",
                    peer.endpoint_base_url, remote.vault_id, hash_hex
                );
                let mut cresp_opt = None;
                for attempt in 1..=4u32 {
                    let mut creq = http.get(&chunk_url);
                    if let Some(ref c) = peer.session_cookie {
                        creq = creq.header("Cookie", format!("lv_session={c}"));
                    }
                    match creq.send().await {
                        Ok(r) if r.status().is_success() => {
                            cresp_opt = Some(r);
                            break;
                        }
                        Ok(r) if matches!(r.status().as_u16(), 408 | 429 | 500 | 502 | 503 | 504) && attempt < 4 => {
                            tokio::time::sleep(Duration::from_millis(400u64 << (attempt - 1))).await;
                        }
                        Ok(r) => bail!("failed to download chunk {}: HTTP {}", hash_hex, r.status()),
                        Err(_) if attempt < 4 => {
                            tokio::time::sleep(Duration::from_millis(400u64 << (attempt - 1))).await;
                        }
                        Err(err) => return Err(err.into()),
                    }
                }
                let cresp = cresp_opt.ok_or_else(|| anyhow!("failed to download chunk {hash_hex} after retries"))?;
                let enc_hdr = cresp
                    .headers()
                    .get("x-lanvault-encoding")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("xchacha20poly1305+zstd")
                    .to_string();
                let comp = if enc_hdr.ends_with("+raw") {
                    lanvault_core::ChunkCompression::Raw
                } else {
                    lanvault_core::ChunkCompression::Zstd
                };
                let stored = cresp.bytes().await?;
                let plain = lanvault_core::unpack_chunk_with_compression(
                    &stored,
                    &hash,
                    Some(&chunk_key),
                    remote.vault_id.as_bytes(),
                    CHUNK_MAX_SIZE as usize,
                    comp,
                )?;
                std::io::Write::write_all(&mut out_file, &plain)?;
                restored_chunks += 1;
                restored_bytes += plain.len() as u64;
            }
            out_file.sync_all()?;
            drop(out_file);
            std::fs::rename(&part_path, &dest_path)?;
        }
        RestoreSnapshotResultDto {
            snapshot_id: remote.snapshot_id,
            restored_chunks,
            restored_bytes,
            output_path: restore_root.to_string_lossy().into_owned(),
        }
    };

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
