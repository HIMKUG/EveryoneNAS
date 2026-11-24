use super::*;

#[derive(Debug, Deserialize)]
pub(super) struct SetDebugConfigRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct DebugLogsQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ClientLogRequest {
    level: String,
    source: String,
    message: String,
    detail: Option<String>,
}

pub(super) async fn handle_get_debug_config(
    Extension(session): Extension<SessionInfo>,
) -> Response {
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    Json(serde_json::json!({
        "debug_enabled": lanvault_core::is_debug_enabled(),
    }))
    .into_response()
}

pub(super) async fn handle_set_debug_config(
    Extension(session): Extension<SessionInfo>,
    Json(req): Json<SetDebugConfigRequest>,
) -> Response {
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    lanvault_core::set_debug_mode(req.enabled);
    Json(serde_json::json!({
        "debug_enabled": lanvault_core::is_debug_enabled(),
    }))
    .into_response()
}

pub(super) async fn handle_get_debug_logs(
    Extension(session): Extension<SessionInfo>,
    Query(query): Query<DebugLogsQuery>,
) -> Response {
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    let limit = query.limit.unwrap_or(200);
    let entries = lanvault_core::get_debug_logs(limit);
    Json(serde_json::json!({
        "debug_enabled": lanvault_core::is_debug_enabled(),
        "entries": entries,
    }))
    .into_response()
}

pub(super) async fn handle_clear_debug_logs(
    Extension(session): Extension<SessionInfo>,
) -> Response {
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    lanvault_core::clear_debug_logs();
    Json(serde_json::json!({ "cleared": true })).into_response()
}

pub(super) async fn handle_report_client_log(
    Extension(session): Extension<SessionInfo>,
    Json(req): Json<ClientLogRequest>,
) -> Response {
    if !session.is_admin && session.device_id.is_none() {
        return unauthorized_response();
    }
    if req.message.trim().is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "client log message must not be empty".to_string(),
        ));
    }
    let source = if let Some(dev_id) = session.device_id.as_deref() {
        format!("peer.{dev_id}.{}", req.source.trim())
    } else {
        req.source
    };
    lanvault_core::record_frontend_log(&req.level, &source, req.message, req.detail);
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "recorded": true })),
    )
        .into_response()
}
