//! Axum 0.8 HTTP REST API, Cookie-authenticated WebSocket telemetry/alerts with
//! Ping/Pong backpressure disconnection, conditional `static-serve` Web UI embedding,
//! and graceful restart orchestration.
//!
//! 100% aligned with `ARCHITECTURE_CONTRACTS.md` §3.1 (REST endpoints + `/ws`).

mod debug_routes;
use debug_routes::*;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Component, Path as StdPath, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State};
use axum::http::header::{HeaderName, HeaderValue, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use std::fs::{self, File};
use std::io::Write;

use dashmap::{DashMap, DashSet};
use futures_util::{SinkExt, StreamExt};
use lanvault_core::crypto::constant_time_eq;
use lanvault_core::{
    blake3_hash, public_key_fingerprint, ClipboardKind, Database, LanVaultError, PoolMode,
    RetentionTier, SessionKeyExchange, SnapshotRecord, SymmetricKey, TrustedPeer, VolumeRole,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, Notify};
use tower_cookies::{CookieManagerLayer, Cookies};
use tracing::{info, warn};

use crate::auth::{
    build_clear_session_cookie, build_session_cookie, AuthManager, AuthOutcome, LoginOutcome,
    SESSION_COOKIE_NAME,
};
use crate::scrub::{BitRotAlertEvent, BitRotScrubber, ScrubReport};
use crate::vault::{unix_timestamp_secs, ExtraVolumeSpec, VaultStore};

#[cfg(feature = "embed-web-ui")]
static_serve::embed_assets!("web-ui");

const WS_PING_INTERVAL_SECS: u64 = 15;
const WS_PONG_TIMEOUT_SECS: u64 = 30;
const WS_OUTBOUND_QUEUE_CAPACITY: usize = 32;
const GRACEFUL_DRAIN_SECS: u64 = 3;
const MAX_CHUNK_UPLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_CLIPBOARD_BODY_BYTES: usize = 16 * 1024 * 1024;

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
        self.state.active_chunk_writes.fetch_sub(1, Ordering::SeqCst);
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
        from_device_id: String,
        target_device_id: Option<String>,
        kind: ClipboardKind,
        content: String,
        filename: Option<String>,
        timestamp: i64,
    },
}

/// Shared runtime state injected into all Axum handlers.
pub struct AppState {
    pub device_id: String,
    pub os_name: String,
    pub keep_alive_level: String,
    pub lan_ip: String,
    pub port: u16,
    pub mdns_active: AtomicBool,
    pub started_at: Instant,
    pub throughput_bps: AtomicU64,
    pub active_chunk_writes: AtomicU64,
    pub accepting_tasks: AtomicBool,
    pub pairing_pin: String,
    pub online_peers: DashSet<String>,
    pub peer_session_keys: DashMap<String, SymmetricKey>,
    pub db: Arc<Database>,
    pub vault_store: Arc<VaultStore>,
    pub scrubber: Arc<BitRotScrubber>,
    pub auth: Arc<AuthManager>,
    pub ws_tx: broadcast::Sender<WsServerEvent>,
    pub restart_notify: Arc<Notify>,
}

impl AppState {
    /// Builds a snapshot of current system telemetry matching `GET /api/system/status`.
    pub fn build_system_status(&self) -> Result<SystemStatusResponse, LanVaultError> {
        let corrupted_chunks = self.db.count_corrupted_chunks()?;
        let vaults = self.db.list_vaults()?;
        let mut vault_used_bytes = 0u64;
        let mut vault_quota_bytes = 0u64;
        let mut any_volume_offline = false;

        for v in &vaults {
            vault_used_bytes = vault_used_bytes.saturating_add(v.used_bytes);
            vault_quota_bytes = vault_quota_bytes.saturating_add(v.quota_bytes);
            let vols = self.db.list_vault_volumes(&v.vault_id)?;
            if vols.iter().any(|vol| !vol.is_online) {
                any_volume_offline = true;
            }
        }

        let bit_rot_health = if corrupted_chunks > 0 {
            "corrupted"
        } else if any_volume_offline {
            "degraded"
        } else {
            "healthy"
        };

        Ok(SystemStatusResponse {
            os: self.os_name.clone(),
            keep_alive_level: self.keep_alive_level.clone(),
            lan_ip: self.lan_ip.clone(),
            port: self.port,
            mdns_active: self.mdns_active.load(Ordering::Relaxed),
            uptime_secs: self.started_at.elapsed().as_secs(),
            throughput_bps: self.throughput_bps.load(Ordering::Relaxed),
            bit_rot_health: bit_rot_health.to_string(),
            corrupted_chunks,
            vault_used_bytes,
            vault_quota_bytes,
        })
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
    let api_router = Router::new()
        .route("/api/auth/login", post(handle_login))
        .route("/api/auth/logout", post(handle_logout))
        .route("/api/system/status", get(handle_system_status))
        .route("/api/system/restart", post(handle_system_restart))
        .route("/api/vaults", get(handle_list_vaults).post(handle_create_vault))
        .route(
            "/api/vaults/{vault_id}/chunks/{hash_hex}",
            put(handle_put_chunk).layer(DefaultBodyLimit::max(MAX_CHUNK_UPLOAD_BYTES)),
        )
        .route(
            "/api/vaults/{vault_id}/snapshots",
            post(handle_create_snapshot),
        )
        .route("/api/snapshots", get(handle_list_snapshots))
        .route(
            "/api/snapshots/{snapshot_id}/file",
            get(handle_snapshot_file_download),
        )
        .route("/api/scrub/trigger", post(handle_scrub_trigger))
        .route("/api/peers", get(handle_list_peers))
        .route("/api/peers/pair", post(handle_pair_peer))
        .route("/api/peers/{device_id}", delete(handle_delete_peer))
        .route(
            "/api/clipboard",
            post(handle_clipboard_push).layer(DefaultBodyLimit::max(MAX_CLIPBOARD_BODY_BYTES)),
        )
        .route("/api/debug/config", get(handle_get_debug_config).post(handle_set_debug_config))
        .route("/api/debug/logs", get(handle_get_debug_logs).delete(handle_clear_debug_logs))
        .route("/api/debug/logs/client", post(handle_report_client_log))
        .route("/ws", get(handle_ws_upgrade))
        .layer(CookieManagerLayer::new())
        .with_state(state);

    #[cfg(feature = "embed-web-ui")]
    {
        api_router.merge(static_router::<()>())
    }
    #[cfg(not(feature = "embed-web-ui"))]
    {
        api_router
    }
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
fn require_auth(state: &AppState, cookies: &Cookies) -> Result<String, Response> {
    let token = cookies
        .get(SESSION_COOKIE_NAME)
        .map(|c| c.value().to_string());
    match token {
        Some(t) if state.auth.validate_session(&t) => Ok(t),
        _ => Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "unauthorized" })),
        )
            .into_response()),
    }
}

fn map_domain_error(err: LanVaultError) -> Response {
    let status = match &err {
        LanVaultError::InvalidInput(_) => StatusCode::BAD_REQUEST,
        LanVaultError::NotFound(_) => StatusCode::NOT_FOUND,
        LanVaultError::QuotaExceeded { .. } => StatusCode::INSUFFICIENT_STORAGE,
        LanVaultError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
        LanVaultError::Auth(_) => StatusCode::UNAUTHORIZED,
        LanVaultError::CorruptedChunk { .. } => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(serde_json::json!({ "error": err.to_string() }))).into_response()
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

fn validate_relative_snapshot_path(raw: &str) -> Result<PathBuf, LanVaultError> {
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
    pub device_id: String,
    pub root_tree_hash_hex: String,
    pub e2ee_enabled: bool,
    pub created_at: i64,
    pub files: Vec<SnapshotManifestFile>,
}

#[derive(Debug, Deserialize)]
struct CreateSnapshotRequest {
    snapshot_id: Option<String>,
    device_id: String,
    root_tree_hash_hex: Option<String>,
    #[serde(default)]
    retention_tier: Option<RetentionTier>,
    files: Vec<SnapshotManifestFile>,
}

#[derive(Debug, Deserialize)]
struct WsConnectQuery {
    device_id: Option<String>,
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
    device_name: String,
    os_type: String,
}

#[derive(Debug, Serialize)]
struct PairPinResponse {
    device_id: String,
    server_x25519_pubkey_hex: String,
    session_cookie: Option<String>,
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
    target_device_id: Option<String>,
    kind: ClipboardKind,
    content: String,
    filename: Option<String>,
}

// =============================================================================
// REST Handlers
// =============================================================================

async fn handle_login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
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
            cookies.add(build_session_cookie(token));
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

async fn handle_logout(State(state): State<Arc<AppState>>, cookies: Cookies) -> Response {
    let token = match require_auth(&state, &cookies) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    state.auth.revoke_session(&token);
    cookies.add(build_clear_session_cookie());
    (
        StatusCode::OK,
        Json(serde_json::json!({ "authenticated": false })),
    )
        .into_response()
}

async fn handle_system_status(State(state): State<Arc<AppState>>, cookies: Cookies) -> Response {
    if let Err(resp) = require_auth(&state, &cookies) {
        return resp;
    }
    match state.build_system_status() {
        Ok(status) => (StatusCode::OK, Json(status)).into_response(),
        Err(err) => map_domain_error(err),
    }
}

async fn handle_system_restart(State(state): State<Arc<AppState>>, cookies: Cookies) -> Response {
    if let Err(resp) = require_auth(&state, &cookies) {
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

async fn handle_list_vaults(State(state): State<Arc<AppState>>, cookies: Cookies) -> Response {
    if let Err(resp) = require_auth(&state, &cookies) {
        return resp;
    }
    let vaults = match state.db.list_vaults() {
        Ok(v) => v,
        Err(err) => return map_domain_error(err),
    };

    let mut result = Vec::with_capacity(vaults.len());
    for v in vaults {
        let volumes = match state.db.list_vault_volumes(&v.vault_id) {
            Ok(vols) => vols,
            Err(err) => return map_domain_error(err),
        };
        result.push(VaultDto {
            vault_id: v.vault_id,
            name: v.name,
            pool_mode: v.pool_mode,
            quota_bytes: v.quota_bytes,
            used_bytes: v.used_bytes,
            e2ee_enabled: v.e2ee_enabled,
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
        });
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({ "vaults": result })),
    )
        .into_response()
}

async fn handle_create_vault(
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
    Json(req): Json<CreateVaultRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_auth(&state, &cookies) {
        return resp;
    }
    match state.vault_store.create_vault(
        req.name,
        req.mount_path,
        req.volume_guid,
        req.pool_mode,
        req.quota_bytes,
        req.e2ee_enabled,
        req.extra_volumes,
    ) {
        Ok((vault, volumes)) => {
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
        Err(err) => map_domain_error(err),
    }
}

async fn handle_put_chunk(
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
    Path((vault_id, hash_hex)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_auth(&state, &cookies) {
        return resp;
    }

    let expected_hash = match decode_hex_32(&hash_hex, "hash_hex") {
        Ok(h) => h,
        Err(err) => return map_domain_error(err),
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

    let _write_guard = ActiveChunkWriteGuard::new(Arc::clone(&state));
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }

    let stored_size = body.len() as u64;
    let vault_store = Arc::clone(&state.vault_store);
    let vid = vault_id.clone();

    let join_res = tokio::task::spawn_blocking(move || {
        vault_store.put_chunk(&vid, &expected_hash, raw_size, &body)
    })
    .await;

    match join_res {
        Ok(Ok(is_new)) => {
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

async fn handle_list_snapshots(
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
    Query(query): Query<SnapshotQuery>,
) -> Response {
    if let Err(resp) = require_auth(&state, &cookies) {
        return resp;
    }
    match state.db.list_snapshots(query.vault_id.as_deref()) {
        Ok(snaps) => {
            let dtos: Vec<SnapshotDto> = snaps
                .into_iter()
                .map(|s| SnapshotDto {
                    snapshot_id: s.snapshot_id,
                    vault_id: s.vault_id,
                    device_id: s.device_id,
                    retention_tier: s.retention_tier.as_str().to_string(),
                    file_count: s.file_count,
                    total_bytes: s.total_bytes,
                    created_at: s.created_at,
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({ "snapshots": dtos })),
            )
                .into_response()
        }
        Err(err) => map_domain_error(err),
    }
}

async fn handle_create_snapshot(
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
    Path(vault_id): Path<String>,
    Json(req): Json<CreateSnapshotRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_auth(&state, &cookies) {
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

    let explicit_root_hash = match req.root_tree_hash_hex.as_deref() {
        Some(hex_str) => match decode_hex_32(hex_str, "root_tree_hash_hex") {
            Ok(h) => Some(h),
            Err(err) => return map_domain_error(err),
        },
        None => None,
    };

    let mut normalized_files = Vec::with_capacity(req.files.len());
    let mut unique_hashes: HashSet<[u8; 32]> = HashSet::new();
    for file in req.files {
        let clean_path = match validate_relative_snapshot_path(&file.rel_path) {
            Ok(p) => normalize_rel_path(&p),
            Err(err) => return map_domain_error(err),
        };
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

    let retention_tier = req.retention_tier.unwrap_or(RetentionTier::Daily);
    let device_id = req.device_id;
    let db = Arc::clone(&state.db);
    let vid = vault_id.clone();

    let join_res = tokio::task::spawn_blocking(move || -> lanvault_core::Result<SnapshotRecord> {
        let vault = db
            .get_vault(&vid)?
            .ok_or_else(|| LanVaultError::NotFound(format!("vault {vid}")))?;
        if db.get_trusted_peer(&device_id)?.is_none() {
            return Err(LanVaultError::NotFound(format!("trusted peer {device_id}")));
        }

        for hash in &unique_hashes {
            if db.get_chunk(&vid, hash)?.is_none() {
                return Err(LanVaultError::NotFound(format!(
                    "chunk {} in vault {vid}",
                    hex::encode(hash)
                )));
            }
        }

        let volumes = db.list_vault_volumes(&vid)?;
        let primary = volumes
            .iter()
            .find(|v| v.is_online && v.role == VolumeRole::Primary)
            .or_else(|| volumes.iter().find(|v| v.is_online))
            .ok_or_else(|| LanVaultError::NotFound(format!("online volume for vault {vid}")))?;

        let created_at = unix_timestamp_secs();
        let file_count = normalized_files.len() as u64;
        let total_bytes = normalized_files
            .iter()
            .fold(0u64, |acc, f| acc.saturating_add(f.size));

        let root_tree_hash = match explicit_root_hash {
            Some(h) => h,
            None => {
                let canonical_bytes = serde_json::to_vec(&normalized_files)?;
                blake3_hash(&canonical_bytes)
            }
        };

        let manifest = SnapshotManifest {
            snapshot_id: snapshot_id.clone(),
            vault_id: vid.clone(),
            device_id: device_id.clone(),
            root_tree_hash_hex: hex::encode(root_tree_hash),
            e2ee_enabled: vault.e2ee_enabled,
            created_at,
            files: normalized_files,
        };

        let root_dir = StdPath::new(&primary.mount_path);
        let manifests_dir = root_dir.join("manifests");
        let staging_dir = root_dir.join("staging");
        fs::create_dir_all(&manifests_dir)?;
        fs::create_dir_all(&staging_dir)?;

        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        let tmp_path = staging_dir.join(format!("manifest.{}.tmp", uuid::Uuid::new_v4()));
        let final_path = manifests_dir.join(format!("{snapshot_id}.json"));
        {
            let mut file = File::create(&tmp_path)?;
            file.write_all(&manifest_bytes)?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, &final_path)?;

        let record = SnapshotRecord {
            snapshot_id,
            vault_id: vid,
            device_id,
            root_tree_hash,
            retention_tier,
            file_count,
            total_bytes,
            created_at,
        };

        if let Err(err) = db.insert_snapshot(&record) {
            let _ = fs::remove_file(&final_path);
            return Err(err);
        }

        Ok(record)
    })
    .await;

    match join_res {
        Ok(Ok(s)) => (
            StatusCode::CREATED,
            Json(SnapshotDto {
                snapshot_id: s.snapshot_id,
                vault_id: s.vault_id,
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
    cookies: Cookies,
    Path(snapshot_id): Path<String>,
    Query(query): Query<SnapshotFileQuery>,
) -> Response {
    if let Err(resp) = require_auth(&state, &cookies) {
        return resp;
    }

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

        let vault = db
            .get_vault(&vault_id)?
            .ok_or_else(|| LanVaultError::NotFound(format!("vault {vault_id}")))?;

        let volumes = db.list_vault_volumes(&vault_id)?;
        let primary = volumes
            .iter()
            .find(|v| v.is_online && v.role == VolumeRole::Primary)
            .or_else(|| volumes.iter().find(|v| v.is_online))
            .ok_or_else(|| LanVaultError::NotFound(format!("online volume for vault {vault_id}")))?;

        let manifest_path = StdPath::new(&primary.mount_path)
            .join("manifests")
            .join(format!("{snap_id}.json"));
        let manifest_bytes = fs::read(&manifest_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                LanVaultError::NotFound(format!("snapshot manifest for {snap_id}"))
            } else {
                LanVaultError::Io(e)
            }
        })?;
        let manifest: SnapshotManifest = serde_json::from_slice(&manifest_bytes)?;

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
            let disposition = format!("attachment; filename=\"{filename}\"");
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
                    let bytes = tokio::task::spawn_blocking(move || store_bg.get_chunk(&vid_bg, &hash))
                        .await
                        .map_err(|e| std::io::Error::other(format!("chunk stream join error: {e}")))?
                        .map_err(|e| std::io::Error::other(e.to_string()))?;
                    Ok(Some((axum::body::Bytes::from(bytes), (iter, store, vid))))
                },
            );
            let mut resp = axum::response::Response::builder()
                .status(StatusCode::OK)
                .body(axum::body::Body::from_stream(chunk_stream))
                .expect("valid streaming snapshot response");
            resp.headers_mut().insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            if let Ok(val) = HeaderValue::from_str(&disposition) {
                resp.headers_mut().insert(CONTENT_DISPOSITION, val);
            }
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
    cookies: Cookies,
    Query(query): Query<ScrubQuery>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_auth(&state, &cookies) {
        return resp;
    }

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

async fn handle_list_peers(State(state): State<Arc<AppState>>, cookies: Cookies) -> Response {
    if let Err(resp) = require_auth(&state, &cookies) {
        return resp;
    }
    let peers = match state.db.list_trusted_peers() {
        Ok(p) => p,
        Err(err) => return map_domain_error(err),
    };
    let online_set: HashSet<String> = state.online_peers.iter().map(|r| r.clone()).collect();
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

    let pairing_qr_uri = format!(
        "lanvault://pair?id={}&ip={}&port={}&pin={}",
        state.device_id, state.lan_ip, state.port, state.pairing_pin
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "peers": peer_dtos,
            "pairing_pin": state.pairing_pin,
            "pairing_qr_uri": pairing_qr_uri,
        })),
    )
        .into_response()
}

async fn handle_pair_peer(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    cookies: Cookies,
    Json(req): Json<PairPinRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }

    let pin_matched = constant_time_eq(req.pin.as_bytes(), state.pairing_pin.as_bytes());
    match state.auth.check_and_record_pair_attempt(addr.ip(), pin_matched) {
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

    let server_kex = SessionKeyExchange::new();
    let server_x25519_pub = server_kex.public_key_bytes();
    let session_key = match server_kex.agree(&client_x25519_pubkey, b"lanvault.pair.v1") {
        Ok(key) => key,
        Err(err) => return map_domain_error(err),
    };

    let peer_fp = public_key_fingerprint(&client_ed25519_pubkey);
    let peer_device_id = format!("lv-{}", &peer_fp[..12]);
    let peer = TrustedPeer {
        device_id: peer_device_id.clone(),
        device_name: req.device_name,
        os_type: req.os_type,
        ed25519_pubkey: client_ed25519_pubkey.to_vec(),
        last_ip: Some(addr.ip().to_string()),
        last_port: Some(addr.port()),
        auto_connect: true,
        paired_at: unix_timestamp_secs(),
    };

    if let Err(err) = state.db.upsert_trusted_peer(&peer) {
        return map_domain_error(err);
    }

    state
        .peer_session_keys
        .insert(peer_device_id.clone(), session_key);
    state.online_peers.insert(peer_device_id);
    let session_token = state.auth.issue_session();
    cookies.add(build_session_cookie(session_token.clone()));

    (
        StatusCode::OK,
        Json(PairPinResponse {
            device_id: state.device_id.clone(),
            server_x25519_pubkey_hex: hex::encode(server_x25519_pub),
            session_cookie: Some(session_token),
        }),
    )
        .into_response()
}

async fn handle_delete_peer(
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
    Path(device_id): Path<String>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_auth(&state, &cookies) {
        return resp;
    }
    match state.db.delete_trusted_peer(&device_id) {
        Ok(true) => {
            state.online_peers.remove(&device_id);
            state.peer_session_keys.remove(&device_id);
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

async fn handle_clipboard_push(
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
    Json(req): Json<ClipboardRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_auth(&state, &cookies) {
        return resp;
    }
    if req.content.is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "clipboard content must not be empty".to_string(),
        ));
    }

    let timestamp = unix_timestamp_secs();
    let delivered_to = match &req.target_device_id {
        Some(target_id) => usize::from(state.online_peers.contains(target_id)),
        None => state.online_peers.len(),
    };

    let _ = state.ws_tx.send(WsServerEvent::ClipboardReceived {
        from_device_id: state.device_id.clone(),
        target_device_id: req.target_device_id,
        kind: req.kind,
        content: req.content,
        filename: req.filename,
        timestamp,
    });

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "delivered_to": delivered_to,
            "timestamp": timestamp,
        })),
    )
        .into_response()
}

// =============================================================================
// WebSocket Handler with Ping/Pong & Bounded Backpressure Disconnect
// =============================================================================

async fn handle_ws_upgrade(
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
    Query(query): Query<WsConnectQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Err(resp) = require_auth(&state, &cookies) {
        return resp;
    }
    let connected_device_id = query.device_id.filter(|id| !id.trim().is_empty());
    ws.on_upgrade(move |socket| serve_ws_connection(socket, state, connected_device_id))
}

async fn serve_ws_connection(
    socket: WebSocket,
    state: Arc<AppState>,
    connected_device_id: Option<String>,
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
                        if let WsServerEvent::ClipboardReceived {
                            target_device_id: Some(ref target_id),
                            ..
                        } = ev
                        {
                            if connected_device_id.as_deref() != Some(target_id.as_str()) {
                                continue;
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

    drop(out_tx);
    if tokio::time::timeout(Duration::from_secs(1), &mut writer_task)
        .await
        .is_err()
    {
        writer_task.abort();
    }
}

