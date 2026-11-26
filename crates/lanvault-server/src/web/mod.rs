//! Axum 0.8 HTTP REST API, Cookie-authenticated WebSocket telemetry/alerts with
//! Ping/Pong backpressure disconnection,
//! and graceful restart orchestration.
//!
//! 100% aligned with `ARCHITECTURE_CONTRACTS.md` §3.1 (REST endpoints + `/ws`).

mod catalog_routes;
mod debug_routes;
mod groups_routes;
use catalog_routes::*;
use debug_routes::*;
use groups_routes::*;

use std::collections::HashSet;
use std::net::IpAddr;
use std::path::{Component, Path as StdPath, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::Request;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::header::{HeaderName, HeaderValue, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use std::fs::{self, File};
use std::io::Write;

use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use lanvault_core::crypto::constant_time_eq;
use lanvault_core::{
    blake3_hash, encrypt_aead, pairing_identity_message, pairing_response_message,
    public_key_fingerprint, session_renew_message, verify_ed25519, ClipboardKind, Database,
    DeviceIdentity, LanVaultError, MessageTargetAck, PoolMode, RetentionTier, SessionKeyExchange,
    SnapshotRecord, SymmetricKey, TrustedPeer, VolumeRole, CHUNK_MAX_SIZE, CLIPBOARD_AAD,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, Notify};
use tower_cookies::{CookieManagerLayer, Cookies};
use tracing::{info, warn};

use crate::auth::{
    build_clear_session_cookie, build_session_cookie, AuthManager, AuthOutcome, LoginOutcome,
    SessionInfo, SESSION_COOKIE_NAME, SESSION_TTL_SECS,
};
use crate::scrub::{BitRotAlertEvent, BitRotScrubber, ScrubReport};
use crate::tls::LanConnectionInfo;
use crate::vault::{unix_timestamp_secs, ExtraVolumeSpec, VaultStore};

const WS_PING_INTERVAL_SECS: u64 = 15;
const WS_PONG_TIMEOUT_SECS: u64 = 30;
const WS_OUTBOUND_QUEUE_CAPACITY: usize = 32;
const GRACEFUL_DRAIN_SECS: u64 = 3;
/// 单块上传请求体上限。FastCDC 最大块为 4 MiB，缓存后仍有充足余量。
const MAX_CHUNK_UPLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_CLIPBOARD_BODY_BYTES: usize = 16 * 1024 * 1024;
pub const PENDING_PAIR_TTL_SECS: i64 = 60;
pub const MAX_PENDING_PAIR_REQUESTS: usize = 16;
pub const MAX_PENDING_PAIR_REQUESTS_PER_IP: usize = 2;
pub const RENEW_CHALLENGE_TTL_SECS: u64 = 60;
const MAX_RENEW_CHALLENGES: usize = 256;
const RENEW_CHALLENGE_WINDOW_SECS: u64 = 60;
const MAX_RENEW_CHALLENGES_PER_IP: u32 = 20;
const MAX_RENEW_CHALLENGES_PER_DEVICE: u32 = 8;
const MAX_RENEW_CHALLENGES_ACTIVE_PER_DEVICE: usize = 4;
const MAX_RENEW_REQUESTS_PER_IP: u32 = 30;
const MAX_RENEW_RATE_ENTRIES: usize = 4096;
pub const MAX_TRANSFER_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_ACTIVE_TRANSFERS_PER_DEVICE: usize = 8;
pub const MAX_ACTIVE_TRANSFERS_GLOBAL: usize = 32;
pub const MAX_STAGING_BYTES_PER_DEVICE: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_STAGING_BYTES_GLOBAL: u64 = 32 * 1024 * 1024 * 1024;
const MAX_TRANSFER_PARTS: u64 = 16_384;
const MAX_RETAINED_TRANSFERS: usize = 10_000;
pub(crate) const TRANSFER_LOCK_COUNT: usize = 64;
/// 已配对设备显式握手的在线租约时长：设备必须每 15s 主动握手一次才算在线。
/// 这样"手机认为电脑在线"必然以一次经过 Cookie 鉴权的双向握手为前提，
/// 而不是依赖 mDNS 广播或上一次会话的残留状态（对齐 Tailscale / ZeroTier 的
/// "authenticated heartbeat, lease-based presence" 做法）。
pub const PEER_PRESENCE_LEASE_SECS: u64 = 45;

/// Interactive pairing request summary exposed to the receiving device UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingPairRequestDto {
    pub request_id: String,
    pub device_id: String,
    pub device_name: String,
    pub os_type: String,
    pub fingerprint: String,
    pub remote_ip: String,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub enum PendingPairStatus {
    Pending,
    Approved(PairPinResponse),
    Rejected,
}

#[derive(Debug, Clone)]
pub struct PendingPairEntry {
    pub dto: PendingPairRequestDto,
    pub poll_token: String,
    pub nonce: String,
    pub client_ed25519_pubkey: [u8; 32],
    pub client_x25519_pubkey: [u8; 32],
    pub identity_message: Vec<u8>,
    pub status: PendingPairStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairQueueRejectReason {
    SourceLimitReached,
    GlobalLimitReached,
}

#[derive(Debug, Clone)]
pub struct RenewChallengeEntry {
    pub device_id: String,
    pub server_device_id: String,
    pub expires_at: Instant,
}

/// RAII guard tracking in-flight chunk writes for graceful restart draining.
struct ActiveChunkWriteGuard {
    state: Arc<AppState>,
}

impl ActiveChunkWriteGuard {
    fn new(state: Arc<AppState>) -> Self {
        state.active_chunk_writes.fetch_add(1, Ordering::SeqCst);
        Self { state }
    }
}

impl Drop for ActiveChunkWriteGuard {
    fn drop(&mut self) {
        self.state
            .active_chunk_writes
            .fetch_sub(1, Ordering::SeqCst);
    }
}

/// WebSocket envelope pushed to connected dashboard clients.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum WsServerEvent {
    Telemetry(SystemStatusResponse),
    ServerRestarting {
        drain_secs: u64,
    },
    BitRotAlert(BitRotAlertEvent),
    ClipboardReceived {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_msg_id: Option<String>,
        from_device_id: String,
        target_device_id: Option<String>,
        kind: ClipboardKind,
        content: String,
        filename: Option<String>,
        encrypted: bool,
        timestamp: i64,
    },
    ClipboardStatus {
        client_msg_id: String,
        from_device_id: String,
        delivered_to: usize,
        accepted_to: usize,
        server_ack_at: i64,
        targets: Vec<MessageTargetAck>,
    },
    GroupSyncUpdate {
        group_id: String,
        rule_id: String,
        binding_id: String,
        operation_id: String,
        new_revision: i64,
        applied_ops: usize,
        timestamp: i64,
    },
    PairingRequested(PendingPairRequestDto),
    TransferReceived {
        transfer_id: String,
        filename: String,
        size_bytes: u64,
        saved_path: String,
        timestamp: i64,
    },
}

/// Shared runtime state injected into all Axum handlers.
pub struct AppState {
    pub device_id: String,
    pub identity: Arc<DeviceIdentity>,
    pub clipboard_key: SymmetricKey,
    pub os_name: String,
    pub keep_alive_level: String,
    pub lan_ip: String,
    pub port: u16,
    pub mdns_active: AtomicBool,
    pub started_at: Instant,
    pub throughput_bps: AtomicU64,
    pub(crate) transfer_bytes: AtomicU64,
    pub(crate) throughput_sample: std::sync::Mutex<(Instant, u64)>,
    pub active_chunk_writes: AtomicU64,
    pub upload_semaphore: tokio::sync::Semaphore,
    pub accepting_tasks: AtomicBool,
    pub pairing_pin: StdMutex<String>,
    pub pending_pair_lock: StdMutex<()>,
    pub pending_pair_requests: DashMap<String, PendingPairEntry>,
    pub renew_challenges: DashMap<String, RenewChallengeEntry>,
    pub renew_rate_limits: DashMap<String, (Instant, u32)>,
    pub transfer_budget_lock: StdMutex<()>,
    pub(crate) transfer_locks: [Arc<tokio::sync::RwLock<()>>; TRANSFER_LOCK_COUNT],
    /// WebSocket 在线连接计数（按 device_id 聚合）。
    pub online_peers: DashMap<String, usize>,
    /// 显式握手租约：`device_id -> 最近一次通过 Cookie 鉴权的握手时刻`。
    /// 只有租约未过期或存在活跃 WS 的设备才会被判定为在线，杜绝"单端残留显示在线"。
    pub peer_leases: DashMap<String, Instant>,
    pub db: Arc<Database>,
    pub vault_store: Arc<VaultStore>,
    pub scrubber: Arc<BitRotScrubber>,
    pub auth: Arc<AuthManager>,
    pub ws_tx: broadcast::Sender<WsServerEvent>,
    pub restart_notify: Arc<Notify>,
    pub(crate) telemetry_cache: RwLock<Option<(Instant, SystemStatusResponse)>>,
}

impl AppState {
    fn current_throughput_bps(&self) -> u64 {
        let total = self.transfer_bytes.load(Ordering::Relaxed);
        let Ok(mut sample) = self.throughput_sample.lock() else {
            return self.throughput_bps.load(Ordering::Relaxed);
        };
        let elapsed = sample.0.elapsed();
        if elapsed.as_secs_f64() < 1.0 {
            return self.throughput_bps.load(Ordering::Relaxed);
        }
        let delta = total.saturating_sub(sample.1);
        let throughput = (delta as f64 / elapsed.as_secs_f64()) as u64;
        self.throughput_bps.store(throughput, Ordering::Relaxed);
        *sample = (Instant::now(), total);
        throughput
    }

    /// 记录一次显式握手：把 `device_id` 的在线租约续期到 `now + PEER_PRESENCE_LEASE_SECS`。
    ///
    /// 这是服务端唯一认可的"设备在线"来源。只有通过 `lv_session` Cookie 鉴权并携带
    /// 已配对 `device_id` 的请求才能续租，因此手机端看到电脑在线，必然意味着电脑刚刚
    /// 成功发出过一次握手（对齐 Tailscale / ZeroTier 的 authenticated heartbeat 模型）。
    pub fn touch_peer_lease(&self, device_id: &str) {
        if self.peer_leases.len() >= 1024 {
            self.prune_peer_leases();
        }
        self.peer_leases
            .insert(device_id.to_string(), Instant::now());
    }

    /// 清理已过期的在线租约，避免内存与错误"在线"状态长期残留。
    pub fn prune_peer_leases(&self) {
        let lease = Duration::from_secs(PEER_PRESENCE_LEASE_SECS);
        self.peer_leases
            .retain(|_, last_seen| last_seen.elapsed() < lease);
    }

    /// 是否在线：连接了 WebSocket，或显式握手租约尚未过期。
    pub fn is_peer_online(&self, device_id: &str) -> bool {
        if self
            .online_peers
            .get(device_id)
            .is_some_and(|count| *count > 0)
        {
            return true;
        }
        self.peer_leases.get(device_id).is_some_and(|last_seen| {
            last_seen.elapsed() < Duration::from_secs(PEER_PRESENCE_LEASE_SECS)
        })
    }

    /// 当前全部在线已配对设备 ID。
    pub fn online_peer_ids(&self) -> HashSet<String> {
        self.prune_peer_leases();
        let mut ids: HashSet<String> = self
            .online_peers
            .iter()
            .filter(|entry| *entry.value() > 0)
            .map(|entry| entry.key().clone())
            .collect();
        let lease = Duration::from_secs(PEER_PRESENCE_LEASE_SECS);
        for entry in self.peer_leases.iter() {
            if entry.value().elapsed() < lease {
                ids.insert(entry.key().clone());
            }
        }
        ids
    }

    /// Builds a snapshot of current system telemetry matching `GET /api/system/status`.
    pub fn build_system_status(&self) -> Result<SystemStatusResponse, LanVaultError> {
        if let Ok(cache) = self.telemetry_cache.read() {
            if let Some((cached_at, status)) = cache.as_ref() {
                if cached_at.elapsed() < Duration::from_secs(1) {
                    return Ok(status.clone());
                }
            }
        }
        let corrupted_chunks = self.db.count_corrupted_chunks()?;
        let vaults = self.db.list_vaults()?;
        let all_volumes = self.db.list_all_volumes()?;
        let mut vault_used_bytes = 0u64;
        let mut vault_quota_bytes = 0u64;
        for v in &vaults {
            vault_used_bytes = vault_used_bytes.saturating_add(v.used_bytes);
            vault_quota_bytes = vault_quota_bytes.saturating_add(v.quota_bytes);
        }
        let any_volume_offline = all_volumes.iter().any(|vol| !vol.is_online);

        let bit_rot_health = if corrupted_chunks > 0 {
            "corrupted"
        } else if any_volume_offline {
            "degraded"
        } else {
            "healthy"
        };

        let status = SystemStatusResponse {
            os: self.os_name.clone(),
            keep_alive_level: self.keep_alive_level.clone(),
            lan_ip: self.lan_ip.clone(),
            port: self.port,
            mdns_active: self.mdns_active.load(Ordering::Relaxed),
            uptime_secs: self.started_at.elapsed().as_secs(),
            throughput_bps: self.current_throughput_bps(),
            bit_rot_health: bit_rot_health.to_string(),
            corrupted_chunks,
            vault_used_bytes,
            vault_quota_bytes,
        };
        if let Ok(mut cache) = self.telemetry_cache.write() {
            *cache = Some((Instant::now(), status.clone()));
        }
        Ok(status)
    }

    /// Executes the graceful restart drain sequence:
    /// 1. Stop accepting new tasks
    /// 2. Broadcast `server_restarting` over WebSocket
    /// 3. Wait up to 3 seconds for in-flight chunk writes to flush
    /// 4. Run `PRAGMA wal_checkpoint(TRUNCATE)`
    /// 5. Signal the host supervisor / service wrapper to restart
    pub async fn execute_graceful_restart(self: Arc<Self>) {
        self.accepting_tasks.store(false, Ordering::SeqCst);
        let _ = self.ws_tx.send(WsServerEvent::ServerRestarting {
            drain_secs: GRACEFUL_DRAIN_SECS,
        });

        let deadline = Instant::now() + Duration::from_secs(GRACEFUL_DRAIN_SECS);
        while self.active_chunk_writes.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        match self.db.wal_checkpoint_truncate() {
            Ok(report) => info!(
                busy = report.busy,
                log_frames = report.log_frames,
                checkpointed = report.checkpointed_frames,
                "SQLite WAL checkpoint(TRUNCATE) completed prior to restart"
            ),
            Err(e) => warn!("SQLite WAL checkpoint failed during graceful restart: {e}"),
        }

        self.restart_notify.notify_waiters();
    }
}

/// Builds the Axum 0.8 router with all REST endpoints, `/ws`, and optional embedded Web UI.
pub fn build_router(state: Arc<AppState>) -> Router {
    let protected_router = Router::new()
        .route("/api/auth/logout", post(handle_logout))
        .route("/api/system/status", get(handle_system_status))
        .route("/api/system/restart", post(handle_system_restart))
        .route(
            "/api/vaults",
            get(handle_list_vaults).post(handle_create_vault),
        )
        .route(
            "/api/vaults/{vault_id}/devices/{device_id}",
            put(handle_grant_vault_access),
        )
        .route(
            "/api/vaults/{vault_id}/chunks/exists",
            post(handle_check_chunks_exists).layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route(
            "/api/vaults/{vault_id}/chunks/{hash_hex}",
            get(handle_get_chunk)
                .put(handle_put_chunk)
                .head(handle_probe_chunk)
                .layer(DefaultBodyLimit::max(MAX_CHUNK_UPLOAD_BYTES)),
        )
        .route(
            "/api/snapshots/{snapshot_id}/manifest",
            get(handle_get_snapshot_manifest),
        )
        .route(
            "/api/vaults/{vault_id}/snapshots",
            post(handle_create_snapshot).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route("/api/snapshots", get(handle_list_snapshots))
        .route(
            "/api/snapshots/{snapshot_id}/file",
            get(handle_snapshot_file_download),
        )
        .route("/api/scrub/trigger", post(handle_scrub_trigger))
        .route("/api/peers", get(handle_list_peers))
        .route("/api/peers/heartbeat", post(handle_peer_heartbeat))
        .route("/api/peers/pair-requests", get(handle_list_pair_requests))
        .route(
            "/api/peers/pair-requests/{request_id}/respond",
            post(handle_respond_pair_request),
        )
        .route("/api/peers/{device_id}", delete(handle_delete_peer))
        .route(
            "/api/clipboard",
            post(handle_clipboard_push).layer(DefaultBodyLimit::max(MAX_CLIPBOARD_BODY_BYTES)),
        )
        .route("/api/transfers/init", post(handle_transfer_init))
        .route(
            "/api/transfers/{transfer_id}/parts/{part_index}",
            put(handle_transfer_part).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route(
            "/api/transfers/{transfer_id}/status",
            get(handle_transfer_status),
        )
        .route(
            "/api/transfers/{transfer_id}/commit",
            post(handle_transfer_commit),
        )
        .route("/api/clipboard/history", get(handle_clipboard_history))
        .route("/api/capabilities", get(handle_get_capabilities))
        .route(
            "/api/groups",
            get(handle_list_groups).post(handle_create_group),
        )
        .route(
            "/api/groups/{group_id}",
            patch(handle_update_group).delete(handle_delete_group),
        )
        .route(
            "/api/groups/{group_id}/members",
            get(handle_list_group_members).post(handle_add_group_member),
        )
        .route(
            "/api/groups/{group_id}/members/{device_id}",
            delete(handle_remove_group_member),
        )
        .route(
            "/api/groups/{group_id}/rules",
            get(handle_list_group_rules).post(handle_create_group_rule),
        )
        .route(
            "/api/rules/{rule_id}",
            patch(handle_update_group_rule).delete(handle_delete_group_rule),
        )
        .route(
            "/api/rules/{rule_id}/bindings",
            get(handle_list_rule_bindings).post(handle_create_rule_binding),
        )
        .route(
            "/api/rules/{rule_id}/bindings/{binding_id}",
            delete(handle_delete_rule_binding),
        )
        .route("/api/mirror-roots", get(handle_list_mirror_roots))
        .route(
            "/api/mirror-roots/register",
            post(handle_register_mirror_root),
        )
        .route(
            "/api/mirror-roots/{root_id}",
            delete(handle_revoke_mirror_root),
        )
        .route(
            "/api/vaults/{vault_id}/chunks/check",
            post(handle_check_chunks),
        )
        .route(
            "/api/bindings/{binding_id}/leases",
            post(handle_acquire_sync_lease),
        )
        .route(
            "/api/bindings/{binding_id}/catalog/commit",
            post(handle_commit_catalog).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route(
            "/api/bindings/{binding_id}/catalog/operations/{operation_id}",
            get(handle_get_catalog_operation),
        )
        .route(
            "/api/bindings/{binding_id}/catalog",
            get(handle_list_catalog_entries),
        )
        .route(
            "/api/bindings/{binding_id}/versions",
            get(handle_list_catalog_versions),
        )
        .route(
            "/api/bindings/{binding_id}/versions/{version_id}/download",
            get(handle_download_catalog_version),
        )
        .route(
            "/api/debug/config",
            get(handle_get_debug_config).post(handle_set_debug_config),
        )
        .route(
            "/api/debug/logs",
            get(handle_get_debug_logs).delete(handle_clear_debug_logs),
        )
        .route(
            "/api/debug/logs/client",
            post(handle_report_client_log).layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route("/ws", get(handle_ws_upgrade))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            require_auth_middleware,
        ));

    let api_router = Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/peers/pair", post(handle_pair_peer))
        .route(
            "/api/peers/session/challenge",
            post(handle_peer_session_challenge),
        )
        .route("/api/peers/session", post(handle_peer_session_renew))
        .route(
            "/api/peers/pair-request",
            post(handle_pair_request_initiate),
        )
        .route("/api/peers/pair-status", post(handle_pair_request_status))
        .merge(protected_router)
        .layer(CookieManagerLayer::new())
        .with_state(state);

    api_router
}

// =============================================================================
// Auth, Drain Guard & Boundary Error Mapping
// =============================================================================

// `Response` is the ready-to-send rejection body; boxing it would only add an allocation per request.
#[allow(clippy::result_large_err)]
fn require_accepting_tasks(state: &AppState) -> Result<(), Response> {
    if state.accepting_tasks.load(Ordering::SeqCst) {
        Ok(())
    } else {
        Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "server_restarting" })),
        )
            .into_response())
    }
}

#[allow(clippy::result_large_err)]
fn require_auth(state: &AppState, cookies: &Cookies) -> Result<SessionInfo, Response> {
    let token = cookies
        .get(SESSION_COOKIE_NAME)
        .map(|c| c.value().to_string());
    match token {
        Some(t) => {
            let token_hash = hex::encode(blake3_hash(t.as_bytes()));
            match state
                .db
                .is_session_revoked(&token_hash, unix_timestamp_secs())
            {
                Ok(true) => return Err(unauthorized_response()),
                Ok(false) => {}
                Err(err) => return Err(map_domain_error(err)),
            }
            let session = state
                .auth
                .validate_session(&t)
                .ok_or_else(unauthorized_response)?;
            if let Some(device_id) = session.device_id.as_deref() {
                match state.db.get_trusted_peer(device_id) {
                    Ok(Some(_)) => {}
                    Ok(None) => return Err(unauthorized_response()),
                    Err(err) => return Err(map_domain_error(err)),
                }
            }
            Ok(session)
        }
        _ => Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "unauthorized" })),
        )
            .into_response()),
    }
}

#[allow(clippy::result_large_err)]
fn require_admin(session: &SessionInfo) -> Result<(), Response> {
    if session.is_admin {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "admin_session_required" })),
        )
            .into_response())
    }
}

#[allow(clippy::result_large_err)]
fn require_session_device(session: &SessionInfo, device_id: &str) -> Result<(), Response> {
    if session.is_admin || session.device_id.as_deref() == Some(device_id) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "peer_identity_mismatch" })),
        )
            .into_response())
    }
}

async fn require_auth_middleware(
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
    mut request: Request,
    next: Next,
) -> Response {
    match require_auth(&state, &cookies) {
        Ok(session) => {
            request.extensions_mut().insert(session);
            next.run(request).await
        }
        Err(response) => response,
    }
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "unauthorized" })),
    )
        .into_response()
}

fn map_domain_error(err: LanVaultError) -> Response {
    let status = match &err {
        LanVaultError::InvalidInput(_) => StatusCode::BAD_REQUEST,
        LanVaultError::NotFound(_) => StatusCode::NOT_FOUND,
        LanVaultError::QuotaExceeded { .. } => StatusCode::INSUFFICIENT_STORAGE,
        LanVaultError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
        LanVaultError::Auth(_) => StatusCode::UNAUTHORIZED,
        LanVaultError::InvalidState(_) => StatusCode::CONFLICT,
        LanVaultError::CorruptedChunk { .. } => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let public_error = match status {
        StatusCode::BAD_REQUEST
        | StatusCode::NOT_FOUND
        | StatusCode::CONFLICT
        | StatusCode::INSUFFICIENT_STORAGE
        | StatusCode::UNAUTHORIZED
        | StatusCode::TOO_MANY_REQUESTS => err.to_string(),
        _ => {
            warn!(error = %err, "internal request failure");
            "internal_server_error".to_string()
        }
    };
    (status, Json(serde_json::json!({ "error": public_error }))).into_response()
}

fn validate_snapshot_id(raw: &str) -> Result<(), LanVaultError> {
    if raw.is_empty()
        || raw.len() > 128
        || !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(LanVaultError::InvalidInput(
            "snapshot_id must be 1..=128 ASCII alphanumeric, '-' or '_' characters".to_string(),
        ));
    }
    Ok(())
}

fn normalize_rel_path(path: &StdPath) -> String {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn decode_hex_32(hex_str: &str, field_name: &str) -> Result<[u8; 32], LanVaultError> {
    let bytes = hex::decode(hex_str)
        .map_err(|e| LanVaultError::InvalidInput(format!("invalid {field_name} hex: {e}")))?;
    bytes.try_into().map_err(|v: Vec<u8>| {
        LanVaultError::InvalidInput(format!(
            "{field_name} must be 32 bytes (64 hex chars), got {} bytes",
            v.len()
        ))
    })
}

pub fn validate_relative_snapshot_path(raw: &str) -> Result<PathBuf, LanVaultError> {
    if raw.is_empty() || raw.contains('\0') {
        return Err(LanVaultError::InvalidInput(
            "snapshot file path must be non-empty".to_string(),
        ));
    }
    if raw.starts_with('/') || raw.starts_with('\\') || raw.contains('\\') {
        return Err(LanVaultError::InvalidInput(
            "absolute or backslash snapshot file paths are forbidden".to_string(),
        ));
    }
    let path = StdPath::new(raw);
    if path.is_absolute() {
        return Err(LanVaultError::InvalidInput(
            "absolute snapshot file paths are forbidden".to_string(),
        ));
    }

    let mut clean = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(LanVaultError::InvalidInput(
                    "path traversal is forbidden".to_string(),
                ));
            }
        }
    }

    if clean.as_os_str().is_empty() {
        return Err(LanVaultError::InvalidInput(
            "snapshot file path must reference a file".to_string(),
        ));
    }
    Ok(clean)
}

fn manifest_root_hash(files: &[SnapshotManifestFile]) -> Result<[u8; 32], LanVaultError> {
    let mut tree_input = Vec::new();
    for file in files {
        tree_input.extend_from_slice(file.rel_path.as_bytes());
        tree_input.push(0);
        tree_input.extend_from_slice(&file.size.to_le_bytes());
        tree_input.extend_from_slice(&file.mtime_ns.to_le_bytes());
        for hash in &file.chunk_hashes_hex {
            tree_input.extend_from_slice(&decode_hex_32(hash, "chunk_hashes_hex")?);
        }
    }
    Ok(blake3_hash(&tree_input))
}

// =============================================================================
// Request / Response DTOs (aligned with ARCHITECTURE_CONTRACTS.md §3.1)
// =============================================================================

#[derive(Debug, Deserialize)]
struct LoginRequest {
    password: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemStatusResponse {
    pub os: String,
    pub keep_alive_level: String,
    pub lan_ip: String,
    pub port: u16,
    pub mdns_active: bool,
    pub uptime_secs: u64,
    pub throughput_bps: u64,
    pub bit_rot_health: String,
    pub corrupted_chunks: u64,
    pub vault_used_bytes: u64,
    pub vault_quota_bytes: u64,
}

#[derive(Debug, Serialize)]
struct VolumeDto {
    volume_id: String,
    mount_path: String,
    volume_guid: Option<String>,
    role: VolumeRole,
    is_online: bool,
}

#[derive(Debug, Serialize)]
struct VaultDto {
    vault_id: String,
    name: String,
    pool_mode: PoolMode,
    quota_bytes: u64,
    used_bytes: u64,
    e2ee_enabled: bool,
    volumes: Vec<VolumeDto>,
}

#[derive(Debug, Deserialize)]
struct CreateVaultRequest {
    name: String,
    mount_path: String,
    volume_guid: Option<String>,
    pool_mode: PoolMode,
    quota_bytes: u64,
    e2ee_enabled: bool,
    #[serde(default)]
    extra_volumes: Vec<ExtraVolumeSpec>,
}

#[derive(Debug, Deserialize)]
struct SnapshotQuery {
    vault_id: Option<String>,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_snapshot_limit")]
    limit: usize,
}

fn default_snapshot_limit() -> usize {
    200
}

#[derive(Debug, Deserialize)]
struct SnapshotFileQuery {
    vault_id: String,
    path: String,
}

#[derive(Debug, Serialize)]
struct SnapshotDto {
    snapshot_id: String,
    vault_id: String,
    rule_id: String,
    device_id: String,
    retention_tier: String,
    file_count: u64,
    total_bytes: u64,
    created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifestFile {
    pub rel_path: String,
    pub size: u64,
    #[serde(default)]
    pub mtime_ns: i64,
    pub chunk_hashes_hex: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub snapshot_id: String,
    pub vault_id: String,
    #[serde(default)]
    pub rule_id: String,
    pub device_id: String,
    pub root_tree_hash_hex: String,
    pub e2ee_enabled: bool,
    pub created_at: i64,
    pub files: Vec<SnapshotManifestFile>,
}

pub fn load_snapshot_manifest(
    store: &VaultStore,
    snapshot: &SnapshotRecord,
) -> lanvault_core::Result<Option<SnapshotManifest>> {
    validate_snapshot_id(&snapshot.snapshot_id)?;
    let filename = format!("{}.json", snapshot.snapshot_id);
    let mut invalid = false;
    for root in store.manifest_backup_roots(&snapshot.vault_id)? {
        let bytes = match fs::read(root.join("manifests").join(&filename)) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                invalid = true;
                continue;
            }
        };
        if let Ok(candidate) = serde_json::from_slice::<SnapshotManifest>(&bytes) {
            if candidate.snapshot_id == snapshot.snapshot_id
                && candidate.vault_id == snapshot.vault_id
                && candidate.device_id == snapshot.device_id
                && candidate.rule_id == snapshot.rule_id
                && candidate.files.iter().all(|file| {
                    validate_relative_snapshot_path(&file.rel_path).is_ok()
                })
                && manifest_root_hash(&candidate.files).ok() == Some(snapshot.root_tree_hash)
            {
                return Ok(Some(candidate));
            }
        }
        invalid = true;
    }
    if invalid {
        return Err(LanVaultError::InvalidState(
            "no verified manifest replica".to_string(),
        ));
    }
    Ok(None)
}

#[derive(Debug, Deserialize)]
struct CreateSnapshotRequest {
    snapshot_id: Option<String>,
    #[serde(default)]
    rule_id: String,
    device_id: String,
    root_tree_hash_hex: Option<String>,
    #[serde(default)]
    retention_tier: Option<RetentionTier>,
    files: Vec<SnapshotManifestFile>,
}

#[derive(Debug, Deserialize)]
struct ScrubQuery {
    vault_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PairPinRequest {
    pin: String,
    client_ed25519_pubkey_hex: String,
    client_x25519_pubkey_hex: String,
    client_ed25519_signature_hex: String,
    device_name: String,
    os_type: String,
}

#[derive(Debug, Deserialize)]
struct PairRequestInitiateBody {
    nonce: String,
    client_ed25519_pubkey_hex: String,
    client_x25519_pubkey_hex: String,
    client_ed25519_signature_hex: String,
    device_name: String,
    os_type: String,
}

#[derive(Debug, Deserialize)]
struct PairRequestStatusBody {
    request_id: String,
    poll_token: String,
}

#[derive(Debug, Deserialize)]
struct RespondPairRequestBody {
    accept: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairPinResponse {
    pub device_id: String,
    pub server_ed25519_pubkey_hex: String,
    pub server_x25519_pubkey_hex: String,
    pub server_ed25519_signature_hex: String,
    pub clipboard_key_envelope_hex: String,
    pub session_cookie: Option<String>,
}

#[derive(Debug, Serialize)]
struct PeerDto {
    device_id: String,
    device_name: String,
    os_type: String,
    last_ip: Option<String>,
    last_port: Option<u16>,
    auto_connect: bool,
    online: bool,
    paired_at: i64,
}

#[derive(Debug, Deserialize)]
struct ClipboardRequest {
    #[serde(default)]
    client_msg_id: Option<String>,
    target_device_id: Option<String>,
    #[serde(default)]
    target_device_ids: Option<Vec<String>>,
    kind: ClipboardKind,
    content: String,
    filename: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClipboardHistoryQuery {
    limit: Option<usize>,
    session_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HeartbeatRequest {
    last_port: Option<u16>,
}

/// 会话续期请求：服务端重启后内存会话表清空，已配对设备用自己持有的 Ed25519
/// 私钥对 `renew_nonce` 签名，即可**无感换发新会话**，不必重新走一遍配对流程。
#[derive(Debug, Deserialize)]
struct PeerSessionChallengeRequest {
    device_id: String,
}

#[derive(Debug, Deserialize)]
struct PeerSessionRenewRequest {
    device_id: String,
    client_ed25519_pubkey_hex: String,
    client_ed25519_signature_hex: String,
    #[serde(alias = "challenge")]
    nonce: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClipboardEnvelope {
    wire_hex: String,
}

// =============================================================================
// REST Handlers
// =============================================================================

async fn handle_login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(LanConnectionInfo { addr, secure }): ConnectInfo<LanConnectionInfo>,
    cookies: Cookies,
    Json(req): Json<LoginRequest>,
) -> Response {
    let auth = Arc::clone(&state.auth);
    let client_ip = addr.ip();
    let password = req.password;

    let outcome = match tokio::task::spawn_blocking(move || auth.login(client_ip, &password)).await
    {
        Ok(outcome) => outcome,
        Err(err) => {
            warn!("Argon2id login blocking task failed: {err}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "authentication_failed" })),
            )
                .into_response();
        }
    };

    match outcome {
        LoginOutcome::Authenticated {
            token,
            expires_in_secs,
        } => {
            cookies.add(build_session_cookie(token, secure));
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "authenticated": true,
                    "expires_in_secs": expires_in_secs,
                })),
            )
                .into_response()
        }
        LoginOutcome::InvalidPassword { remaining_attempts } => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "invalid_password",
                "remaining_attempts": remaining_attempts,
            })),
        )
            .into_response(),
        LoginOutcome::LockedOut { retry_after_secs } => {
            let mut resp = (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "error": "locked_out",
                    "retry_after_secs": retry_after_secs,
                })),
            )
                .into_response();
            if let Ok(val) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                resp.headers_mut()
                    .insert(HeaderName::from_static("retry-after"), val);
            }
            resp
        }
    }
}

async fn handle_logout(
    State(state): State<Arc<AppState>>,
    ConnectInfo(LanConnectionInfo { secure, .. }): ConnectInfo<LanConnectionInfo>,
    Extension(_session): Extension<SessionInfo>,
    cookies: Cookies,
) -> Response {
    let token = cookies
        .get(SESSION_COOKIE_NAME)
        .map(|cookie| cookie.value().to_string())
        .ok_or_else(unauthorized_response);
    let token = match token {
        Ok(token) => token,
        Err(resp) => return resp,
    };
    state.auth.revoke_session(&token);
    let token_hash = hex::encode(blake3_hash(token.as_bytes()));
    if let Err(err) = state.db.revoke_session(
        &token_hash,
        unix_timestamp_secs().saturating_add(SESSION_TTL_SECS as i64),
    ) {
        return map_domain_error(err);
    }
    cookies.add(build_clear_session_cookie(secure));
    (
        StatusCode::OK,
        Json(serde_json::json!({ "authenticated": false })),
    )
        .into_response()
}

async fn handle_system_status(
    State(state): State<Arc<AppState>>,
    Extension(_session): Extension<SessionInfo>,
) -> Response {
    match state.build_system_status() {
        Ok(status) => (StatusCode::OK, Json(status)).into_response(),
        Err(err) => map_domain_error(err),
    }
}

async fn handle_system_restart(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
) -> Response {
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    if !state.accepting_tasks.swap(false, Ordering::SeqCst) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "server_restarting" })),
        )
            .into_response();
    }
    let state_clone = Arc::clone(&state);
    tokio::spawn(async move {
        state_clone.execute_graceful_restart().await;
    });
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "status": "restarting",
            "drain_secs": GRACEFUL_DRAIN_SECS,
        })),
    )
        .into_response()
}

async fn handle_list_vaults(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
) -> Response {
    let db = Arc::clone(&state.db);
    let is_admin = session.is_admin;
    let join_res = tokio::task::spawn_blocking(move || -> lanvault_core::Result<Vec<VaultDto>> {
        let vaults = db.list_vaults()?;
        let mut by_vault: std::collections::HashMap<String, Vec<VolumeDto>> =
            std::collections::HashMap::new();
        if is_admin {
            for vol in db.list_all_volumes()? {
                by_vault.entry(vol.vault_id).or_default().push(VolumeDto {
                    volume_id: vol.volume_id,
                    mount_path: vol.mount_path,
                    volume_guid: vol.volume_guid,
                    role: vol.role,
                    is_online: vol.is_online,
                });
            }
        }
        Ok(vaults
            .into_iter()
            .map(|v| {
                let volumes = by_vault.remove(&v.vault_id).unwrap_or_default();
                VaultDto {
                    vault_id: v.vault_id,
                    name: v.name,
                    pool_mode: v.pool_mode,
                    quota_bytes: v.quota_bytes,
                    used_bytes: v.used_bytes,
                    e2ee_enabled: v.e2ee_enabled,
                    volumes,
                }
            })
            .collect())
    })
    .await;

    match join_res {
        Ok(Ok(result)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "vaults": result })),
        )
            .into_response(),
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => {
            warn!("list_vaults blocking task failed: {err}");
            map_domain_error(LanVaultError::InvalidState(
                "list_vaults task failed".to_string(),
            ))
        }
    }
}

async fn handle_grant_vault_access(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path((vault_id, device_id)): Path<(String, String)>,
) -> Response {
    if let Err(response) = require_admin(&session) {
        return response;
    }
    if let Err(response) = require_accepting_tasks(&state) {
        return response;
    }
    let result = tokio::task::spawn_blocking(move || {
        if state.db.get_vault(&vault_id)?.is_none() {
            return Err(LanVaultError::NotFound(format!("vault {vault_id}")));
        }
        let peer = state.db.get_trusted_peer(&device_id)?;
        if !peer.is_some_and(|peer| peer.revoked_at.is_none()) {
            return Err(LanVaultError::NotFound(format!("trusted peer {device_id}")));
        }
        state
            .db
            .grant_vault_access(&vault_id, &device_id, unix_timestamp_secs())
    })
    .await;
    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(error)) => map_domain_error(error),
        Err(error) => internal_error(format!("vault grant task failed: {error}")),
    }
}

async fn handle_create_vault(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Json(req): Json<CreateVaultRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    if !req.e2ee_enabled {
        return map_domain_error(LanVaultError::InvalidInput(
            "vaults must keep end-to-end encryption enabled".to_string(),
        ));
    }
    let Ok(_upload_permit) = state.upload_semaphore.acquire().await else {
        return map_domain_error(LanVaultError::InvalidState(
            "upload semaphore closed".to_string(),
        ));
    };
    let _write_guard = ActiveChunkWriteGuard::new(Arc::clone(&state));
    let vault_store = Arc::clone(&state.vault_store);
    let join_res = tokio::task::spawn_blocking(move || {
        vault_store.create_vault(
            req.name,
            req.mount_path,
            req.volume_guid,
            req.pool_mode,
            req.quota_bytes,
            req.e2ee_enabled,
            req.extra_volumes,
        )
    })
    .await;
    match join_res {
        Ok(Ok((vault, volumes))) => {
            let dto = VaultDto {
                vault_id: vault.vault_id,
                name: vault.name,
                pool_mode: vault.pool_mode,
                quota_bytes: vault.quota_bytes,
                used_bytes: vault.used_bytes,
                e2ee_enabled: vault.e2ee_enabled,
                volumes: volumes
                    .into_iter()
                    .map(|vol| VolumeDto {
                        volume_id: vol.volume_id,
                        mount_path: vol.mount_path,
                        volume_guid: vol.volume_guid,
                        role: vol.role,
                        is_online: vol.is_online,
                    })
                    .collect(),
            };
            (StatusCode::CREATED, Json(dto)).into_response()
        }
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => {
            warn!("create_vault blocking task failed: {err}");
            map_domain_error(LanVaultError::InvalidState(
                "create_vault task failed".to_string(),
            ))
        }
    }
}

#[derive(Debug, Deserialize)]
struct CheckChunksExistsRequest {
    hashes_hex: Vec<String>,
}

fn internal_error(msg: String) -> Response {
    warn!(error = %msg, "internal server error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": msg })),
    )
        .into_response()
}

fn validate_safe_id(raw: &str, label: &str) -> lanvault_core::Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > 128
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(LanVaultError::InvalidInput(format!("invalid {label}")));
    }
    Ok(trimmed.to_string())
}

#[allow(clippy::result_large_err)]
fn require_vault_access(
    state: &AppState,
    session: &SessionInfo,
    vault_id: &str,
) -> Result<Option<String>, Response> {
    if session.is_admin {
        return Ok(None);
    }
    let Some(dev_id) = session
        .device_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Err(unauthorized_response());
    };
    if !state.auth.validate_session_device_active(dev_id) {
        return Err(unauthorized_response());
    }
    match state.db.is_device_authorized_for_vault(vault_id, dev_id) {
        Ok(true) => Ok(Some(dev_id.to_string())),
        Ok(false) => match state.db.is_group_source_for_vault(dev_id, vault_id) {
            Ok(true) => Ok(Some(dev_id.to_string())),
            Ok(false) => Err((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": "vault_access_denied" })),
            )
                .into_response()),
            Err(err) => Err(map_domain_error(err)),
        },
        Err(err) => Err(map_domain_error(err)),
    }
}

async fn handle_check_chunks_exists(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(vault_id): Path<String>,
    Json(req): Json<CheckChunksExistsRequest>,
) -> Response {
    if let Err(resp) = require_vault_access(&state, &session, &vault_id) {
        return resp;
    }
    if req.hashes_hex.len() > 1000 {
        return map_domain_error(LanVaultError::InvalidInput(
            "hashes_hex batch exceeds limit of 1000".to_string(),
        ));
    }
    let vault_store = Arc::clone(&state.vault_store);
    let vid = vault_id.clone();
    let hashes_hex = req.hashes_hex;
    let join_res = tokio::task::spawn_blocking(move || -> lanvault_core::Result<Vec<String>> {
        let mut missing = Vec::new();
        for h_hex in hashes_hex {
            let Ok(h) = decode_hex_32(&h_hex, "hash_hex") else {
                missing.push(h_hex);
                continue;
            };
            if !vault_store.has_verified_chunk(&vid, &h)? {
                missing.push(h_hex);
            }
        }
        Ok(missing)
    })
    .await;

    match join_res {
        Ok(Ok(missing)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "missing": missing })),
        )
            .into_response(),
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => internal_error(format!("chunks exists task failed: {err}")),
    }
}

async fn handle_get_chunk(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path((vault_id, hash_hex)): Path<(String, String)>,
) -> Response {
    let peer_device_id = match require_vault_access(&state, &session, &vault_id) {
        Ok(dev) => dev,
        Err(resp) => return resp,
    };
    let hash = match decode_hex_32(&hash_hex, "hash_hex") {
        Ok(h) => h,
        Err(err) => return map_domain_error(err),
    };
    let vault_store = Arc::clone(&state.vault_store);
    let db = Arc::clone(&state.db);
    let vid = vault_id.clone();
    let res =
        tokio::task::spawn_blocking(move || -> lanvault_core::Result<(Vec<u8>, &'static str)> {
            if let Some(dev_id) = peer_device_id.as_deref() {
                if !db.is_chunk_accessible_to_device(&vid, &hash, dev_id)? {
                    return Err(LanVaultError::Auth(
                        "chunk is not referenced by any snapshot owned by this device".to_string(),
                    ));
                }
            }
            let bytes = vault_store.get_chunk(&vid, &hash)?;
            let comp = db.get_chunk_compression(&vid, &hash);
            let enc: &'static str = if comp.eq_ignore_ascii_case("raw") {
                "xchacha20poly1305+raw"
            } else {
                "xchacha20poly1305+zstd"
            };
            Ok((bytes, enc))
        })
        .await;

    match res {
        Ok(Ok((bytes, enc))) => (
            StatusCode::OK,
            [
                ("content-type", "application/octet-stream"),
                ("x-lanvault-encoding", enc),
            ],
            bytes,
        )
            .into_response(),
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => internal_error(format!("get chunk task failed: {err}")),
    }
}

async fn handle_get_snapshot_manifest(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(snapshot_id): Path<String>,
) -> Response {
    let snapshot_id = match validate_safe_id(&snapshot_id, "snapshot_id") {
        Ok(id) => id,
        Err(err) => return map_domain_error(err),
    };
    let db = Arc::clone(&state.db);
    let vault_store = Arc::clone(&state.vault_store);
    let sid = snapshot_id.clone();
    let res = tokio::task::spawn_blocking(move || -> lanvault_core::Result<serde_json::Value> {
        let snap = db
            .get_snapshot(&sid)?
            .ok_or_else(|| LanVaultError::NotFound(format!("snapshot {sid}")))?;
        if !session.is_admin && session.device_id.as_deref() != Some(snap.device_id.as_str()) {
            return Err(LanVaultError::Auth(
                "snapshot belongs to another device".to_string(),
            ));
        }
        load_snapshot_manifest(&vault_store, &snap)?
            .map(|manifest| {
                serde_json::json!({
                    "vault_id": snap.vault_id,
                    "snapshot_id": snap.snapshot_id,
                    "manifest": manifest,
                })
            })
            .ok_or_else(|| {
                LanVaultError::NotFound(format!("manifest for snapshot {sid}"))
            })
    })
    .await;

    match res {
        Ok(Ok(val)) => (StatusCode::OK, Json(val)).into_response(),
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => internal_error(format!("get snapshot manifest failed: {err}")),
    }
}

async fn handle_put_chunk(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path((vault_id, hash_hex)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_vault_access(&state, &session, &vault_id) {
        return resp;
    }

    let expected_hash = match decode_hex_32(&hash_hex, "hash_hex") {
        Ok(h) => h,
        Err(err) => return map_domain_error(err),
    };
    let encoding_hdr = headers
        .get("x-lanvault-encoding")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("xchacha20poly1305+zstd");
    let compression_tag = match encoding_hdr {
        "xchacha20poly1305+zstd" => "zstd",
        "xchacha20poly1305+raw" => "raw",
        _ => {
            return map_domain_error(LanVaultError::InvalidInput(
                "chunk encoding must be xchacha20poly1305+zstd or xchacha20poly1305+raw"
                    .to_string(),
            ));
        }
    };

    let raw_size: u64 = match headers.get("x-lanvault-raw-size") {
        Some(val) => match val.to_str().ok().and_then(|s| s.parse::<u64>().ok()) {
            Some(sz) => sz,
            None => {
                return map_domain_error(LanVaultError::InvalidInput(
                    "invalid X-LanVault-Raw-Size header".to_string(),
                ));
            }
        },
        None => body.len() as u64,
    };
    if raw_size > CHUNK_MAX_SIZE as u64 {
        return map_domain_error(LanVaultError::InvalidInput(format!(
            "X-LanVault-Raw-Size exceeds maximum chunk size of {CHUNK_MAX_SIZE} bytes"
        )));
    }

    let _write_guard = ActiveChunkWriteGuard::new(Arc::clone(&state));
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }

    let stored_size = body.len() as u64;
    let vault_store = Arc::clone(&state.vault_store);
    let vid = vault_id.clone();

    let comp_tag_owned = compression_tag.to_string();
    let join_res = tokio::task::spawn_blocking(move || {
        vault_store.put_chunk_with_compression(
            &vid,
            &expected_hash,
            raw_size,
            &body,
            &comp_tag_owned,
        )
    })
    .await;

    match join_res {
        Ok(Ok(is_new)) => {
            state
                .transfer_bytes
                .fetch_add(stored_size, Ordering::Relaxed);
            let status = if is_new {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (
                status,
                Json(serde_json::json!({
                    "stored": true,
                    "deduplicated": !is_new,
                    "vault_id": vault_id,
                    "hash": hash_hex,
                    "stored_size": stored_size,
                })),
            )
                .into_response()
        }
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => {
            warn!("put_chunk blocking task failed: {err}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "chunk_write_failed" })),
            )
                .into_response()
        }
    }
}

async fn handle_probe_chunk(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path((vault_id, hash_hex)): Path<(String, String)>,
) -> Response {
    if let Err(response) = require_accepting_tasks(&state) {
        return response;
    }
    if let Err(response) = require_vault_access(&state, &session, &vault_id) {
        return response;
    }
    let hash = match decode_hex_32(&hash_hex, "hash_hex") {
        Ok(hash) => hash,
        Err(err) => return map_domain_error(err),
    };
    let store = Arc::clone(&state.vault_store);
    let result =
        tokio::task::spawn_blocking(move || store.resume_chunk(&vault_id, &hash)).await;
    let mut response = match result {
        Ok(Ok(true)) => StatusCode::OK.into_response(),
        Ok(Ok(false)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => internal_error(format!("chunk availability task failed: {err}")),
    };
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    response
}

async fn handle_list_snapshots(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Query(query): Query<SnapshotQuery>,
) -> Response {
    let db = Arc::clone(&state.db);
    let vault_id = query.vault_id;
    let limit = query.limit.clamp(1, 1000);
    let offset = query.offset;
    let is_admin = session.is_admin;
    let session_device_id = session.device_id;

    let join_res =
        tokio::task::spawn_blocking(move || -> lanvault_core::Result<Vec<SnapshotDto>> {
            let snaps = if is_admin {
                db.list_snapshots_filtered_paginated(vault_id.as_deref(), None, limit, offset)?
            } else if let Some(dev_id) = session_device_id.as_deref() {
                db.list_snapshots_filtered_paginated(
                    vault_id.as_deref(),
                    Some(dev_id),
                    limit,
                    offset,
                )?
            } else {
                Vec::new()
            };
            Ok(snaps
                .into_iter()
                .map(|s| SnapshotDto {
                    snapshot_id: s.snapshot_id,
                    vault_id: s.vault_id,
                    rule_id: s.rule_id,
                    device_id: s.device_id,
                    retention_tier: s.retention_tier.as_str().to_string(),
                    file_count: s.file_count,
                    total_bytes: s.total_bytes,
                    created_at: s.created_at,
                })
                .collect())
        })
        .await;

    match join_res {
        Ok(Ok(dtos)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "snapshots": dtos })),
        )
            .into_response(),
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => {
            warn!("list_snapshots blocking task failed: {err}");
            map_domain_error(LanVaultError::InvalidState(
                "list_snapshots task failed".to_string(),
            ))
        }
    }
}

async fn handle_create_snapshot(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(vault_id): Path<String>,
    Json(req): Json<CreateSnapshotRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }

    let snapshot_id = match req.snapshot_id {
        Some(id) => {
            if let Err(err) = validate_snapshot_id(&id) {
                return map_domain_error(err);
            }
            id
        }
        None => uuid::Uuid::new_v4().to_string(),
    };

    if req.device_id.trim().is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "device_id must not be empty".to_string(),
        ));
    }
    if req.rule_id.trim().is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "rule_id must not be empty".to_string(),
        ));
    }
    if let Err(resp) = require_session_device(&session, &req.device_id) {
        return resp;
    }
    if let Err(resp) = require_vault_access(&state, &session, &vault_id) {
        return resp;
    }

    let explicit_root_hash = match req.root_tree_hash_hex.as_deref() {
        Some(hex_str) => match decode_hex_32(hex_str, "root_tree_hash_hex") {
            Ok(h) => Some(h),
            Err(err) => return map_domain_error(err),
        },
        None => None,
    };

    let mut normalized_files = Vec::with_capacity(req.files.len());
    let mut unique_hashes: HashSet<[u8; 32]> = HashSet::new();
    let mut unique_paths = HashSet::new();
    for file in req.files {
        let clean_path = match validate_relative_snapshot_path(&file.rel_path) {
            Ok(p) => normalize_rel_path(&p),
            Err(err) => return map_domain_error(err),
        };
        if !unique_paths.insert(clean_path.clone()) {
            return map_domain_error(LanVaultError::InvalidInput(format!(
                "duplicate snapshot path: {clean_path}"
            )));
        }
        let mut normalized_hashes = Vec::with_capacity(file.chunk_hashes_hex.len());
        for h_hex in &file.chunk_hashes_hex {
            let decoded = match decode_hex_32(h_hex, "chunk_hashes_hex") {
                Ok(h) => h,
                Err(err) => return map_domain_error(err),
            };
            unique_hashes.insert(decoded);
            normalized_hashes.push(hex::encode(decoded));
        }
        normalized_files.push(SnapshotManifestFile {
            rel_path: clean_path,
            size: file.size,
            mtime_ns: file.mtime_ns,
            chunk_hashes_hex: normalized_hashes,
        });
    }
    normalized_files.sort_by(|left, right| left.rel_path.cmp(&right.rel_path));

    let _write_guard = ActiveChunkWriteGuard::new(Arc::clone(&state));
    let retention_tier = req.retention_tier.unwrap_or(RetentionTier::Daily);
    let rule_id = req.rule_id;
    let device_id = req.device_id;
    let db = Arc::clone(&state.db);
    let vault_store = Arc::clone(&state.vault_store);
    let vid = vault_id.clone();

    let join_res = tokio::task::spawn_blocking(move || -> lanvault_core::Result<SnapshotRecord> {
        let hashes: Vec<[u8; 32]> = unique_hashes.into_iter().collect();
        // Verify every referenced chunk before writing the manifest. A size-only check can
        // let same-size Bit-Rot corruption publish an unrecoverable snapshot.
        vault_store.validate_snapshot_chunks(&vid, &hashes)?;

        vault_store.with_write_lock(|| {
            let vault = db
                .get_vault(&vid)?
                .ok_or_else(|| LanVaultError::NotFound(format!("vault {vid}")))?;
            if db.get_trusted_peer(&device_id)?.is_none() {
                return Err(LanVaultError::NotFound(format!("trusted peer {device_id}")));
            }
            let manifest_roots = vault_store.manifest_backup_roots(&vid)?;

            let created_at = unix_timestamp_secs();
            let file_count = normalized_files.len() as u64;
            let total_bytes = normalized_files
                .iter()
                .fold(0u64, |acc, f| acc.saturating_add(f.size));

            let canonical_root = manifest_root_hash(&normalized_files)?;
            let root_tree_hash = match explicit_root_hash {
                Some(hash) if hash == canonical_root => hash,
                Some(_) => {
                    return Err(LanVaultError::InvalidInput(
                        "root_tree_hash_hex does not match the canonical manifest".to_string(),
                    ))
                }
                None => canonical_root,
            };

            let record = SnapshotRecord {
                snapshot_id: snapshot_id.clone(),
                vault_id: vid.clone(),
                rule_id: rule_id.clone(),
                device_id: device_id.clone(),
                root_tree_hash,
                retention_tier,
                file_count,
                total_bytes,
                created_at,
            };
            if let Some(existing) = db.get_snapshot(&snapshot_id)? {
                if existing.vault_id == record.vault_id
                    && existing.rule_id == record.rule_id
                    && existing.device_id == record.device_id
                    && existing.root_tree_hash == record.root_tree_hash
                    && existing.retention_tier == record.retention_tier
                    && existing.file_count == record.file_count
                    && existing.total_bytes == record.total_bytes
                {
                    return Ok(existing);
                }
                return Err(LanVaultError::InvalidInput(format!(
                    "snapshot_id {snapshot_id} already exists with different contents"
                )));
            }

            let manifest = SnapshotManifest {
                snapshot_id: snapshot_id.clone(),
                vault_id: vid.clone(),
                rule_id: rule_id.clone(),
                device_id: device_id.clone(),
                root_tree_hash_hex: hex::encode(root_tree_hash),
                e2ee_enabled: vault.e2ee_enabled,
                created_at,
                files: normalized_files,
            };

            let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
            let mut manifest_paths = Vec::with_capacity(manifest_roots.len());
            let write_manifests = (|| -> lanvault_core::Result<()> {
                for root_dir in &manifest_roots {
                    let manifests_dir = root_dir.join("manifests");
                    let staging_dir = root_dir.join("staging");
                    fs::create_dir_all(&manifests_dir)?;
                    fs::create_dir_all(&staging_dir)?;
                    let tmp_path =
                        staging_dir.join(format!("manifest.{}.tmp", uuid::Uuid::new_v4()));
                    let final_path = manifests_dir.join(format!("{snapshot_id}.json"));
                    {
                        let mut file = File::create(&tmp_path)?;
                        file.write_all(&manifest_bytes)?;
                        file.sync_all()?;
                    }
                    fs::rename(&tmp_path, &final_path)?;
                    manifest_paths.push(final_path);
                }
                Ok(())
            })();
            if let Err(err) = write_manifests {
                for path in &manifest_paths {
                    let _ = fs::remove_file(path);
                }
                return Err(err);
            }
            if let Err(err) = db.insert_snapshot_with_chunks(&record, &hashes) {
                for path in &manifest_paths {
                    let _ = fs::remove_file(path);
                }
                return Err(err);
            }

            Ok(record)
        })
    })
    .await;

    match join_res {
        Ok(Ok(s)) => (
            StatusCode::CREATED,
            Json(SnapshotDto {
                snapshot_id: s.snapshot_id,
                vault_id: s.vault_id,
                rule_id: s.rule_id,
                device_id: s.device_id,
                retention_tier: s.retention_tier.as_str().to_string(),
                file_count: s.file_count,
                total_bytes: s.total_bytes,
                created_at: s.created_at,
            }),
        )
            .into_response(),
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => {
            warn!("create_snapshot blocking task failed: {err}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "snapshot_commit_failed" })),
            )
                .into_response()
        }
    }
}

async fn handle_snapshot_file_download(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(snapshot_id): Path<String>,
    Query(query): Query<SnapshotFileQuery>,
) -> Response {
    if let Err(err) = validate_snapshot_id(&snapshot_id) {
        return map_domain_error(err);
    }

    let rel_path = match validate_relative_snapshot_path(&query.path) {
        Ok(p) => p,
        Err(err) => return map_domain_error(err),
    };
    let normalized_rel_path = normalize_rel_path(&rel_path);

    let filename = rel_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.replace(['"', '\r', '\n'], "_"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "download.bin".to_string());

    let db = Arc::clone(&state.db);
    let vault_store = Arc::clone(&state.vault_store);
    let vault_id = query.vault_id;
    let snap_id = snapshot_id.clone();

    let scan_vault_id = vault_id.clone();
    let join_res =
        tokio::task::spawn_blocking(move || -> lanvault_core::Result<(Vec<[u8; 32]>, bool)> {
            let vault_id = scan_vault_id;
            let snap = db
                .get_snapshot(&snap_id)?
                .ok_or_else(|| LanVaultError::NotFound(format!("snapshot {snap_id}")))?;
            if snap.vault_id != vault_id {
                return Err(LanVaultError::NotFound(format!(
                    "snapshot {snap_id} in vault {vault_id}"
                )));
            }
            if !session.is_admin && session.device_id.as_deref() != Some(snap.device_id.as_str()) {
                return Err(LanVaultError::NotFound(format!(
                    "snapshot {snap_id} in vault {vault_id}"
                )));
            }

            let vault = db
                .get_vault(&vault_id)?
                .ok_or_else(|| LanVaultError::NotFound(format!("vault {vault_id}")))?;

            let manifest = load_snapshot_manifest(&vault_store, &snap)?.ok_or_else(|| {
                LanVaultError::NotFound(format!("valid snapshot manifest for {snap_id}"))
            })?;

            let file_entry = manifest
                .files
                .iter()
                .find(|f| f.rel_path == normalized_rel_path)
                .ok_or_else(|| {
                    LanVaultError::NotFound(format!(
                        "file {normalized_rel_path} in snapshot {snap_id}"
                    ))
                })?;

            let mut chunk_hashes = Vec::with_capacity(file_entry.chunk_hashes_hex.len());
            for hash_hex in &file_entry.chunk_hashes_hex {
                chunk_hashes.push(decode_hex_32(hash_hex, "manifest chunk hash")?);
            }

            Ok((chunk_hashes, vault.e2ee_enabled))
        })
        .await;

    match join_res {
        Ok(Ok((chunk_hashes, e2ee_enabled))) => {
            let disposition = format!("attachment; filename=\"{filename}.lanvault-chunks\"");
            let stream_store = Arc::clone(&state.vault_store);
            let stream_vault_id = vault_id.clone();
            let chunk_stream = futures_util::stream::try_unfold(
                (chunk_hashes.into_iter(), stream_store, stream_vault_id),
                |(mut iter, store, vid)| async move {
                    let Some(hash) = iter.next() else {
                        return Ok::<_, std::io::Error>(None);
                    };
                    let store_bg = Arc::clone(&store);
                    let vid_bg = vid.clone();
                    let bytes =
                        tokio::task::spawn_blocking(move || store_bg.get_chunk(&vid_bg, &hash))
                            .await
                            .map_err(|e| {
                                std::io::Error::other(format!("chunk stream join error: {e}"))
                            })?
                            .map_err(|e| std::io::Error::other(e.to_string()))?;
                    let frame_len: u32 = bytes.len().try_into().map_err(|_| {
                        std::io::Error::other(format!(
                            "chunk exceeds u32 frame limit: {}",
                            bytes.len()
                        ))
                    })?;
                    let mut framed = Vec::with_capacity(4 + bytes.len());
                    framed.extend_from_slice(&frame_len.to_be_bytes());
                    framed.extend_from_slice(&bytes);
                    Ok(Some((axum::body::Bytes::from(framed), (iter, store, vid))))
                },
            );
            let mut resp = match axum::response::Response::builder()
                .status(StatusCode::OK)
                .body(axum::body::Body::from_stream(chunk_stream))
            {
                Ok(resp) => resp,
                Err(err) => {
                    warn!("failed to build streaming snapshot response: {err}");
                    return map_domain_error(LanVaultError::InvalidState(
                        "failed to build snapshot stream".to_string(),
                    ));
                }
            };
            resp.headers_mut().insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            if let Ok(val) = HeaderValue::from_str(&disposition) {
                resp.headers_mut().insert(CONTENT_DISPOSITION, val);
            }
            resp.headers_mut().insert(
                HeaderName::from_static("x-lanvault-encoded"),
                HeaderValue::from_static("true"),
            );
            resp.headers_mut().insert(
                HeaderName::from_static("x-lanvault-chunk-framing"),
                HeaderValue::from_static("u32be-aead-v1"),
            );
            if e2ee_enabled {
                resp.headers_mut().insert(
                    HeaderName::from_static("x-lanvault-e2ee"),
                    HeaderValue::from_static("true"),
                );
            }
            resp
        }
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => {
            warn!("snapshot file download blocking task failed: {err}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "snapshot_download_failed" })),
            )
                .into_response()
        }
    }
}

async fn handle_scrub_trigger(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Query(query): Query<ScrubQuery>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }

    let _write_guard = ActiveChunkWriteGuard::new(Arc::clone(&state));
    let state_clone = Arc::clone(&state);
    let vault_id = query.vault_id;

    let join_res = tokio::task::spawn_blocking(move || -> lanvault_core::Result<ScrubReport> {
        let target_vault_ids: Vec<String> = match vault_id {
            Some(vid) => vec![vid],
            None => state_clone
                .db
                .list_vaults()?
                .into_iter()
                .map(|v| v.vault_id)
                .collect(),
        };

        let mut aggregate = ScrubReport {
            scrub_id: uuid::Uuid::new_v4().to_string(),
            checked_chunks: 0,
            corrupted_chunks: 0,
            healed_chunks: 0,
            status: "completed".to_string(),
        };

        for vid in target_vault_ids {
            let report = state_clone.scrubber.run_scrub(&vid)?;
            aggregate.checked_chunks += report.checked_chunks;
            aggregate.corrupted_chunks += report.corrupted_chunks;
            aggregate.healed_chunks += report.healed_chunks;
        }

        Ok(aggregate)
    })
    .await;

    match join_res {
        Ok(Ok(aggregate)) => (StatusCode::ACCEPTED, Json(aggregate)).into_response(),
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => {
            warn!("scrub blocking task failed: {err}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "scrub_task_failed" })),
            )
                .into_response()
        }
    }
}

async fn handle_list_peers(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
) -> Response {
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    let peers = match state.db.list_trusted_peers() {
        Ok(p) => p,
        Err(err) => return map_domain_error(err),
    };
    // 以"显式握手租约 + 活跃 WS"为唯一在线判据：不再用任何单端缓存推断对端状态。
    let online_set: HashSet<String> = state.online_peer_ids();
    let peer_dtos: Vec<PeerDto> = peers
        .into_iter()
        .map(|p| {
            let online = online_set.contains(&p.device_id);
            PeerDto {
                device_id: p.device_id,
                device_name: p.device_name,
                os_type: p.os_type,
                last_ip: p.last_ip,
                last_port: p.last_port,
                auto_connect: p.auto_connect,
                online,
                paired_at: p.paired_at,
            }
        })
        .collect();

    let pairing_pin = match state.pairing_pin.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let pairing_qr_uri = format!(
        "lanvault://pair?id={}&ip={}&port={}&pin={}&fp={}",
        state.device_id,
        state.lan_ip,
        state.port,
        pairing_pin,
        state.identity.fingerprint_hex()
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "peers": peer_dtos,
            "pairing_pin": pairing_pin,
            "pairing_qr_uri": pairing_qr_uri,
        })),
    )
        .into_response()
}

impl AppState {
    fn complete_pairing_handshake(
        &self,
        client_ed25519_pubkey: [u8; 32],
        client_x25519_pubkey: [u8; 32],
        identity_message: &[u8],
        challenge: &str,
        device_name: String,
        os_type: String,
        remote_ip: String,
    ) -> Result<(PairPinResponse, String), LanVaultError> {
        let server_kex = SessionKeyExchange::new();
        let server_x25519_pub = server_kex.public_key_bytes();
        let session_key = server_kex.agree(&client_x25519_pubkey, identity_message)?;

        let peer_fp = public_key_fingerprint(&client_ed25519_pubkey);
        let peer_device_id = format!("lv-{}", &peer_fp[..12]);
        let existing_last_port = self
            .db
            .get_trusted_peer(&peer_device_id)?
            .and_then(|peer| peer.last_port);
        let peer = TrustedPeer {
            device_id: peer_device_id.clone(),
            device_name,
            os_type,
            ed25519_pubkey: client_ed25519_pubkey.to_vec(),
            last_ip: Some(remote_ip),
            last_port: existing_last_port,
            auto_connect: true,
            paired_at: unix_timestamp_secs(),
            revoked_at: None,
        };

        self.db.upsert_trusted_peer(&peer)?;
        self.auth.unrevoke_device(&peer_device_id);

        let session_token = self.auth.issue_peer_session(peer_device_id);
        let server_public_key = self.identity.public_key_bytes();
        let clipboard_key_envelope = encrypt_aead(
            &session_key,
            self.clipboard_key.as_bytes(),
            b"lanvault.clipboard.key.v1",
        )?;
        let response_message = pairing_response_message(
            challenge,
            &client_ed25519_pubkey,
            &client_x25519_pubkey,
            &server_public_key,
            &server_x25519_pub,
            &self.device_id,
            &clipboard_key_envelope,
        );
        let response_signature = self.identity.sign(&response_message);

        Ok((
            PairPinResponse {
                device_id: self.device_id.clone(),
                server_ed25519_pubkey_hex: hex::encode(server_public_key),
                server_x25519_pubkey_hex: hex::encode(server_x25519_pub),
                server_ed25519_signature_hex: hex::encode(response_signature),
                clipboard_key_envelope_hex: hex::encode(clipboard_key_envelope),
                session_cookie: Some(session_token.clone()),
            },
            session_token,
        ))
    }

    pub fn enqueue_pair_request(
        &self,
        entry: PendingPairEntry,
        now: i64,
    ) -> Result<(), PairQueueRejectReason> {
        let _guard = self
            .pending_pair_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.pending_pair_requests
            .retain(|_, existing| existing.dto.expires_at > now);

        // Deduplicate pending requests from the same device + IP so repeated clicks stay idempotent.
        let stale_ids: Vec<String> = self
            .pending_pair_requests
            .iter()
            .filter(|e| {
                e.value().dto.device_id == entry.dto.device_id
                    && e.value().dto.remote_ip == entry.dto.remote_ip
                    && matches!(e.value().status, PendingPairStatus::Pending)
            })
            .map(|e| e.key().clone())
            .collect();
        for id in stale_ids {
            self.pending_pair_requests.remove(&id);
        }

        let source_count = self
            .pending_pair_requests
            .iter()
            .filter(|e| {
                e.value().dto.remote_ip == entry.dto.remote_ip
                    && matches!(e.value().status, PendingPairStatus::Pending)
            })
            .count();
        if source_count >= MAX_PENDING_PAIR_REQUESTS_PER_IP {
            return Err(PairQueueRejectReason::SourceLimitReached);
        }

        if self.pending_pair_requests.len() >= MAX_PENDING_PAIR_REQUESTS {
            return Err(PairQueueRejectReason::GlobalLimitReached);
        }

        self.pending_pair_requests
            .insert(entry.dto.request_id.clone(), entry);
        Ok(())
    }

    fn check_renew_rate_limit(
        &self,
        key: &str,
        max_per_window: u32,
    ) -> Result<(), LanVaultError> {
        let now = Instant::now();
        let window = Duration::from_secs(RENEW_CHALLENGE_WINDOW_SECS);
        if self.renew_rate_limits.len() >= MAX_RENEW_RATE_ENTRIES {
            self.renew_rate_limits
                .retain(|_, (started_at, _)| now.saturating_duration_since(*started_at) < window);
            if self.renew_rate_limits.len() >= MAX_RENEW_RATE_ENTRIES {
                return Err(LanVaultError::RateLimited {
                    retry_after_secs: RENEW_CHALLENGE_WINDOW_SECS,
                });
            }
        }
        match self.renew_rate_limits.entry(key.to_string()) {
            dashmap::mapref::entry::Entry::Occupied(mut entry) => {
                let (started_at, count) = entry.get_mut();
                let elapsed = now.saturating_duration_since(*started_at);
                if elapsed >= window {
                    *started_at = now;
                    *count = 1;
                    Ok(())
                } else if *count >= max_per_window {
                    let retry_after_secs =
                        RENEW_CHALLENGE_WINDOW_SECS.saturating_sub(elapsed.as_secs()).max(1);
                    Err(LanVaultError::RateLimited { retry_after_secs })
                } else {
                    *count += 1;
                    Ok(())
                }
            }
            dashmap::mapref::entry::Entry::Vacant(vacant) => {
                vacant.insert((now, 1));
                Ok(())
            }
        }
    }

    pub fn issue_renew_challenge(&self, device_id: &str) -> Result<String, LanVaultError> {
        self.issue_renew_challenge_from_ip(None, device_id)
    }

    pub fn issue_renew_challenge_from_ip(
        &self,
        client_ip: Option<IpAddr>,
        device_id: &str,
    ) -> Result<String, LanVaultError> {
        let trimmed = device_id.trim();
        if trimmed.is_empty() {
            return Err(LanVaultError::InvalidInput(
                "device_id must not be empty".to_string(),
            ));
        }
        if let Some(ip) = client_ip {
            self.check_renew_rate_limit(&format!("chal_ip:{ip}"), MAX_RENEW_CHALLENGES_PER_IP)?;
        }
        if !self.auth.validate_session_device_active(trimmed) {
            return Err(LanVaultError::Auth("device is revoked".to_string()));
        }
        let Some(peer) = self.db.get_trusted_peer(trimmed)? else {
            return Err(LanVaultError::Auth("unknown device".to_string()));
        };
        if peer.revoked_at.is_some() {
            return Err(LanVaultError::Auth("device is revoked".to_string()));
        }
        self.check_renew_rate_limit(&format!("chal_dev:{trimmed}"), MAX_RENEW_CHALLENGES_PER_DEVICE)?;

        let now = Instant::now();
        self.renew_challenges
            .retain(|_, entry| entry.expires_at > now);
        let active_for_device = self
            .renew_challenges
            .iter()
            .filter(|e| e.value().device_id == trimmed)
            .count();
        if active_for_device >= MAX_RENEW_CHALLENGES_ACTIVE_PER_DEVICE
            || self.renew_challenges.len() >= MAX_RENEW_CHALLENGES
        {
            return Err(LanVaultError::RateLimited {
                retry_after_secs: RENEW_CHALLENGE_TTL_SECS,
            });
        }

        let mut raw = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut raw);
        let challenge = hex::encode(raw);
        self.renew_challenges.insert(
            challenge.clone(),
            RenewChallengeEntry {
                device_id: trimmed.to_string(),
                server_device_id: self.device_id.clone(),
                expires_at: now + Duration::from_secs(RENEW_CHALLENGE_TTL_SECS),
            },
        );
        Ok(challenge)
    }

    pub fn consume_renew_challenge(&self, challenge: &str, expected_device_id: &str) -> bool {
        let Some((_, entry)) = self.renew_challenges.remove(challenge) else {
            return false;
        };
        Instant::now() <= entry.expires_at
            && entry.device_id == expected_device_id
            && entry.server_device_id == self.device_id
    }

    pub fn list_pending_pair_requests(&self) -> Vec<PendingPairRequestDto> {
        let _guard = self
            .pending_pair_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = unix_timestamp_secs();
        self.pending_pair_requests
            .retain(|_, entry| entry.dto.expires_at > now);
        let mut items: Vec<PendingPairRequestDto> = self
            .pending_pair_requests
            .iter()
            .filter(|entry| matches!(entry.value().status, PendingPairStatus::Pending))
            .map(|entry| entry.value().dto.clone())
            .collect();
        items.sort_by_key(|item| item.created_at);
        items
    }

    pub fn respond_pair_request(
        &self,
        request_id: &str,
        accept: bool,
    ) -> Result<bool, LanVaultError> {
        let _guard = self
            .pending_pair_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = unix_timestamp_secs();
        self.pending_pair_requests
            .retain(|_, entry| entry.dto.expires_at > now);

        let Some(mut entry) = self.pending_pair_requests.get_mut(request_id) else {
            return Err(LanVaultError::NotFound(format!(
                "pairing request {request_id}"
            )));
        };
        if !matches!(entry.status, PendingPairStatus::Pending) {
            return Err(LanVaultError::InvalidState(
                "pairing request was already resolved".to_string(),
            ));
        }

        if !accept {
            entry.status = PendingPairStatus::Rejected;
            return Ok(false);
        }

        let (resp, _) = self.complete_pairing_handshake(
            entry.client_ed25519_pubkey,
            entry.client_x25519_pubkey,
            &entry.identity_message,
            &entry.nonce,
            entry.dto.device_name.clone(),
            entry.dto.os_type.clone(),
            entry.dto.remote_ip.clone(),
        )?;
        entry.status = PendingPairStatus::Approved(resp);
        Ok(true)
    }
}

async fn handle_pair_peer(
    State(state): State<Arc<AppState>>,
    ConnectInfo(LanConnectionInfo { addr, secure }): ConnectInfo<LanConnectionInfo>,
    cookies: Cookies,
    Json(req): Json<PairPinRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }

    if let Some(retry_after_secs) = state.auth.pair_lockout_remaining(addr.ip()) {
        let mut resp = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "locked_out",
                "retry_after_secs": retry_after_secs,
            })),
        )
            .into_response();
        if let Ok(val) = HeaderValue::from_str(&retry_after_secs.to_string()) {
            resp.headers_mut()
                .insert(HeaderName::from_static("retry-after"), val);
        }
        return resp;
    }

    let client_ed25519_pubkey =
        match decode_hex_32(&req.client_ed25519_pubkey_hex, "client_ed25519_pubkey_hex") {
            Ok(pk) => pk,
            Err(err) => return map_domain_error(err),
        };
    let client_x25519_pubkey =
        match decode_hex_32(&req.client_x25519_pubkey_hex, "client_x25519_pubkey_hex") {
            Ok(pk) => pk,
            Err(err) => return map_domain_error(err),
        };
    if req.device_name.trim().is_empty() || req.os_type.trim().is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "device_name and os_type must not be empty".to_string(),
        ));
    }
    let identity_message = pairing_identity_message(
        &req.pin,
        &client_ed25519_pubkey,
        &client_x25519_pubkey,
        req.device_name.trim(),
        req.os_type.trim(),
    );
    let signature_bytes = match hex::decode(&req.client_ed25519_signature_hex) {
        Ok(bytes) => bytes,
        Err(err) => {
            return map_domain_error(LanVaultError::InvalidInput(format!(
                "invalid client identity signature hex: {err}"
            )))
        }
    };
    if let Err(err) = verify_ed25519(&client_ed25519_pubkey, &identity_message, &signature_bytes) {
        return map_domain_error(err);
    }

    let pair_outcome = {
        let mut pairing_pin = match state.pairing_pin.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let matched = constant_time_eq(req.pin.as_bytes(), pairing_pin.as_bytes());
        let outcome = state.auth.check_and_record_pair_attempt(addr.ip(), matched);
        if matches!(outcome, AuthOutcome::Allowed) {
            *pairing_pin = crate::generate_six_digit_pin();
        }
        outcome
    };
    match pair_outcome {
        AuthOutcome::Allowed => {}
        AuthOutcome::Rejected { remaining_attempts } => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "invalid_pairing_pin",
                    "remaining_attempts": remaining_attempts,
                })),
            )
                .into_response();
        }
        AuthOutcome::LockedOut { retry_after_secs } => {
            let mut resp = (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "error": "locked_out",
                    "retry_after_secs": retry_after_secs,
                })),
            )
                .into_response();
            if let Ok(val) = HeaderValue::from_str(&retry_after_secs.to_string()) {
                resp.headers_mut()
                    .insert(HeaderName::from_static("retry-after"), val);
            }
            return resp;
        }
    }

    match state.complete_pairing_handshake(
        client_ed25519_pubkey,
        client_x25519_pubkey,
        &identity_message,
        &req.pin,
        req.device_name.trim().to_string(),
        req.os_type.trim().to_string(),
        addr.ip().to_string(),
    ) {
        Ok((pair_resp, session_token)) => {
            cookies.add(build_session_cookie(session_token, secure));
            (StatusCode::OK, Json(pair_resp)).into_response()
        }
        Err(err) => map_domain_error(err),
    }
}

async fn handle_pair_request_initiate(
    State(state): State<Arc<AppState>>,
    ConnectInfo(LanConnectionInfo { addr, .. }): ConnectInfo<LanConnectionInfo>,
    Json(req): Json<PairRequestInitiateBody>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Some(retry_after_secs) = state.auth.pair_lockout_remaining(addr.ip()) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "locked_out",
                "retry_after_secs": retry_after_secs,
            })),
        )
            .into_response();
    }
    if req.nonce.trim().is_empty()
        || req.device_name.trim().is_empty()
        || req.os_type.trim().is_empty()
    {
        return map_domain_error(LanVaultError::InvalidInput(
            "nonce, device_name and os_type must not be empty".to_string(),
        ));
    }
    let client_ed25519_pubkey =
        match decode_hex_32(&req.client_ed25519_pubkey_hex, "client_ed25519_pubkey_hex") {
            Ok(pk) => pk,
            Err(err) => return map_domain_error(err),
        };
    let client_x25519_pubkey =
        match decode_hex_32(&req.client_x25519_pubkey_hex, "client_x25519_pubkey_hex") {
            Ok(pk) => pk,
            Err(err) => return map_domain_error(err),
        };
    let device_name = req.device_name.trim().to_string();
    let os_type = req.os_type.trim().to_string();
    let nonce = req.nonce.trim().to_string();

    let identity_message = pairing_identity_message(
        &nonce,
        &client_ed25519_pubkey,
        &client_x25519_pubkey,
        &device_name,
        &os_type,
    );
    let signature_bytes = match hex::decode(&req.client_ed25519_signature_hex) {
        Ok(bytes) => bytes,
        Err(err) => {
            return map_domain_error(LanVaultError::InvalidInput(format!(
                "invalid client identity signature hex: {err}"
            )))
        }
    };
    if let Err(err) = verify_ed25519(&client_ed25519_pubkey, &identity_message, &signature_bytes) {
        return map_domain_error(err);
    }

    let now = unix_timestamp_secs();
    let fp = public_key_fingerprint(&client_ed25519_pubkey);
    let device_id = format!("lv-{}", &fp[..12]);
    let remote_ip = addr.ip().to_string();

    let request_id = uuid::Uuid::new_v4().to_string();
    let mut poll_bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut poll_bytes);
    let poll_token = hex::encode(poll_bytes);
    let expires_at = now + PENDING_PAIR_TTL_SECS;

    let dto = PendingPairRequestDto {
        request_id: request_id.clone(),
        device_id,
        device_name,
        os_type,
        fingerprint: fp,
        remote_ip,
        created_at: now,
        expires_at,
    };

    let entry = PendingPairEntry {
        dto: dto.clone(),
        poll_token: poll_token.clone(),
        nonce,
        client_ed25519_pubkey,
        client_x25519_pubkey,
        identity_message,
        status: PendingPairStatus::Pending,
    };
    if let Err(reason) = state.enqueue_pair_request(entry, now) {
        let code = match reason {
            PairQueueRejectReason::SourceLimitReached => {
                "too_many_pending_pair_requests_from_source"
            }
            PairQueueRejectReason::GlobalLimitReached => "too_many_pending_pair_requests",
        };
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "error": code })),
        )
            .into_response();
    }

    let _ = state.ws_tx.send(WsServerEvent::PairingRequested(dto));

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "request_id": request_id,
            "poll_token": poll_token,
            "expires_in_secs": PENDING_PAIR_TTL_SECS,
        })),
    )
        .into_response()
}

async fn handle_pair_request_status(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PairRequestStatusBody>,
) -> Response {
    let _queue_guard = state
        .pending_pair_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = unix_timestamp_secs();
    state
        .pending_pair_requests
        .retain(|_, entry| entry.dto.expires_at > now);

    let Some(entry) = state.pending_pair_requests.get(&req.request_id) else {
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "expired" })),
        )
            .into_response();
    };
    if !constant_time_eq(req.poll_token.as_bytes(), entry.poll_token.as_bytes()) {
        return unauthorized_response();
    }

    match &entry.status {
        PendingPairStatus::Pending => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "pending" })),
        )
            .into_response(),
        PendingPairStatus::Rejected => {
            drop(entry);
            state.pending_pair_requests.remove(&req.request_id);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "status": "rejected" })),
            )
                .into_response()
        }
        PendingPairStatus::Approved(pair_resp) => {
            let cloned = pair_resp.clone();
            drop(entry);
            state.pending_pair_requests.remove(&req.request_id);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "approved",
                    "response": cloned,
                })),
            )
                .into_response()
        }
    }
}

async fn handle_list_pair_requests(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
) -> Response {
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    let requests = state.list_pending_pair_requests();
    (
        StatusCode::OK,
        Json(serde_json::json!({ "requests": requests })),
    )
        .into_response()
}

async fn handle_respond_pair_request(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(request_id): Path<String>,
    Json(req): Json<RespondPairRequestBody>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    match state.respond_pair_request(&request_id, req.accept) {
        Ok(accepted) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "request_id": request_id,
                "accepted": accepted,
            })),
        )
            .into_response(),
        Err(err) => map_domain_error(err),
    }
}

/// 已配对设备的显式握手（14）。每次调用都会续期在线租约，并把对端观测到的 IP/端口
/// 写回 `trusted_peers`，让双方状态机收敛到同一份"谁在线"的视图。
///
/// 请求：`POST /api/peers/heartbeat { "last_port": number|null }`（需 `lv_session` Cookie，
/// 且必须是已配对设备会话）。响应：等式化的在线设备列表 + 服务端自身身份。
async fn handle_peer_heartbeat(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    ConnectInfo(LanConnectionInfo { addr, .. }): ConnectInfo<LanConnectionInfo>,
    Json(req): Json<HeartbeatRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    // 管理员会话（本机 Web 控制台）没有 device_id，视为"无对端"，直接返回当前视图。
    let Some(device_id) = session.device_id.clone() else {
        return heartbeat_response(&state, None);
    };

    let peer = match state.db.get_trusted_peer(&device_id) {
        Ok(Some(peer)) => peer,
        Ok(None) => return unauthorized_response(),
        Err(err) => return map_domain_error(err),
    };
    if state.auth.validate_session_device_active(&device_id) == false {
        return unauthorized_response();
    }

    let reported_port = req.last_port.filter(|port| *port > 0).or(peer.last_port);
    let observed_ip = addr.ip().to_string();
    let ip_changed = peer.last_ip.as_deref() != Some(observed_ip.as_str());
    let port_changed = peer.last_port != reported_port;

    if ip_changed || port_changed {
        let updated = TrustedPeer {
            device_id: peer.device_id.clone(),
            device_name: peer.device_name.clone(),
            os_type: peer.os_type.clone(),
            ed25519_pubkey: peer.ed25519_pubkey.clone(),
            last_ip: Some(observed_ip),
            last_port: reported_port,
            auto_connect: peer.auto_connect,
            paired_at: peer.paired_at,
            revoked_at: peer.revoked_at,
        };
        if let Err(err) = state.db.upsert_trusted_peer(&updated) {
            return map_domain_error(err);
        }
    }

    state.touch_peer_lease(&device_id);
    heartbeat_response(&state, Some(device_id))
}

/// 构造握手响应：`peers` 是**经过租约校验**的在线已配对设备，`self_device_id` 让客户端
/// 校验自己是否已经与对端处于同一信任集合（避免单边残留配对）。
fn heartbeat_response(state: &AppState, ack_device_id: Option<String>) -> Response {
    state.prune_peer_leases();
    let online_ids = state.online_peer_ids();
    let peers: Vec<serde_json::Value> = match state.db.list_trusted_peers() {
        Ok(peers) => peers
            .into_iter()
            .map(|peer| {
                serde_json::json!({
                    "device_id": peer.device_id,
                    "device_name": peer.device_name,
                    "os_type": peer.os_type,
                    "ip": peer.last_ip,
                    "port": peer.last_port,
                    "paired_at": peer.paired_at,
                    "online": online_ids.contains(&peer.device_id),
                })
            })
            .collect(),
        Err(err) => return map_domain_error(err),
    };
    let lease_secs = PEER_PRESENCE_LEASE_SECS;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "self_device_id": state.device_id,
            "ack_device_id": ack_device_id,
            "lease_secs": lease_secs,
            "peers": peers,
        })),
    )
        .into_response()
}

/// 已配对设备的会话续期（公开路由，但必须持有该设备 Ed25519 私钥才能通过）。
///
/// 服务端的 `lv_session` 会话表只在内存里，服务端重启后旧 Cookie 全部失效。
/// 若让心跳把 401 当成"被解绑"，就会误删一份合法的配对。这里用设备自己的签名
/// 换取新会话，让两端状态机在服务端重启后依然收敛（对齐 Tailscale 的
/// "long-lived key re-authorization" 做法）。
fn rate_limited_response(retry_after_secs: u64) -> Response {
    let mut resp = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(serde_json::json!({
            "error": "rate_limited",
            "retry_after_secs": retry_after_secs,
        })),
    )
        .into_response();
    if let Ok(val) = HeaderValue::from_str(&retry_after_secs.to_string()) {
        resp.headers_mut()
            .insert(HeaderName::from_static("retry-after"), val);
    }
    resp
}

async fn handle_peer_session_challenge(
    State(state): State<Arc<AppState>>,
    ConnectInfo(LanConnectionInfo { addr, .. }): ConnectInfo<LanConnectionInfo>,
    Json(req): Json<PeerSessionChallengeRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    match state.issue_renew_challenge_from_ip(Some(addr.ip()), &req.device_id) {
        Ok(challenge) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "challenge": challenge,
                "server_device_id": state.device_id,
                "expires_in_secs": RENEW_CHALLENGE_TTL_SECS,
            })),
        )
            .into_response(),
        Err(LanVaultError::RateLimited { retry_after_secs }) => {
            rate_limited_response(retry_after_secs)
        }
        Err(LanVaultError::Auth(_)) => unauthorized_response(),
        Err(err) => map_domain_error(err),
    }
}

async fn handle_peer_session_renew(
    State(state): State<Arc<AppState>>,
    ConnectInfo(LanConnectionInfo { addr, secure }): ConnectInfo<LanConnectionInfo>,
    cookies: Cookies,
    Json(req): Json<PeerSessionRenewRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(LanVaultError::RateLimited { retry_after_secs }) =
        state.check_renew_rate_limit(&format!("renew_ip:{}", addr.ip()), MAX_RENEW_REQUESTS_PER_IP)
    {
        return rate_limited_response(retry_after_secs);
    }
    let challenge = req.nonce.trim();
    if challenge.is_empty() || req.device_id.trim().is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "device_id and challenge must not be empty".to_string(),
        ));
    }
    let declared_pubkey =
        match decode_hex_32(&req.client_ed25519_pubkey_hex, "client_ed25519_pubkey_hex") {
            Ok(pk) => pk,
            Err(err) => return map_domain_error(err),
        };
    let signature = match hex::decode(&req.client_ed25519_signature_hex) {
        Ok(bytes) => bytes,
        Err(err) => {
            return map_domain_error(LanVaultError::InvalidInput(format!(
                "invalid client signature hex: {err}"
            )))
        }
    };

    // 设备 ID 必须由公钥指纹推出，且与请求声明一致，杜绝冒用他人 ID。
    let expected_device_id = format!("lv-{}", &public_key_fingerprint(&declared_pubkey)[..12]);
    if expected_device_id != req.device_id.trim() {
        return unauthorized_response();
    }
    if !state
        .auth
        .validate_session_device_active(&expected_device_id)
    {
        return unauthorized_response();
    }

    let peer = match state.db.get_trusted_peer(&expected_device_id) {
        Ok(Some(peer)) => peer,
        Ok(None) => return unauthorized_response(),
        Err(err) => return map_domain_error(err),
    };
    if peer.revoked_at.is_some()
        || peer.ed25519_pubkey.len() != 32
        || peer.ed25519_pubkey.as_slice() != declared_pubkey.as_slice()
    {
        return unauthorized_response();
    }

    let expected_msg = session_renew_message(&state.device_id, &expected_device_id, challenge);
    if let Err(err) = verify_ed25519(&declared_pubkey, &expected_msg, &signature) {
        return map_domain_error(err);
    }
    if !state.consume_renew_challenge(challenge, &expected_device_id) {
        return unauthorized_response();
    }

    let token = state.auth.issue_peer_session(expected_device_id.clone());
    if addr.ip().to_string() != peer.last_ip.clone().unwrap_or_default() {
        let updated = TrustedPeer {
            last_ip: Some(addr.ip().to_string()),
            ..peer
        };
        let _ = state.db.upsert_trusted_peer(&updated);
    }
    state.touch_peer_lease(&expected_device_id);
    info!(device_id = %expected_device_id, "peer session renewed via Ed25519 signature");

    cookies.add(build_session_cookie(token.clone(), secure));
    Json(serde_json::json!({
        "ok": true,
        "device_id": expected_device_id,
        "session_cookie": token,
    }))
    .into_response()
}

async fn handle_delete_peer(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(device_id): Path<String>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    match state.db.delete_trusted_peer(&device_id) {
        Ok(true) => {
            if let Err(err) = state.db.revoke_peer_from_all_groups(&device_id) {
                return map_domain_error(err);
            }
            state.online_peers.remove(&device_id);
            state.peer_leases.remove(&device_id);
            state.auth.revoke_device_sessions(&device_id);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "revoked": true,
                    "device_id": device_id,
                })),
            )
                .into_response()
        }
        Ok(false) => map_domain_error(LanVaultError::NotFound(format!("peer {device_id}"))),
        Err(err) => map_domain_error(err),
    }
}

#[derive(Debug, Deserialize)]
struct TransferInitRequest {
    transfer_id: String,
    filename: String,
    total_size: u64,
    chunk_size: u64,
    file_blake3_hex: String,
}

fn resolve_transfer_dirs(
    state: &AppState,
) -> lanvault_core::Result<(std::path::PathBuf, std::path::PathBuf)> {
    let vaults = state.db.list_vaults()?;
    let base_root = if let Some(v) = vaults.first() {
        let vols = state.vault_store.online_volumes(&v.vault_id)?;
        if let Some(vol) = vols.first() {
            std::path::PathBuf::from(&vol.mount_path)
        } else {
            std::env::temp_dir().join("everyonenas_transfers")
        }
    } else {
        std::env::temp_dir().join("everyonenas_transfers")
    };
    let staging = base_root.join("transfers_staging");
    let received = base_root.join("received");
    std::fs::create_dir_all(&staging)?;
    std::fs::create_dir_all(&received)?;
    Ok((staging, received))
}

fn sanitize_transfer_filename(raw: &str) -> String {
    let name = std::path::Path::new(raw)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("received_file");
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() {
        "received_file".to_string()
    } else {
        cleaned
    }
}

fn transfer_owner_id(session: &SessionInfo) -> Result<String, Response> {
    if let Some(device_id) = session
        .device_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Ok(device_id.to_string())
    } else if session.is_admin {
        Ok("__admin__".to_string())
    } else {
        Err(unauthorized_response())
    }
}

fn scoped_transfer_id(owner_id: &str, client_transfer_id: &str) -> String {
    format!("{owner_id}__{client_transfer_id}")
}

fn expected_transfer_part_len(
    total_size: u64,
    chunk_size: u64,
    total_parts: u64,
    part_index: u64,
) -> Option<u64> {
    if part_index >= total_parts || chunk_size == 0 {
        return None;
    }
    if total_size == 0 {
        return (part_index == 0).then_some(0);
    }
    let offset = part_index.checked_mul(chunk_size)?;
    if offset >= total_size {
        return None;
    }
    Some(chunk_size.min(total_size - offset))
}

fn is_disk_full_io_error(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::StorageFull
        || matches!(err.raw_os_error(), Some(28) | Some(112))
}

impl AppState {
    pub(crate) fn reclaim_expired_transfers(&self, now_secs: i64) -> lanvault_core::Result<()> {
        // Run cleanup only when all transfer operations have released their locks.
        let Ok(_locks) = self
            .transfer_locks
            .iter()
            .map(|lock| lock.try_write())
            .collect::<Result<Vec<_>, _>>()
        else {
            return Ok(());
        };
        let (staging, _) = resolve_transfer_dirs(self)?;
        self.db.prune_expired_transfer_sessions(now_secs)?;
        for entry in fs::read_dir(staging)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("part")
                || !entry.file_type()?.is_file()
            {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !self.db.is_active_transfer_session(id)? {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }
}

fn transfer_lock(state: &AppState, scoped_id: &str) -> Arc<tokio::sync::RwLock<()>> {
    let digest = blake3_hash(scoped_id.as_bytes());
    Arc::clone(&state.transfer_locks[usize::from(digest[0]) % TRANSFER_LOCK_COUNT])
}

fn received_usage(path: &StdPath) -> std::io::Result<(usize, u64)> {
    fs::read_dir(path)?.try_fold((0usize, 0u64), |(count, bytes), entry| {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            Ok((
                count.saturating_add(1),
                bytes.saturating_add(entry.metadata()?.len()),
            ))
        } else {
            Ok((count, bytes))
        }
    })
}

fn allocate_unique_received_path(
    received_dir: &StdPath,
    filename: &str,
    reserved: &[String],
) -> std::path::PathBuf {
    let available = |candidate: &StdPath| {
        let key = candidate.to_string_lossy();
        !candidate.exists()
            && !reserved.iter().any(|path| {
                if cfg!(windows) {
                    path.to_lowercase() == key.to_lowercase()
                } else {
                    path == key.as_ref()
                }
            })
    };
    let direct = received_dir.join(filename);
    if available(&direct) {
        return direct;
    }
    let stem = StdPath::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let ext = StdPath::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    for idx in 1..1000u32 {
        let candidate_name = if ext.is_empty() {
            format!("{stem} ({idx})")
        } else {
            format!("{stem} ({idx}).{ext}")
        };
        let cand = received_dir.join(candidate_name);
        if available(&cand) {
            return cand;
        }
    }
    let suffix = uuid::Uuid::new_v4();
    let fallback = if ext.is_empty() {
        format!("{stem}-{suffix}")
    } else {
        format!("{stem}-{suffix}.{ext}")
    };
    received_dir.join(fallback)
}

async fn handle_transfer_init(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Json(req): Json<TransferInitRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    let owner_id = match transfer_owner_id(&session) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let safe_tid = match validate_safe_id(&req.transfer_id, "transfer_id") {
        Ok(id) => id,
        Err(err) => return map_domain_error(err),
    };
    if req.chunk_size == 0
        || req.chunk_size > 8 * 1024 * 1024
        || req.total_size > MAX_TRANSFER_FILE_BYTES
        || req.file_blake3_hex.len() != 64
        || !req.file_blake3_hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return map_domain_error(LanVaultError::InvalidInput(
            "invalid transfer init parameters".to_string(),
        ));
    }
    let scoped_id = scoped_transfer_id(&owner_id, &safe_tid);
    let filename = sanitize_transfer_filename(&req.filename);
    let total_parts = if req.total_size == 0 {
        1
    } else {
        req.total_size.div_ceil(req.chunk_size)
    };
    if total_parts > MAX_TRANSFER_PARTS {
        return map_domain_error(LanVaultError::InvalidInput(
            "transfer has too many parts".to_string(),
        ));
    }
    let init_guard = transfer_lock(&state, &scoped_id).write_owned().await;
    let write_guard = ActiveChunkWriteGuard::new(Arc::clone(&state));
    if let Err(response) = require_accepting_tasks(&state) {
        return response;
    }
    let result = tokio::task::spawn_blocking(move || {
        let _init_guard = init_guard;
        let _write_guard = write_guard;
        let now = unix_timestamp_secs();

        let _budget_guard = state
            .transfer_budget_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Resuming an already-active identical transfer for the same owner does not double-count budget.
        if let Ok(Some(existing)) = state.db.get_transfer_session(&scoped_id) {
            if existing.owner_device_id == owner_id
                && existing.status == "active"
                && (existing.expires_at > now || existing.saved_path.is_some())
                && existing.total_size == req.total_size
                && existing.chunk_size == req.chunk_size
                && existing
                    .file_blake3
                    .eq_ignore_ascii_case(&req.file_blake3_hex)
            {
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "transfer_id": safe_tid,
                        "total_parts": existing.total_parts,
                        "received_parts": existing.received_parts,
                    })),
                )
                    .into_response();
            }
            if existing.status == "active" {
                return map_domain_error(LanVaultError::InvalidState(
                    "transfer ID already exists with different parameters or expired".to_string(),
                ));
            }
        }

        let (owner_count, owner_bytes, global_count, global_bytes) =
            match state.db.active_transfer_usage(&owner_id) {
                Ok(usage) => usage,
                Err(err) => return map_domain_error(err),
            };
        if owner_count >= MAX_ACTIVE_TRANSFERS_PER_DEVICE
            || global_count >= MAX_ACTIVE_TRANSFERS_GLOBAL
        {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({ "error": "too_many_active_transfers" })),
            )
                .into_response();
        }
        let (retained_count, retained_bytes) = match resolve_transfer_dirs(&state)
            .and_then(|(_, received)| Ok(received_usage(&received)?))
        {
            Ok(usage) => usage,
            Err(error) => return map_domain_error(error),
        };
        if global_count.saturating_add(retained_count) >= MAX_RETAINED_TRANSFERS {
            return (
                StatusCode::INSUFFICIENT_STORAGE,
                Json(serde_json::json!({ "error": "received_file_count_limit" })),
            )
                .into_response();
        }
        let reserved_and_retained = global_bytes.saturating_add(retained_bytes);
        if owner_bytes.saturating_add(req.total_size) > MAX_STAGING_BYTES_PER_DEVICE
            || reserved_and_retained.saturating_add(req.total_size) > MAX_STAGING_BYTES_GLOBAL
        {
            return map_domain_error(LanVaultError::QuotaExceeded {
                required: req.total_size,
                available: MAX_STAGING_BYTES_PER_DEVICE
                    .saturating_sub(owner_bytes)
                    .min(MAX_STAGING_BYTES_GLOBAL.saturating_sub(reserved_and_retained)),
            });
        }
        match state.db.list_vaults() {
            Ok(vaults) => {
                if let Some(vault) = vaults.first() {
                    let used_with_staging = vault.used_bytes.saturating_add(reserved_and_retained);
                    let available = vault.quota_bytes.saturating_sub(used_with_staging);
                    if vault.quota_bytes > 0 && req.total_size > available {
                        return map_domain_error(LanVaultError::QuotaExceeded {
                            required: req.total_size,
                            available,
                        });
                    }
                }
            }
            Err(error) => return map_domain_error(error),
        }

        let rec = lanvault_core::TransferSessionRecord {
            transfer_id: scoped_id,
            owner_device_id: owner_id,
            filename,
            total_size: req.total_size,
            chunk_size: req.chunk_size,
            total_parts,
            received_parts: Vec::new(),
            file_blake3: req.file_blake3_hex.to_ascii_lowercase(),
            encoding: "raw".to_string(),
            status: "active".to_string(),
            saved_path: None,
            created_at: now,
            updated_at: now,
            expires_at: now + 86_400,
        };
        match state.db.upsert_transfer_session(&rec) {
            Ok(session_rec) => (
                StatusCode::OK,
                Json(serde_json::json!({
                    "transfer_id": safe_tid,
                    "total_parts": session_rec.total_parts,
                    "received_parts": session_rec.received_parts,
                })),
            )
                .into_response(),
            Err(err) => map_domain_error(err),
        }
    })
    .await;
    match result {
        Ok(response) => response,
        Err(error) => internal_error(format!("transfer init task failed: {error}")),
    }
}

async fn handle_transfer_part(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path((transfer_id, part_index)): Path<(String, u64)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    let owner_id = match transfer_owner_id(&session) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let safe_tid = match validate_safe_id(&transfer_id, "transfer_id") {
        Ok(id) => id,
        Err(err) => return map_domain_error(err),
    };
    let scoped_id = scoped_transfer_id(&owner_id, &safe_tid);
    let part_read_guard = transfer_lock(&state, &scoped_id).read_owned().await;
    let write_guard = ActiveChunkWriteGuard::new(Arc::clone(&state));
    if let Err(response) = require_accepting_tasks(&state) {
        return response;
    }

    let session_rec = match state.db.get_transfer_session_metadata(&scoped_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!("transfer {safe_tid}")))
        }
        Err(err) => return map_domain_error(err),
    };
    if session_rec.owner_device_id != owner_id {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "forbidden" })),
        )
            .into_response();
    }
    if session_rec.status != "active"
        || session_rec.saved_path.is_some()
        || session_rec.expires_at <= unix_timestamp_secs()
    {
        return map_domain_error(LanVaultError::InvalidState(
            "transfer session is not active".to_string(),
        ));
    }
    let Some(expected_len) = expected_transfer_part_len(
        session_rec.total_size,
        session_rec.chunk_size,
        session_rec.total_parts,
        part_index,
    ) else {
        return map_domain_error(LanVaultError::InvalidInput(
            "part_index out of range".to_string(),
        ));
    };
    if (body.len() as u64) != expected_len {
        return map_domain_error(LanVaultError::InvalidInput(format!(
            "part {part_index} size mismatch: expected {expected_len} bytes, got {} bytes",
            body.len()
        )));
    }
    if let Some(expected_hex) = headers
        .get("x-lanvault-part-blake3")
        .and_then(|v| v.to_str().ok())
    {
        let actual_hex = hex::encode(lanvault_core::blake3_hash(&body));
        if !actual_hex.eq_ignore_ascii_case(expected_hex) {
            return map_domain_error(LanVaultError::InvalidInput(
                "part blake3 checksum mismatch".to_string(),
            ));
        }
    }

    let (staging_dir, _) = match resolve_transfer_dirs(&state) {
        Ok(dirs) => dirs,
        Err(err) => return map_domain_error(err),
    };
    let part_path = staging_dir.join(format!("{}.part", session_rec.transfer_id));
    let offset = part_index.saturating_mul(session_rec.chunk_size);
    let body_vec = body.to_vec();
    let part_len = body_vec.len() as u64;

    let write_res = tokio::task::spawn_blocking(move || -> lanvault_core::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let _part_read_guard = part_read_guard;
        let _write_guard = write_guard;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&part_path)?;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&body_vec)?;
        state.db.mark_transfer_part_received(
            &session_rec.transfer_id,
            part_index,
            part_len,
            unix_timestamp_secs(),
        )?;
        state.transfer_bytes.fetch_add(part_len, Ordering::Relaxed);
        Ok(())
    })
    .await;

    match write_res {
        Ok(Ok(())) => (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true, "part_index": part_index })),
        )
            .into_response(),
        Ok(Err(LanVaultError::Io(ref err))) if is_disk_full_io_error(err) => {
            map_domain_error(LanVaultError::QuotaExceeded {
                required: part_len,
                available: 0,
            })
        }
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => internal_error(format!("transfer part write failed: {err}")),
    }
}

async fn handle_transfer_status(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(transfer_id): Path<String>,
) -> Response {
    let owner_id = match transfer_owner_id(&session) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let safe_tid = match validate_safe_id(&transfer_id, "transfer_id") {
        Ok(id) => id,
        Err(err) => return map_domain_error(err),
    };
    let scoped_id = scoped_transfer_id(&owner_id, &safe_tid);
    match state.db.get_transfer_session(&scoped_id) {
        Ok(Some(r)) => {
            if r.owner_device_id != owner_id {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({ "error": "forbidden" })),
                )
                    .into_response();
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "transfer_id": safe_tid,
                    "status": r.status,
                    "total_parts": r.total_parts,
                    "received_parts": r.received_parts,
                })),
            )
                .into_response()
        }
        Ok(None) => map_domain_error(LanVaultError::NotFound(format!("transfer {safe_tid}"))),
        Err(err) => map_domain_error(err),
    }
}

async fn handle_transfer_commit(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(transfer_id): Path<String>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    let owner_id = match transfer_owner_id(&session) {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let safe_tid = match validate_safe_id(&transfer_id, "transfer_id") {
        Ok(id) => id,
        Err(err) => return map_domain_error(err),
    };
    let scoped_id = scoped_transfer_id(&owner_id, &safe_tid);
    let commit_write_guard = transfer_lock(&state, &scoped_id).write_owned().await;
    let write_guard = ActiveChunkWriteGuard::new(Arc::clone(&state));
    if let Err(response) = require_accepting_tasks(&state) {
        return response;
    }

    let session_rec = match state.db.get_transfer_session(&scoped_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!("transfer {safe_tid}")))
        }
        Err(err) => return map_domain_error(err),
    };
    if session_rec.owner_device_id != owner_id {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "forbidden" })),
        )
            .into_response();
    }
    // Idempotent retry: if already committed, return the persisted result directly.
    if session_rec.status == "completed" {
        if let Some(saved_path) = session_rec.saved_path.clone() {
            return (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "saved_path": saved_path,
                    "size_bytes": session_rec.total_size,
                })),
            )
                .into_response();
        }
    }
    if session_rec.status != "active"
        || (session_rec.saved_path.is_none() && session_rec.expires_at <= unix_timestamp_secs())
    {
        return map_domain_error(LanVaultError::InvalidState(
            "transfer session is not active".to_string(),
        ));
    }
    if (session_rec.received_parts.len() as u64) != session_rec.total_parts {
        return map_domain_error(LanVaultError::InvalidState(format!(
            "transfer incomplete: {}/{} parts received",
            session_rec.received_parts.len(),
            session_rec.total_parts
        )));
    }
    let (staging_dir, received_dir) = match resolve_transfer_dirs(&state) {
        Ok(dirs) => dirs,
        Err(err) => return map_domain_error(err),
    };
    let part_path = staging_dir.join(format!("{}.part", session_rec.transfer_id));
    let expected_hash = session_rec.file_blake3.clone();
    let total_size = session_rec.total_size;
    let filename = session_rec.filename.clone();
    let event_filename = filename.clone();
    let state_for_commit = Arc::clone(&state);
    let commit_res = tokio::task::spawn_blocking(move || -> lanvault_core::Result<(PathBuf, i64)> {
        use std::io::Read;
        let _commit_write_guard = commit_write_guard;
        let _write_guard = write_guard;
        let mut dest = session_rec.saved_path.as_deref().map(PathBuf::from);
        let recovers_published = dest.is_some() && !part_path.try_exists()?;
        let verify_path = if recovers_published {
            dest.as_deref().expect("published transfer has a destination")
        } else {
            part_path.as_path()
        };
        let (actual_len, actual_hash) = {
            let file = if total_size == 0 && !recovers_published && !verify_path.try_exists()? {
                std::fs::OpenOptions::new()
                    .create_new(true)
                    .read(true)
                    .write(true)
                    .open(verify_path)?
            } else {
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(verify_path)?
            };
            file.sync_all()?;
            let actual_len = file.metadata()?.len();
            let mut reader = std::io::BufReader::new(file);
            let mut hasher = blake3::Hasher::new();
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            (actual_len, hasher.finalize().to_hex().to_string())
        };
        if actual_len != total_size {
            return Err(LanVaultError::InvalidState(format!(
                "transfer file size mismatch: expected {total_size} bytes, actual {actual_len} bytes"
            )));
        }
        if !actual_hash.eq_ignore_ascii_case(&expected_hash) {
            return Err(LanVaultError::CorruptedChunk { hash: expected_hash });
        }
        let _budget_guard = state_for_commit
            .transfer_budget_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let db = &state_for_commit.db;
        if dest.is_none() {
            let reserved = db.reserved_transfer_destinations()?;
            let target = allocate_unique_received_path(&received_dir, &filename, &reserved);
            db.reserve_transfer_destination(&session_rec.transfer_id, &target.to_string_lossy())?;
            dest = Some(target);
        }
        let dest = dest.expect("transfer destination is reserved");
        if !recovers_published {
            let publication = if dest.try_exists()? {
                Err(std::io::ErrorKind::AlreadyExists.into())
            } else {
                fs::rename(&part_path, &dest)
            };
            if let Err(error) = publication {
                if part_path.try_exists()? {
                    db.release_transfer_destination(&session_rec.transfer_id, &dest.to_string_lossy())?;
                }
                return Err(error.into());
            }
        }
        let now = unix_timestamp_secs();
        let peer_label = db
            .get_trusted_peer(&owner_id)?
            .map(|peer| peer.device_name)
            .unwrap_or_else(|| "对端设备".to_string());
        db.complete_transfer_session_with_history(
            &session_rec.transfer_id,
            &dest.to_string_lossy(),
            &filename,
            &peer_label,
            total_size,
            now,
        )?;
        Ok((dest, now))
    })
    .await;

    match commit_res {
        Ok(Ok((dest_path, now))) => {
            let saved_str = dest_path.to_string_lossy().to_string();
            let _ = state.ws_tx.send(WsServerEvent::TransferReceived {
                transfer_id: safe_tid,
                filename: event_filename,
                size_bytes: total_size,
                saved_path: saved_str.clone(),
                timestamp: now,
            });
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "saved_path": saved_str,
                    "size_bytes": total_size,
                })),
            )
                .into_response()
        }
        Ok(Err(LanVaultError::Io(ref io_err))) if is_disk_full_io_error(io_err) => {
            map_domain_error(LanVaultError::QuotaExceeded {
                required: total_size,
                available: 0,
            })
        }
        Ok(Err(err)) => map_domain_error(err),
        Err(err) => internal_error(format!("transfer commit task failed: {err}")),
    }
}

async fn handle_clipboard_push(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Json(req): Json<ClipboardRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if req.content.is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "clipboard content must not be empty".to_string(),
        ));
    }

    let timestamp = unix_timestamp_secs();
    let raw_size_bytes = req.content.len() as u64;
    let sender_device_id = session
        .device_id
        .clone()
        .unwrap_or_else(|| state.device_id.clone());
    let client_msg_id = req
        .client_msg_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let content = if session.is_admin {
        let wire = match encrypt_aead(&state.clipboard_key, req.content.as_bytes(), CLIPBOARD_AAD) {
            Ok(wire) => wire,
            Err(err) => return map_domain_error(err),
        };
        match serde_json::to_string(&ClipboardEnvelope {
            wire_hex: hex::encode(wire),
        }) {
            Ok(content) => content,
            Err(err) => return map_domain_error(err.into()),
        }
    } else {
        if serde_json::from_str::<ClipboardEnvelope>(&req.content).is_err() {
            return map_domain_error(LanVaultError::InvalidInput(
                "peer clipboard content must contain an encrypted envelope".to_string(),
            ));
        }
        req.content
    };

    let explicit_targets: Option<Vec<String>> = match (req.target_device_ids, req.target_device_id)
    {
        (Some(ids), _) => {
            let mut seen = HashSet::new();
            let mut deduped = Vec::with_capacity(ids.len());
            for id in ids {
                let trimmed = id.trim();
                if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                    deduped.push(trimmed.to_string());
                }
            }
            Some(deduped)
        }
        (None, Some(single)) => {
            let trimmed = single.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(vec![trimmed.to_string()])
            }
        }
        (None, None) => None,
    };

    let target_ids: Vec<String> = match &explicit_targets {
        Some(list) => list.clone(),
        None => {
            let mut online: Vec<String> = state
                .online_peer_ids()
                .into_iter()
                .filter(|id| id != &sender_device_id)
                .collect();
            online.sort();
            online
        }
    };

    let mut targets: Vec<MessageTargetAck> = Vec::with_capacity(target_ids.len());
    let mut delivered_targets: Vec<String> = Vec::new();
    for target_id in &target_ids {
        if target_id == &sender_device_id {
            targets.push(MessageTargetAck {
                target_device_id: target_id.clone(),
                status: "failed".to_string(),
                error_code: Some("self_target".to_string()),
                error_message: Some("cannot send clipboard payload to self".to_string()),
            });
            continue;
        }
        let trusted = match state.db.get_trusted_peer(target_id) {
            Ok(peer) => peer,
            Err(err) => return map_domain_error(err),
        };
        if trusted.is_none() {
            targets.push(MessageTargetAck {
                target_device_id: target_id.clone(),
                status: "revoked".to_string(),
                error_code: Some("peer_revoked".to_string()),
                error_message: Some("target peer is revoked or not trusted".to_string()),
            });
        } else if state.is_peer_online(target_id) {
            delivered_targets.push(target_id.clone());
            targets.push(MessageTargetAck {
                target_device_id: target_id.clone(),
                status: "delivered".to_string(),
                error_code: None,
                error_message: None,
            });
        } else {
            targets.push(MessageTargetAck {
                target_device_id: target_id.clone(),
                status: "offline".to_string(),
                error_code: Some("peer_offline".to_string()),
                error_message: Some("target peer is currently offline".to_string()),
            });
        }
    }

    let delivered_to = delivered_targets.len();
    let accepted_to = delivered_to;
    let response_value = serde_json::json!({
        "client_msg_id": client_msg_id,
        "delivered_to": delivered_to,
        "accepted_to": accepted_to,
        "server_ack_at": timestamp,
        "timestamp": timestamp,
        "targets": targets,
    });
    let ack_json = match serde_json::to_string(&response_value) {
        Ok(s) => s,
        Err(err) => return map_domain_error(err.into()),
    };

    // Enforce idempotency per (sender_device_id, client_msg_id, server_device_id).
    match state.db.record_or_check_message_dedup(
        &sender_device_id,
        &client_msg_id,
        &state.device_id,
        &ack_json,
    ) {
        Ok(Some(existing_ack_json)) => {
            let existing: serde_json::Value = match serde_json::from_str(&existing_ack_json) {
                Ok(v) => v,
                Err(err) => return map_domain_error(err.into()),
            };
            return (StatusCode::OK, Json(existing)).into_response();
        }
        Ok(None) => {}
        Err(err) => return map_domain_error(err),
    }

    let kind_str = match req.kind {
        ClipboardKind::Text => "text",
        ClipboardKind::File => "file",
    };
    let title = req
        .filename
        .clone()
        .unwrap_or_else(|| format!("clipboard:{client_msg_id}"));
    let subtitle = format!("delivered={delivered_to}/{}", targets.len());
    if let Err(err) = state.db.insert_transfer_record(
        "send",
        kind_str,
        &title,
        &subtitle,
        &sender_device_id,
        None,
        raw_size_bytes,
        timestamp,
    ) {
        return map_domain_error(err);
    }

    if explicit_targets.is_some() {
        for target_id in delivered_targets {
            let _ = state.ws_tx.send(WsServerEvent::ClipboardReceived {
                client_msg_id: Some(client_msg_id.clone()),
                from_device_id: sender_device_id.clone(),
                target_device_id: Some(target_id),
                kind: req.kind,
                content: content.clone(),
                filename: req.filename.clone(),
                encrypted: true,
                timestamp,
            });
        }
    } else if delivered_to > 0 {
        let _ = state.ws_tx.send(WsServerEvent::ClipboardReceived {
            client_msg_id: Some(client_msg_id.clone()),
            from_device_id: sender_device_id.clone(),
            target_device_id: None,
            kind: req.kind,
            content,
            filename: req.filename,
            encrypted: true,
            timestamp,
        });
    }

    let _ = state.ws_tx.send(WsServerEvent::ClipboardStatus {
        client_msg_id,
        from_device_id: sender_device_id,
        delivered_to,
        accepted_to,
        server_ack_at: timestamp,
        targets,
    });

    (StatusCode::OK, Json(response_value)).into_response()
}

async fn handle_clipboard_history(
    State(state): State<Arc<AppState>>,
    Extension(_session): Extension<SessionInfo>,
    Query(query): Query<ClipboardHistoryQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let transfers = match state.db.list_transfer_records(limit) {
        Ok(records) => records,
        Err(err) => return map_domain_error(err),
    };
    let outbox = match state
        .db
        .list_outbox_messages(query.session_key.as_deref(), limit)
    {
        Ok(records) => records,
        Err(err) => return map_domain_error(err),
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "transfers": transfers,
            "outbox": outbox,
        })),
    )
        .into_response()
}

// =============================================================================
// WebSocket Handler with Ping/Pong & Bounded Backpressure Disconnect
// =============================================================================

async fn handle_ws_upgrade(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    cookies: Cookies,
    ws: WebSocketUpgrade,
) -> Response {
    let token = cookies
        .get(SESSION_COOKIE_NAME)
        .map(|cookie| cookie.value().to_string())
        .unwrap_or_default();
    let connected_device_id = session.device_id;
    ws.on_upgrade(move |socket| serve_ws_connection(socket, state, connected_device_id, token))
}

async fn serve_ws_connection(
    socket: WebSocket,
    state: Arc<AppState>,
    connected_device_id: Option<String>,
    session_token: String,
) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(WS_OUTBOUND_QUEUE_CAPACITY);

    let mut writer_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    let mut broadcast_rx = state.ws_tx.subscribe();
    let mut telemetry_tick = tokio::time::interval(Duration::from_secs(1));
    let mut ping_tick = tokio::time::interval(Duration::from_secs(WS_PING_INTERVAL_SECS));
    let mut last_pong = Instant::now();
    if let Some(device_id) = connected_device_id.as_ref() {
        *state.online_peers.entry(device_id.clone()).or_insert(0) += 1;
    }

    loop {
        tokio::select! {
            _ = telemetry_tick.tick() => {
                match state.build_system_status() {
                    Ok(status) => match serde_json::to_string(&WsServerEvent::Telemetry(status)) {
                        Ok(json) => {
                            if out_tx.try_send(Message::Text(json.into())).is_err() {
                                warn!("closing slow WebSocket client due to backpressure");
                                break;
                            }
                        }
                        Err(err) => {
                            warn!("failed to serialize telemetry payload: {err}");
                        }
                    },
                    Err(err) => {
                        warn!("failed to build system telemetry from database: {err}");
                    }
                }
            }
            _ = ping_tick.tick() => {
                if state.auth.validate_session(&session_token).is_none() {
                    break;
                }
                if last_pong.elapsed() > Duration::from_secs(WS_PONG_TIMEOUT_SECS) {
                    warn!("closing zombie WebSocket client: Pong timeout");
                    break;
                }
                if out_tx.try_send(Message::Ping(Vec::new().into())).is_err() {
                    break;
                }
            }
            event = broadcast_rx.recv() => {
                match event {
                    Ok(ev) => {
                        if matches!(
                            ev,
                            WsServerEvent::BitRotAlert(_) | WsServerEvent::PairingRequested(_)
                        ) && connected_device_id.is_some()
                        {
                            continue;
                        }
                        if let WsServerEvent::ClipboardReceived {
                            target_device_id: Some(ref target_id),
                            ..
                        } = ev
                        {
                            if connected_device_id.as_deref() != Some(target_id.as_str()) {
                                continue;
                            }
                        }
                        if let WsServerEvent::ClipboardReceived {
                            ref from_device_id,
                            target_device_id: None,
                            ..
                        } = ev
                        {
                            if connected_device_id.as_deref() == Some(from_device_id.as_str()) {
                                continue;
                            }
                        }
                        if let WsServerEvent::ClipboardStatus {
                            ref from_device_id,
                            ..
                        } = ev
                        {
                            if let Some(ref dev_id) = connected_device_id {
                                if dev_id != from_device_id {
                                    continue;
                                }
                            }
                        }
                        if let WsServerEvent::GroupSyncUpdate { ref group_id, .. } = ev {
                            if let Some(ref dev_id) = connected_device_id {
                                if !state
                                    .db
                                    .is_active_group_member(group_id, dev_id)
                                    .unwrap_or(false)
                                {
                                    continue;
                                }
                            }
                        }
                        if let Ok(json) = serde_json::to_string(&ev) {
                            if out_tx.try_send(Message::Text(json.into())).is_err() {
                                warn!("closing slow WebSocket client during event broadcast");
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        warn!("closing lagged WebSocket client");
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = ws_receiver.next() => {
                match incoming {
                    Some(Ok(Message::Pong(_))) => {
                        last_pong = Instant::now();
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        last_pong = Instant::now();
                        if out_tx.try_send(Message::Pong(payload)).is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {
                        last_pong = Instant::now();
                    }
                }
            }
        }
    }

    if let Some(device_id) = connected_device_id.as_ref() {
        if let dashmap::mapref::entry::Entry::Occupied(mut entry) =
            state.online_peers.entry(device_id.clone())
        {
            let count = entry.get_mut();
            if *count <= 1 {
                entry.remove();
            } else {
                *count -= 1;
            }
        }
    }
    drop(out_tx);
    if tokio::time::timeout(Duration::from_secs(1), &mut writer_task)
        .await
        .is_err()
    {
        writer_task.abort();
    }
}

#[cfg(test)]
mod batch_security_tests {
    use super::*;
    use crate::scrub::BitRotAlertEvent;

    fn make_test_state() -> (Arc<AppState>, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("everyonenas-web-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        let db = Arc::new(Database::open(&dir.join("index.db")).expect("open test db"));
        let vault_store = Arc::new(VaultStore::new(Arc::clone(&db)));
        vault_store
            .create_vault(
                "Default Vault".to_string(),
                dir.join("primary").to_string_lossy().into_owned(),
                None,
                PoolMode::Single,
                10 * 1024 * 1024 * 1024,
                true,
                Vec::new(),
            )
            .expect("create default vault");
        let identity = Arc::new(
            DeviceIdentity::load_or_generate(&dir.join("device.key")).expect("load identity"),
        );
        let device_id = format!("lv-{}", &identity.fingerprint_hex()[..12]);
        let clipboard_key = identity.derive_local_storage_key("clipboard-group");
        let (ws_tx, _) = broadcast::channel::<WsServerEvent>(32);
        let (alert_tx, _) = broadcast::channel::<BitRotAlertEvent>(16);
        let scrubber = Arc::new(BitRotScrubber::new(
            Arc::clone(&db),
            Arc::clone(&vault_store),
            alert_tx,
        ));
        let auth = Arc::new(
            AuthManager::from_plaintext_password("test-password-1234", Arc::clone(&identity))
                .expect("auth manager"),
        );
        let state = Arc::new(AppState {
            device_id,
            identity,
            clipboard_key,
            os_name: "linux".to_string(),
            keep_alive_level: "TEST".to_string(),
            lan_ip: "127.0.0.1".to_string(),
            port: 7842,
            mdns_active: AtomicBool::new(false),
            started_at: Instant::now(),
            throughput_bps: AtomicU64::new(0),
            transfer_bytes: AtomicU64::new(0),
            throughput_sample: StdMutex::new((Instant::now(), 0)),
            active_chunk_writes: AtomicU64::new(0),
            upload_semaphore: tokio::sync::Semaphore::new(8),
            accepting_tasks: AtomicBool::new(true),
            pairing_pin: StdMutex::new("123456".to_string()),
            pending_pair_lock: StdMutex::new(()),
            pending_pair_requests: DashMap::new(),
            renew_challenges: DashMap::new(),
            renew_rate_limits: DashMap::new(),
            transfer_budget_lock: StdMutex::new(()),
            transfer_locks: std::array::from_fn(|_| Arc::new(tokio::sync::RwLock::new(()))),
            online_peers: DashMap::new(),
            peer_leases: DashMap::new(),
            db,
            vault_store,
            scrubber,
            auth,
            ws_tx,
            restart_notify: Arc::new(Notify::new()),
            telemetry_cache: RwLock::new(None),
        });
        (state, dir)
    }

    fn insert_fixture_peer(state: &AppState, device_id: &str) {
        state
            .db
            .upsert_trusted_peer(&TrustedPeer {
                device_id: device_id.to_string(),
                device_name: device_id.to_string(),
                os_type: "test".into(),
                ed25519_pubkey: vec![0; 32],
                last_ip: None,
                last_port: None,
                auto_connect: false,
                paired_at: unix_timestamp_secs(),
                revoked_at: None,
            })
            .expect("seed trusted device");
    }

    fn dummy_pair_entry(
        req_id: &str,
        device_id: &str,
        ip: &str,
        now: i64,
        ttl: i64,
    ) -> PendingPairEntry {
        PendingPairEntry {
            dto: PendingPairRequestDto {
                request_id: req_id.to_string(),
                device_id: device_id.to_string(),
                device_name: "TestDev".to_string(),
                os_type: "android".to_string(),
                fingerprint: "0123456789abcdef0123456789abcdef".to_string(),
                remote_ip: ip.to_string(),
                created_at: now,
                expires_at: now + ttl,
            },
            poll_token: "poll".to_string(),
            nonce: "nonce".to_string(),
            client_ed25519_pubkey: [1u8; 32],
            client_x25519_pubkey: [2u8; 32],
            identity_message: b"msg".to_vec(),
            status: PendingPairStatus::Pending,
        }
    }

    #[test]
    fn pair_queue_caps_single_ip_and_preserves_other_sources() {
        let (state, dir) = make_test_state();
        let now = unix_timestamp_secs();

        // Same IP fills up to per-IP cap with distinct devices
        for i in 0..MAX_PENDING_PAIR_REQUESTS_PER_IP {
            let entry = dummy_pair_entry(
                &format!("r-{i}"),
                &format!("lv-attacker{i}"),
                "192.168.1.50",
                now,
                60,
            );
            assert!(state.enqueue_pair_request(entry, now).is_ok());
        }
        // Extra device from same IP is rejected
        let extra = dummy_pair_entry("r-extra", "lv-attacker99", "192.168.1.50", now, 60);
        assert_eq!(
            state.enqueue_pair_request(extra, now),
            Err(PairQueueRejectReason::SourceLimitReached)
        );
        // Repeated click from existing device on same IP stays idempotent
        let repeat = dummy_pair_entry("r-repeat", "lv-attacker0", "192.168.1.50", now, 60);
        assert!(state.enqueue_pair_request(repeat, now).is_ok());

        // Legitimate second IP can still enqueue
        let legit = dummy_pair_entry("r-legit", "lv-legit001", "192.168.1.60", now, 60);
        assert!(state.enqueue_pair_request(legit, now).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pair_queue_evicts_expired_and_enforces_global_cap() {
        let (state, dir) = make_test_state();
        let now = unix_timestamp_secs();

        for i in 0..MAX_PENDING_PAIR_REQUESTS {
            let entry = dummy_pair_entry(
                &format!("g-{i}"),
                &format!("lv-dev{i:04}"),
                &format!("10.0.0.{}", i + 1),
                now,
                60,
            );
            assert!(state.enqueue_pair_request(entry, now).is_ok());
        }
        let overflow = dummy_pair_entry("g-over", "lv-over", "10.0.1.1", now, 60);
        assert_eq!(
            state.enqueue_pair_request(overflow, now),
            Err(PairQueueRejectReason::GlobalLimitReached)
        );

        // After TTL passes, expired entries are evicted automatically
        let after_ttl = dummy_pair_entry("g-after", "lv-after", "10.0.1.2", now + 61, 60);
        assert!(state.enqueue_pair_request(after_ttl, now + 61).is_ok());
        assert_eq!(state.pending_pair_requests.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renew_challenge_is_bound_single_use_and_rejects_revoked_and_concurrent_replay() {
        let (state, dir) = make_test_state();
        let peer_id = "lv-peer11112222".to_string();
        state
            .db
            .upsert_trusted_peer(&TrustedPeer {
                device_id: peer_id.clone(),
                device_name: "Phone".to_string(),
                os_type: "android".to_string(),
                ed25519_pubkey: vec![9u8; 32],
                last_ip: Some("192.168.1.20".to_string()),
                last_port: Some(7842),
                auto_connect: true,
                paired_at: unix_timestamp_secs(),
                revoked_at: None,
            })
            .expect("insert trusted peer");

        // Unknown device rejected
        assert!(state.issue_renew_challenge("lv-unknown").is_err());

        // Valid challenge issued, mismatched device rejected without succeeding, replay rejected
        let chal = state
            .issue_renew_challenge(&peer_id)
            .expect("issue challenge");
        assert!(!state.consume_renew_challenge(&chal, "lv-other"));
        // Even after failed mismatch, challenge was consumed/removed to prevent probing
        assert!(!state.consume_renew_challenge(&chal, &peer_id));

        // Concurrent race: only 1 of 8 threads may succeed on the same challenge
        let chal_race = state
            .issue_renew_challenge(&peer_id)
            .expect("issue race challenge");
        let wins = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let st = Arc::clone(&state);
            let ch = chal_race.clone();
            let pid = peer_id.clone();
            let w = Arc::clone(&wins);
            handles.push(std::thread::spawn(move || {
                if st.consume_renew_challenge(&ch, &pid) {
                    w.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for h in handles {
            h.join().expect("join thread");
        }
        assert_eq!(wins.load(Ordering::SeqCst), 1);

        // Expired challenge rejected
        state.renew_challenges.insert(
            "expired-chal".to_string(),
            RenewChallengeEntry {
                device_id: peer_id.clone(),
                server_device_id: state.device_id.clone(),
                expires_at: Instant::now() - Duration::from_secs(1),
            },
        );
        assert!(!state.consume_renew_challenge("expired-chal", &peer_id));

        // Revoked device rejected
        state.auth.revoke_device_sessions(&peer_id);
        assert!(state.issue_renew_challenge(&peer_id).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transfer_owner_isolation_and_incremental_parts_work() {
        let (state, dir) = make_test_state();
        let now = unix_timestamp_secs();
        let client_tid = "deadbeef00112233";
        let scoped_a = scoped_transfer_id("lv-device-a", client_tid);
        let scoped_b = scoped_transfer_id("lv-device-b", client_tid);
        assert_ne!(scoped_a, scoped_b);

        let rec_a = lanvault_core::TransferSessionRecord {
            transfer_id: scoped_a.clone(),
            owner_device_id: "lv-device-a".to_string(),
            filename: "report.pdf".to_string(),
            total_size: 1024,
            chunk_size: 512,
            total_parts: 2,
            received_parts: Vec::new(),
            file_blake3: "a".repeat(64),
            encoding: "raw".to_string(),
            status: "active".to_string(),
            saved_path: None,
            created_at: now,
            updated_at: now,
            expires_at: now + 3600,
        };
        let rec_b = lanvault_core::TransferSessionRecord {
            transfer_id: scoped_b.clone(),
            owner_device_id: "lv-device-b".to_string(),
            ..rec_a.clone()
        };
        state.db.upsert_transfer_session(&rec_a).expect("upsert a");
        state.db.upsert_transfer_session(&rec_b).expect("upsert b");

        // Incremental part recording is idempotent and sorted
        state
            .db
            .mark_transfer_part_received(&scoped_a, 1, 512, now)
            .expect("part 1");
        state
            .db
            .mark_transfer_part_received(&scoped_a, 1, 512, now + 1)
            .expect("dup part 1");
        state
            .db
            .mark_transfer_part_received(&scoped_a, 0, 512, now + 2)
            .expect("part 0");

        let loaded_a = state
            .db
            .get_transfer_session(&scoped_a)
            .expect("get a")
            .expect("some a");
        let loaded_b = state
            .db
            .get_transfer_session(&scoped_b)
            .expect("get b")
            .expect("some b");
        assert_eq!(loaded_a.received_parts, vec![0, 1]);
        assert!(loaded_b.received_parts.is_empty());

        // Atomic completion with history releases active budget
        let (oc_before, ob_before, gc_before, gb_before) = state
            .db
            .active_transfer_usage("lv-device-a")
            .expect("usage before");
        assert_eq!(
            (oc_before, ob_before, gc_before, gb_before),
            (1, 1024, 2, 2048)
        );

        state
            .db
            .reserve_transfer_destination(&scoped_a, "/tmp/report.pdf")
            .expect("reserve a");
        assert!(state
            .db
            .reserve_transfer_destination(&scoped_b, "/tmp/report.pdf")
            .is_err());
        state
            .db
            .complete_transfer_session_with_history(
                &scoped_a,
                "/tmp/report.pdf",
                "report.pdf",
                "Device A",
                1024,
                now + 5,
            )
            .expect("complete a");
        let (oc_after, ob_after, gc_after, gb_after) = state
            .db
            .active_transfer_usage("lv-device-a")
            .expect("usage after");
        assert_eq!((oc_after, ob_after, gc_after, gb_after), (0, 0, 1, 1024));

        // Expected part lengths reject short non-final parts and support zero-byte files
        assert_eq!(expected_transfer_part_len(0, 512, 1, 0), Some(0));
        assert_eq!(expected_transfer_part_len(0, 512, 1, 1), None);
        assert_eq!(expected_transfer_part_len(1000, 512, 2, 0), Some(512));
        assert_eq!(expected_transfer_part_len(1000, 512, 2, 1), Some(488));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn transfer_commit_recovers_published_file_and_writes_history_once() {
        let (state, _dir) = make_test_state();
        let tid = "published-transfer";
        let scoped_id = scoped_transfer_id("__admin__", tid);
        let (_, received) = resolve_transfer_dirs(&state).expect("transfer dirs");
        let dest = received.join("recovered.txt");
        let contents = b"recovered transfer";
        fs::write(&dest, contents).expect("published file");
        let now = unix_timestamp_secs();
        let record = lanvault_core::TransferSessionRecord {
            transfer_id: scoped_id.clone(),
            owner_device_id: "__admin__".into(),
            filename: "recovered.txt".into(),
            total_size: contents.len() as u64,
            chunk_size: contents.len() as u64,
            total_parts: 1,
            received_parts: vec![0],
            file_blake3: hex::encode(blake3_hash(contents)),
            encoding: "raw".into(),
            status: "active".into(),
            saved_path: None,
            created_at: now - 7200,
            updated_at: now - 7200,
            expires_at: now - 3600,
        };
        state
            .db
            .upsert_transfer_session(&record)
            .expect("transfer session");
        state
            .db
            .reserve_transfer_destination(&scoped_id, &dest.to_string_lossy())
            .expect("persist publication");
        state
            .reclaim_expired_transfers(now)
            .expect("cleanup preserves pending publication");
        for _ in 0..2 {
            let response = handle_transfer_commit(
                State(Arc::clone(&state)),
                Extension(SessionInfo {
                    is_admin: true,
                    device_id: None,
                }),
                Path(tid.to_string()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
        }
        assert_eq!(
            state.db.list_transfer_records(10).expect("history").len(),
            1
        );
        assert_eq!(
            received_usage(&received).expect("retained usage"),
            (1, contents.len() as u64)
        );
    }

    #[test]
    fn allocate_unique_received_path_avoids_overwriting_existing_files() {
        let dir =
            std::env::temp_dir().join(format!("everyonenas-recv-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create dir");
        let first = allocate_unique_received_path(&dir, "photo.jpg", &[]);
        assert_eq!(
            first.file_name().and_then(|s| s.to_str()),
            Some("photo.jpg")
        );
        let reserved = [first.to_string_lossy().into_owned()];
        assert_ne!(
            allocate_unique_received_path(&dir, "photo.jpg", &reserved),
            first
        );
        std::fs::write(&first, b"1").expect("write first");

        let second = allocate_unique_received_path(&dir, "photo.jpg", &[]);
        assert_eq!(
            second.file_name().and_then(|s| s.to_str()),
            Some("photo (1).jpg")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn chunk_compression_is_atomic_and_immutable_and_cross_vault_isolated() {
        let (state, dir) = make_test_state();
        insert_fixture_peer(&state, "lv-peer-a");
        insert_fixture_peer(&state, "lv-peer-b");
        let vaults = state.db.list_vaults().expect("list vaults");
        let default_vault_id = vaults[0].vault_id.clone();

        // Create a secondary vault
        let secondary_dir = dir.join("secondary");
        std::fs::create_dir_all(&secondary_dir).expect("create secondary dir");
        let (secondary, _) = state
            .vault_store
            .create_vault(
                "Private Vault".to_string(),
                secondary_dir.to_string_lossy().into_owned(),
                None,
                PoolMode::Single,
                10 * 1024 * 1024,
                true,
                Vec::new(),
            )
            .expect("create secondary vault");

        // Unprivileged device can access default vault, but NOT secondary vault without grant
        assert!(state
            .db
            .is_device_authorized_for_vault(&default_vault_id, "lv-peer-a")
            .expect("auth default"));
        assert!(!state
            .db
            .is_device_authorized_for_vault(&secondary.vault_id, "lv-peer-a")
            .expect("deny secondary"));

        // Granting secondary vault authorizes peer-a while still denying peer-b
        let grant_path = (secondary.vault_id.clone(), "lv-peer-a".to_string());
        let denied = handle_grant_vault_access(
            State(Arc::clone(&state)),
            Extension(SessionInfo {
                is_admin: false,
                device_id: Some("lv-peer-b".into()),
            }),
            Path(grant_path.clone()),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let granted = handle_grant_vault_access(
            State(Arc::clone(&state)),
            Extension(SessionInfo {
                is_admin: true,
                device_id: None,
            }),
            Path(grant_path),
        )
        .await;
        assert_eq!(granted.status(), StatusCode::NO_CONTENT);
        assert!(state
            .db
            .is_device_authorized_for_vault(&secondary.vault_id, "lv-peer-a")
            .expect("allow granted"));
        assert!(!state
            .db
            .is_device_authorized_for_vault(&secondary.vault_id, "lv-peer-b")
            .expect("deny ungranted"));

        // Put raw chunk atomically, dedup with same tag succeeds, conflicting zstd tag rejected
        let raw_payload = b"raw-encrypted-chunk-bytes";
        let raw_hash = blake3_hash(raw_payload);
        assert!(state
            .vault_store
            .put_chunk_with_compression(
                &default_vault_id,
                &raw_hash,
                raw_payload.len() as u64,
                raw_payload,
                "raw"
            )
            .expect("put raw"));
        assert_eq!(
            state.db.get_chunk_compression(&default_vault_id, &raw_hash),
            "raw"
        );
        // Same-tag dedup retry succeeds
        assert!(!state
            .vault_store
            .put_chunk_with_compression(
                &default_vault_id,
                &raw_hash,
                raw_payload.len() as u64,
                raw_payload,
                "raw"
            )
            .expect("dedup raw"));
        // Conflicting tag is rejected and does NOT overwrite stored metadata
        assert!(state
            .vault_store
            .put_chunk_with_compression(
                &default_vault_id,
                &raw_hash,
                raw_payload.len() as u64,
                raw_payload,
                "zstd"
            )
            .is_err());
        assert_eq!(
            state.db.get_chunk_compression(&default_vault_id, &raw_hash),
            "raw"
        );

        // Chunk read isolation: only owning snapshot's device can read chunk
        let snap = SnapshotRecord {
            snapshot_id: "snap-owner-a".to_string(),
            vault_id: default_vault_id.clone(),
            rule_id: "rule-1".to_string(),
            device_id: "lv-peer-a".to_string(),
            root_tree_hash: [7u8; 32],
            retention_tier: RetentionTier::Daily,
            file_count: 1,
            total_bytes: raw_payload.len() as u64,
            created_at: unix_timestamp_secs(),
        };
        state
            .db
            .insert_snapshot_with_chunks(&snap, &[raw_hash])
            .expect("create snapshot");
        assert!(state
            .db
            .is_chunk_accessible_to_device(&default_vault_id, &raw_hash, "lv-peer-a")
            .expect("owner can read"));
        assert!(!state
            .db
            .is_chunk_accessible_to_device(&default_vault_id, &raw_hash, "lv-peer-b")
            .expect("other device denied"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_pagination_pushdown_isolates_devices_and_handles_ties() {
        let (state, dir) = make_test_state();
        insert_fixture_peer(&state, "lv-peer-a");
        insert_fixture_peer(&state, "lv-peer-b");
        let vault_id = state.db.list_vaults().expect("vaults")[0].vault_id.clone();

        // Insert three snapshots for peer A (including equal timestamps) and two for peer B.
        let ts = 1_700_000_000i64;
        for (idx, (sid, dev, created_at)) in [
            ("snap-a1", "lv-peer-a", ts),
            ("snap-a2", "lv-peer-a", ts),
            ("snap-a3", "lv-peer-a", ts + 10),
            ("snap-b1", "lv-peer-b", ts + 20),
            ("snap-b2", "lv-peer-b", ts + 30),
        ]
        .into_iter()
        .enumerate()
        {
            let snap = SnapshotRecord {
                snapshot_id: sid.to_string(),
                vault_id: vault_id.clone(),
                rule_id: format!("r-{idx}"),
                device_id: dev.to_string(),
                root_tree_hash: [idx as u8; 32],
                retention_tier: RetentionTier::Daily,
                file_count: 0,
                total_bytes: 0,
                created_at,
            };
            state
                .db
                .insert_snapshot_with_chunks(&snap, &[])
                .expect("insert snap");
        }

        // Peer A sees only its own snapshots in deterministic DESC order across pages
        let page1 = state
            .db
            .list_snapshots_filtered_paginated(Some(&vault_id), Some("lv-peer-a"), 2, 0)
            .expect("page 1");
        let page2 = state
            .db
            .list_snapshots_filtered_paginated(Some(&vault_id), Some("lv-peer-a"), 2, 2)
            .expect("page 2");
        let page3 = state
            .db
            .list_snapshots_filtered_paginated(Some(&vault_id), Some("lv-peer-a"), 2, 4)
            .expect("page 3");
        let ids1: Vec<&str> = page1.iter().map(|s| s.snapshot_id.as_str()).collect();
        let ids2: Vec<&str> = page2.iter().map(|s| s.snapshot_id.as_str()).collect();
        assert_eq!(ids1, vec!["snap-a3", "snap-a2"]);
        assert_eq!(ids2, vec!["snap-a1"]);
        assert!(page3.is_empty());

        // Admin (device_id = None) sees all 5 snapshots
        let admin_all = state
            .db
            .list_snapshots_filtered_paginated(Some(&vault_id), None, 10, 0)
            .expect("admin all");
        assert_eq!(admin_all.len(), 5);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renew_challenge_rate_limits_and_does_not_evict_other_devices() {
        let (state, dir) = make_test_state();
        insert_fixture_peer(&state, "lv-peer-victim");
        insert_fixture_peer(&state, "lv-peer-flood");
        let ip: IpAddr = "192.168.1.99".parse().unwrap();

        let victim_chal = state
            .issue_renew_challenge_from_ip(Some(ip), "lv-peer-victim")
            .expect("victim challenge");

        // Per-device active cap rejects excessive challenges without evicting victim
        for _ in 0..MAX_RENEW_CHALLENGES_ACTIVE_PER_DEVICE {
            state
                .issue_renew_challenge_from_ip(Some(ip), "lv-peer-flood")
                .expect("flood within active cap");
        }
        let err = state
            .issue_renew_challenge_from_ip(Some(ip), "lv-peer-flood")
            .expect_err("must rate limit above active cap");
        assert!(matches!(err, LanVaultError::RateLimited { .. }));

        // Victim challenge remains intact
        assert!(state.consume_renew_challenge(&victim_chal, "lv-peer-victim"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
