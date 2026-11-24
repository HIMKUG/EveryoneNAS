//! v2 Device Groups, Sync Rules, Rule Target Bindings, Mirror Roots, and Capabilities routes.

use super::*;
use lanvault_core::{
    detect_rule_topology_conflicts, DeviceGroupRecord, GroupSyncRuleRecord, MirrorRootRegistration,
    ProtocolCapabilities, RuleTargetBindingRecord, RuleTopologySpec, SyncRuleMode,
};

fn default_archive_retention_days() -> u32 {
    30
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateGroupRequest {
    name: String,
    coordinator_device_id: Option<String>,
    #[serde(default)]
    member_device_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdateGroupRequest {
    name: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct AddGroupMemberRequest {
    device_id: String,
    role: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateGroupRuleRequest {
    name: String,
    mode: SyncRuleMode,
    source_device_id: String,
    source_root: String,
    source_root_id: Option<String>,
    #[serde(default = "default_archive_retention_days")]
    archive_retention_days: u32,
    #[serde(default = "default_true")]
    wifi_only: bool,
    allowed_ssids: Option<String>,
    #[serde(default)]
    require_charging: bool,
    #[serde(default)]
    max_kbps: u32,
    #[serde(default)]
    schedule_interval_secs: u64,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    needs_binding_confirm: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdateGroupRuleRequest {
    name: Option<String>,
    mode: Option<SyncRuleMode>,
    source_device_id: Option<String>,
    source_root: Option<String>,
    source_root_id: Option<Option<String>>,
    archive_retention_days: Option<u32>,
    wifi_only: Option<bool>,
    allowed_ssids: Option<Option<String>>,
    require_charging: Option<bool>,
    max_kbps: Option<u32>,
    schedule_interval_secs: Option<u64>,
    enabled: Option<bool>,
    needs_binding_confirm: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateRuleBindingRequest {
    target_device_id: String,
    target_vault_id: Option<String>,
    mirror_root_id: Option<String>,
    target_root: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    needs_binding_confirm: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct MirrorRootsQuery {
    device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RegisterMirrorRootRequest {
    mirror_root_id: Option<String>,
    device_id: Option<String>,
    canonical_root: String,
    root_identity: Option<String>,
    #[serde(default)]
    label: String,
}

#[allow(clippy::result_large_err)]
pub(super) fn require_group_access(
    db: &Database,
    session: &SessionInfo,
    group_id: &str,
) -> Result<DeviceGroupRecord, Response> {
    let group = match db.get_device_group(group_id) {
        Ok(Some(group)) => group,
        Ok(None) => {
            return Err(map_domain_error(LanVaultError::NotFound(format!(
                "device_group `{group_id}`"
            ))))
        }
        Err(err) => return Err(map_domain_error(err)),
    };
    if session.is_admin {
        return Ok(group);
    }
    let Some(device_id) = session.device_id.as_deref() else {
        return Err(unauthorized_response());
    };
    match db.is_active_group_member(group_id, device_id) {
        Ok(true) => Ok(group),
        Ok(false) => Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "group_membership_required" })),
        )
            .into_response()),
        Err(err) => Err(map_domain_error(err)),
    }
}

/// Validates rule topology across all active rules/bindings with an optional candidate rule
/// or binding applied. Returns `Err(409 Conflict)` if any overlap, mutual mirror, or cycle exists.
#[allow(clippy::result_large_err)]
fn validate_rule_topology(
    db: &Database,
    candidate_rule: Option<&GroupSyncRuleRecord>,
    candidate_binding: Option<&RuleTargetBindingRecord>,
) -> Result<(), Response> {
    let existing_rules = match db.list_group_sync_rules(None) {
        Ok(rules) => rules,
        Err(err) => return Err(map_domain_error(err)),
    };

    let mut rules: Vec<GroupSyncRuleRecord> = existing_rules
        .into_iter()
        .filter(|r| candidate_rule.map_or(true, |cand| cand.rule_id != r.rule_id))
        .collect();
    if let Some(cand) = candidate_rule {
        rules.push(cand.clone());
    }

    let mut specs = Vec::with_capacity(rules.len());
    for rule in &rules {
        if !rule.enabled {
            continue;
        }
        let mut bindings = match db.list_rule_target_bindings(&rule.rule_id) {
            Ok(b) => b,
            Err(err) => return Err(map_domain_error(err)),
        };
        if let Some(cand_b) = candidate_binding {
            if cand_b.rule_id == rule.rule_id {
                bindings.retain(|b| b.binding_id != cand_b.binding_id);
                bindings.push(cand_b.clone());
            }
        }
        let targets: Vec<(String, String)> = bindings
            .into_iter()
            .filter(|b| b.enabled && b.revoked_at.is_none())
            .filter_map(|b| {
                let root = b.target_root.filter(|r| !r.trim().is_empty())?;
                Some((b.target_device_id, root))
            })
            .collect();
        specs.push(RuleTopologySpec {
            rule_id: rule.rule_id.clone(),
            mode: rule.mode,
            source_device_id: rule.source_device_id.clone(),
            source_root: rule.source_root.clone(),
            targets,
        });
    }

    let conflicts = detect_rule_topology_conflicts(&specs);
    if !conflicts.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "rule_topology_conflict",
                "conflicts": conflicts,
            })),
        )
            .into_response());
    }
    Ok(())
}

pub(super) async fn handle_get_capabilities(
    Extension(_session): Extension<SessionInfo>,
) -> Response {
    (StatusCode::OK, Json(ProtocolCapabilities::current())).into_response()
}

pub(super) async fn handle_list_groups(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
) -> Response {
    let groups = match state.db.list_device_groups() {
        Ok(groups) => groups,
        Err(err) => return map_domain_error(err),
    };
    let visible = if session.is_admin {
        groups
    } else {
        let Some(device_id) = session.device_id.as_deref() else {
            return unauthorized_response();
        };
        let mut filtered = Vec::new();
        for group in groups {
            match state.db.is_active_group_member(&group.group_id, device_id) {
                Ok(true) => filtered.push(group),
                Ok(false) => {}
                Err(err) => return map_domain_error(err),
            }
        }
        filtered
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({ "groups": visible })),
    )
        .into_response()
}

pub(super) async fn handle_create_group(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Json(req): Json<CreateGroupRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    let name = req.name.trim();
    if name.is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "group name must not be empty".to_string(),
        ));
    }
    let coordinator_device_id = req
        .coordinator_device_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| state.device_id.clone());

    let now = unix_timestamp_secs();
    let group_id = uuid::Uuid::new_v4().to_string();
    let record = DeviceGroupRecord {
        group_id: group_id.clone(),
        name: name.to_string(),
        coordinator_device_id: coordinator_device_id.clone(),
        revision: 1,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };

    if let Err(err) = state.db.create_device_group(&record) {
        return map_domain_error(err);
    }
    if let Err(err) = state
        .db
        .add_group_member(&group_id, &coordinator_device_id, "coordinator")
    {
        return map_domain_error(err);
    }
    for member_id in req.member_device_ids {
        let trimmed = member_id.trim();
        if trimmed.is_empty() || trimmed == coordinator_device_id {
            continue;
        }
        if let Err(err) = state.db.add_group_member(&group_id, trimmed, "member") {
            return map_domain_error(err);
        }
    }

    match state.db.get_device_group(&group_id) {
        Ok(Some(saved)) => (StatusCode::CREATED, Json(saved)).into_response(),
        Ok(None) => map_domain_error(LanVaultError::NotFound(format!(
            "device_group `{group_id}`"
        ))),
        Err(err) => map_domain_error(err),
    }
}

pub(super) async fn handle_update_group(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(group_id): Path<String>,
    Json(req): Json<UpdateGroupRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    let new_name = req.name.trim();
    if new_name.is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "group name must not be empty".to_string(),
        ));
    }
    match state.db.rename_device_group(&group_id, new_name) {
        Ok(true) => match state.db.get_device_group(&group_id) {
            Ok(Some(updated)) => (StatusCode::OK, Json(updated)).into_response(),
            Ok(None) => map_domain_error(LanVaultError::NotFound(format!(
                "device_group `{group_id}`"
            ))),
            Err(err) => map_domain_error(err),
        },
        Ok(false) => map_domain_error(LanVaultError::NotFound(format!(
            "device_group `{group_id}`"
        ))),
        Err(err) => map_domain_error(err),
    }
}

pub(super) async fn handle_delete_group(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(group_id): Path<String>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    match state.db.delete_device_group(&group_id) {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "deleted": true,
                "group_id": group_id,
            })),
        )
            .into_response(),
        Ok(false) => map_domain_error(LanVaultError::NotFound(format!(
            "device_group `{group_id}`"
        ))),
        Err(err) => map_domain_error(err),
    }
}

pub(super) async fn handle_list_group_members(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(group_id): Path<String>,
) -> Response {
    if let Err(resp) = require_group_access(&state.db, &session, &group_id) {
        return resp;
    }
    match state.db.list_group_members(&group_id) {
        Ok(members) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "group_id": group_id,
                "members": members,
            })),
        )
            .into_response(),
        Err(err) => map_domain_error(err),
    }
}

pub(super) async fn handle_add_group_member(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(group_id): Path<String>,
    Json(req): Json<AddGroupMemberRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    let device_id = req.device_id.trim();
    if device_id.is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "device_id must not be empty".to_string(),
        ));
    }
    let role = req
        .role
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or("member");
    if role != "member" && role != "coordinator" {
        return map_domain_error(LanVaultError::InvalidInput(format!(
            "unsupported group member role `{role}`: expected `member` or `coordinator`"
        )));
    }
    if let Err(err) = state.db.add_group_member(&group_id, device_id, role) {
        return map_domain_error(err);
    }
    let group_revision = match state.db.get_device_group(&group_id) {
        Ok(Some(group)) => group.revision,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "device_group `{group_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "group_id": group_id,
            "device_id": device_id,
            "role": role,
            "group_revision": group_revision,
        })),
    )
        .into_response()
}

pub(super) async fn handle_remove_group_member(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path((group_id, device_id)): Path<(String, String)>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    match state.db.remove_group_member(&group_id, &device_id) {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "removed": true,
                "group_id": group_id,
                "device_id": device_id,
            })),
        )
            .into_response(),
        Ok(false) => map_domain_error(LanVaultError::NotFound(format!(
            "group member `{device_id}` in `{group_id}`"
        ))),
        Err(err) => map_domain_error(err),
    }
}

pub(super) async fn handle_list_group_rules(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(group_id): Path<String>,
) -> Response {
    if let Err(resp) = require_group_access(&state.db, &session, &group_id) {
        return resp;
    }
    match state.db.list_group_sync_rules(Some(&group_id)) {
        Ok(rules) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "group_id": group_id,
                "rules": rules,
            })),
        )
            .into_response(),
        Err(err) => map_domain_error(err),
    }
}

pub(super) async fn handle_create_group_rule(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(group_id): Path<String>,
    Json(req): Json<CreateGroupRuleRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    let group = match state.db.get_device_group(&group_id) {
        Ok(Some(group)) => group,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "device_group `{group_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };
    if req.source_device_id.trim().is_empty() || req.source_root.trim().is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "source_device_id and source_root must not be empty".to_string(),
        ));
    }

    let now = unix_timestamp_secs();
    let rule = GroupSyncRuleRecord {
        rule_id: uuid::Uuid::new_v4().to_string(),
        group_id,
        name: req.name.trim().to_string(),
        mode: req.mode,
        source_device_id: req.source_device_id.trim().to_string(),
        source_root: req.source_root.trim().to_string(),
        source_root_id: req.source_root_id,
        archive_retention_days: req.archive_retention_days,
        wifi_only: req.wifi_only,
        allowed_ssids: req.allowed_ssids,
        require_charging: req.require_charging,
        max_kbps: req.max_kbps,
        schedule_interval_secs: req.schedule_interval_secs,
        enabled: req.enabled,
        revision: 1,
        group_revision: group.revision,
        needs_binding_confirm: req.needs_binding_confirm,
        created_at: now,
        updated_at: now,
    };

    if let Err(resp) = validate_rule_topology(&state.db, Some(&rule), None) {
        return resp;
    }
    if let Err(err) = state.db.upsert_group_sync_rule(&rule) {
        return map_domain_error(err);
    }
    (StatusCode::CREATED, Json(rule)).into_response()
}

pub(super) async fn handle_update_group_rule(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(rule_id): Path<String>,
    Json(req): Json<UpdateGroupRuleRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    let existing = match state.db.get_group_sync_rule(&rule_id) {
        Ok(Some(rule)) => rule,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "group_sync_rule `{rule_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };
    let group_revision = match state.db.get_device_group(&existing.group_id) {
        Ok(Some(group)) => group.revision,
        Ok(None) => existing.group_revision,
        Err(err) => return map_domain_error(err),
    };

    let mut updated = existing;
    if let Some(name) = req.name {
        updated.name = name.trim().to_string();
    }
    if let Some(mode) = req.mode {
        updated.mode = mode;
    }
    if let Some(source_device_id) = req.source_device_id {
        if source_device_id.trim().is_empty() {
            return map_domain_error(LanVaultError::InvalidInput(
                "source_device_id must not be empty".to_string(),
            ));
        }
        updated.source_device_id = source_device_id.trim().to_string();
    }
    if let Some(source_root) = req.source_root {
        if source_root.trim().is_empty() {
            return map_domain_error(LanVaultError::InvalidInput(
                "source_root must not be empty".to_string(),
            ));
        }
        updated.source_root = source_root.trim().to_string();
    }
    if let Some(source_root_id) = req.source_root_id {
        updated.source_root_id = source_root_id;
    }
    if let Some(days) = req.archive_retention_days {
        updated.archive_retention_days = days;
    }
    if let Some(wifi_only) = req.wifi_only {
        updated.wifi_only = wifi_only;
    }
    if let Some(allowed_ssids) = req.allowed_ssids {
        updated.allowed_ssids = allowed_ssids;
    }
    if let Some(require_charging) = req.require_charging {
        updated.require_charging = require_charging;
    }
    if let Some(max_kbps) = req.max_kbps {
        updated.max_kbps = max_kbps;
    }
    if let Some(interval) = req.schedule_interval_secs {
        updated.schedule_interval_secs = interval;
    }
    if let Some(enabled) = req.enabled {
        updated.enabled = enabled;
    }
    if let Some(needs_binding_confirm) = req.needs_binding_confirm {
        updated.needs_binding_confirm = needs_binding_confirm;
    }
    updated.group_revision = group_revision;
    updated.updated_at = unix_timestamp_secs();

    if let Err(resp) = validate_rule_topology(&state.db, Some(&updated), None) {
        return resp;
    }
    if let Err(err) = state.db.upsert_group_sync_rule(&updated) {
        return map_domain_error(err);
    }
    match state.db.get_group_sync_rule(&rule_id) {
        Ok(Some(saved)) => (StatusCode::OK, Json(saved)).into_response(),
        Ok(None) => map_domain_error(LanVaultError::NotFound(format!(
            "group_sync_rule `{rule_id}`"
        ))),
        Err(err) => map_domain_error(err),
    }
}

pub(super) async fn handle_delete_group_rule(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(rule_id): Path<String>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    match state.db.delete_group_sync_rule(&rule_id) {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "deleted": true,
                "rule_id": rule_id,
            })),
        )
            .into_response(),
        Ok(false) => map_domain_error(LanVaultError::NotFound(format!(
            "group_sync_rule `{rule_id}`"
        ))),
        Err(err) => map_domain_error(err),
    }
}

pub(super) async fn handle_list_rule_bindings(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(rule_id): Path<String>,
) -> Response {
    let rule = match state.db.get_group_sync_rule(&rule_id) {
        Ok(Some(rule)) => rule,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "group_sync_rule `{rule_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };
    if let Err(resp) = require_group_access(&state.db, &session, &rule.group_id) {
        return resp;
    }
    match state.db.list_rule_target_bindings(&rule_id) {
        Ok(bindings) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "rule_id": rule_id,
                "bindings": bindings,
            })),
        )
            .into_response(),
        Err(err) => map_domain_error(err),
    }
}

pub(super) async fn handle_create_rule_binding(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(rule_id): Path<String>,
    Json(req): Json<CreateRuleBindingRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    let rule = match state.db.get_group_sync_rule(&rule_id) {
        Ok(Some(rule)) => rule,
        Ok(None) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "group_sync_rule `{rule_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };
    let target_device_id = req.target_device_id.trim();
    if target_device_id.is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "target_device_id must not be empty".to_string(),
        ));
    }

    let mut effective_target_root = req
        .target_root
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let mirror_root_id = req
        .mirror_root_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    if rule.mode == SyncRuleMode::MirrorSync || mirror_root_id.is_some() {
        let Some(ref root_id) = mirror_root_id else {
            return map_domain_error(LanVaultError::InvalidInput(
                "mirror_sync bindings require a registered mirror_root_id".to_string(),
            ));
        };
        let reg = match state.db.get_mirror_root(root_id) {
            Ok(Some(reg)) => reg,
            Ok(None) => {
                return map_domain_error(LanVaultError::InvalidInput(format!(
                    "mirror_root_id `{root_id}` is not registered"
                )));
            }
            Err(err) => return map_domain_error(err),
        };
        if reg.revoked_at.is_some() {
            return map_domain_error(LanVaultError::InvalidInput(format!(
                "mirror_root_id `{root_id}` has been revoked"
            )));
        }
        if reg.device_id != target_device_id {
            return map_domain_error(LanVaultError::InvalidInput(format!(
                "mirror_root_id `{root_id}` belongs to device `{}`, not `{target_device_id}`",
                reg.device_id
            )));
        }
        if effective_target_root.is_none() {
            effective_target_root = Some(reg.canonical_root);
        }
    }

    let now = unix_timestamp_secs();
    let binding = RuleTargetBindingRecord {
        binding_id: uuid::Uuid::new_v4().to_string(),
        rule_id: rule.rule_id.clone(),
        group_id: rule.group_id.clone(),
        target_device_id: target_device_id.to_string(),
        target_vault_id: req.target_vault_id,
        mirror_root_id,
        target_root: effective_target_root,
        enabled: req.enabled,
        revoked_at: None,
        applied_revision: 0,
        last_source_gen: 0,
        status: if req.needs_binding_confirm {
            "needs_binding_confirm".to_string()
        } else {
            "pending".to_string()
        },
        last_error: None,
        needs_binding_confirm: req.needs_binding_confirm,
        created_at: now,
        updated_at: now,
    };

    if let Err(resp) = validate_rule_topology(&state.db, None, Some(&binding)) {
        return resp;
    }
    if let Err(err) = state.db.upsert_rule_target_binding(&binding) {
        return map_domain_error(err);
    }
    (StatusCode::CREATED, Json(binding)).into_response()
}

pub(super) async fn handle_delete_rule_binding(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path((rule_id, binding_id)): Path<(String, String)>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    let existing = match state.db.get_rule_target_binding(&binding_id) {
        Ok(Some(b)) if b.rule_id == rule_id => b,
        Ok(_) => {
            return map_domain_error(LanVaultError::NotFound(format!(
                "rule_target_binding `{binding_id}` for rule `{rule_id}`"
            )))
        }
        Err(err) => return map_domain_error(err),
    };
    match state.db.disable_rule_target_binding(&existing.binding_id) {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "disabled": true,
                "rule_id": rule_id,
                "binding_id": binding_id,
            })),
        )
            .into_response(),
        Ok(false) => map_domain_error(LanVaultError::NotFound(format!(
            "rule_target_binding `{binding_id}`"
        ))),
        Err(err) => map_domain_error(err),
    }
}

pub(super) async fn handle_list_mirror_roots(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Query(query): Query<MirrorRootsQuery>,
) -> Response {
    let filter_device_id = if session.is_admin {
        query.device_id
    } else {
        let Some(dev_id) = session.device_id.clone() else {
            return unauthorized_response();
        };
        if let Some(requested) = query.device_id.as_deref() {
            if requested != dev_id {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({ "error": "peer_identity_mismatch" })),
                )
                    .into_response();
            }
        }
        Some(dev_id)
    };
    match state.db.list_mirror_roots(filter_device_id.as_deref()) {
        Ok(roots) => (
            StatusCode::OK,
            Json(serde_json::json!({ "mirror_roots": roots })),
        )
            .into_response(),
        Err(err) => map_domain_error(err),
    }
}

pub(super) async fn handle_register_mirror_root(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Json(req): Json<RegisterMirrorRootRequest>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    let canonical_root = req.canonical_root.trim();
    if canonical_root.is_empty() {
        return map_domain_error(LanVaultError::InvalidInput(
            "canonical_root must not be empty".to_string(),
        ));
    }
    let mirror_root_id = req
        .mirror_root_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let device_id = req
        .device_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| state.device_id.clone());

    let reg = MirrorRootRegistration {
        mirror_root_id,
        device_id,
        canonical_root: canonical_root.to_string(),
        root_identity: req.root_identity,
        label: req.label.trim().to_string(),
        created_at: unix_timestamp_secs(),
        revoked_at: None,
    };
    if let Err(err) = state.db.register_mirror_root(&reg) {
        return map_domain_error(err);
    }
    (StatusCode::CREATED, Json(reg)).into_response()
}

pub(super) async fn handle_revoke_mirror_root(
    State(state): State<Arc<AppState>>,
    Extension(session): Extension<SessionInfo>,
    Path(root_id): Path<String>,
) -> Response {
    if let Err(resp) = require_accepting_tasks(&state) {
        return resp;
    }
    if let Err(resp) = require_admin(&session) {
        return resp;
    }
    match state.db.revoke_mirror_root(&root_id) {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "revoked": true,
                "mirror_root_id": root_id,
            })),
        )
            .into_response(),
        Ok(false) => map_domain_error(LanVaultError::NotFound(format!("mirror_root `{root_id}`"))),
        Err(err) => map_domain_error(err),
    }
}
