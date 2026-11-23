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
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
) -> Response {
    match require_auth(&state, &cookies) {
        Ok(session) => {
            if let Err(resp) = require_admin(&session) {
                return resp;
            }
        }
        Err(resp) => return resp,
    }
    Json(serde_json::json!({
        "debug_enabled": lanvault_core::is_debug_enabled(),
    }))
    .into_response()
}

pub(super) async fn handle_set_debug_config(
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
    Json(req): Json<SetDebugConfigRequest>,
) -> Response {
    match require_auth(&state, &cookies) {
        Ok(session) => {
            if let Err(resp) = require_admin(&session) {
                return resp;
            }
        }
        Err(resp) => return resp,
    }
    lanvault_core::set_debug_mode(req.enabled);
    Json(serde_json::json!({
        "debug_enabled": lanvault_core::is_debug_enabled(),
    }))
    .into_response()
}

pub(super) async fn handle_get_debug_logs(
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
    Query(query): Query<DebugLogsQuery>,
) -> Response {
    match require_auth(&state, &cookies) {
        Ok(session) => {
            if let Err(resp) = require_admin(&session) {
                return resp;
            }
        }
        Err(resp) => return resp,
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
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
) -> Response {
    match require_auth(&state, &cookies) {
        Ok(session) => {
            if let Err(resp) = require_admin(&session) {
                return resp;
            }
        }
        Err(resp) => return resp,
    }
    lanvault_core::clear_debug_logs();
    Json(serde_json::json!({ "cleared": true })).into_response()
}

pub(super) async fn handle_report_client_log(
    State(state): State<Arc<AppState>>,
    cookies: Cookies,
    Json(req): Json<ClientLogRequest>,
) -> Response {
    if let Err(resp) = require_auth(&state, &cookies) {
        return resp;
    }
    lanvault_core::record_frontend_log(&req.level, &req.source, req.message, req.detail);
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "recorded": true }))).into_response()
}
