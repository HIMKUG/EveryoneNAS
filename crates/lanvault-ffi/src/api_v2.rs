//! LanVault v2 FFI layer:
//! - Multi-device groups & membership
//! - Group sync rules (diff backup & mirror sync), target bindings, and mirror root registrations
//! - Incremental catalog browsing, historical version listing, and safe verified restore
//! - Non-blocking persistent multi-target message outbox

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use lanvault_client::mirror_engine::{execute_mirror_sync, preview_mirror_sync, MirrorApplyReport};
use lanvault_client::outbox::{
    enqueue_file_message, enqueue_text_message, retry_failed_outbox_message, FrozenOutboxTarget,
};
use lanvault_client::scanner::scan_source_generation;
use lanvault_client::sync_engine::{
    compute_two_pointer_merge, BindingSyncReport, MergeAction, SyncEngine,
};
use lanvault_core::{
    blake3_hash_reader, detect_rule_topology_conflicts, strip_windows_verbatim, unpack_chunk,
    CatalogEntryRecord, CatalogVersionRecord, Database, DeviceGroupMemberRecord, DeviceGroupRecord,
    GroupSyncRuleRecord, MessageOutboxRecord, MessageOutboxTargetRecord, MirrorPreviewReport,
    MirrorRootRegistration, RoleMode, RuleTargetBindingRecord, RuleTopologySpec, SymmetricKey,
    SyncRuleMode, CHUNK_MAX_SIZE,
};
use lanvault_server::vault::VaultStore;
use lanvault_server::web::validate_relative_snapshot_path;

use crate::api::{
    client_data_dir, fetch_remote_default_vault_id, resolve_local_server_db_path, runtime_state,
};

fn default_true() -> bool {
    true
}

fn default_archive_retention_days() -> u32 {
    30
}

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn new_v2_id(prefix: &str) -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    format!("{prefix}-{}", hex::encode(bytes))
}

#[cfg(unix)]
fn extract_root_identity(meta: &fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("unix:{}:{}", meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn extract_root_identity(meta: &fs::Metadata) -> Option<String> {
    use std::os::windows::fs::MetadataExt;
    Some(format!("win:{}", meta.creation_time()))
}

#[cfg(not(any(unix, windows)))]
fn extract_root_identity(_meta: &fs::Metadata) -> Option<String> {
    None
}

fn normalize_root_key(raw: &str) -> String {
    strip_windows_verbatim(raw)
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

// -----------------------------------------------------------------------------
// v2 FFI DTOs
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceGroupView {
    pub group: DeviceGroupRecord,
    pub members: Vec<DeviceGroupMemberRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRuleView {
    pub rule: GroupSyncRuleRecord,
    pub bindings: Vec<RuleTargetBindingRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertGroupRuleInput {
    #[serde(default)]
    pub rule_id: Option<String>,
    pub group_id: String,
    pub name: String,
    pub mode: SyncRuleMode,
    #[serde(default)]
    pub source_device_id: Option<String>,
    pub source_root: String,
    #[serde(default)]
    pub source_root_id: Option<String>,
    #[serde(default = "default_archive_retention_days")]
    pub archive_retention_days: u32,
    #[serde(default = "default_true")]
    pub wifi_only: bool,
    #[serde(default)]
    pub allowed_ssids: Option<String>,
    #[serde(default)]
    pub require_charging: bool,
    #[serde(default)]
    pub max_kbps: u32,
    #[serde(default)]
    pub schedule_interval_secs: u64,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertBindingInput {
    #[serde(default)]
    pub binding_id: Option<String>,
    pub rule_id: String,
    pub target_device_id: String,
    #[serde(default)]
    pub target_vault_id: Option<String>,
    #[serde(default)]
    pub mirror_root_id: Option<String>,
    #[serde(default)]
    pub target_root: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingSyncOutcome {
    pub binding_id: String,
    pub target_device_id: String,
    pub status: String,
    pub error: Option<String>,
    pub diff_report: Option<BindingSyncReport>,
    pub mirror_report: Option<MirrorApplyReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRuleSyncOutcome {
    pub rule_id: String,
    pub source_complete: bool,
    pub scanned_entries: usize,
    pub blocked_reasons: Vec<String>,
    pub volatile_paths: Vec<String>,
    pub bindings: Vec<BindingSyncOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreCatalogVersionResult {
    pub binding_id: String,
    pub version_id: String,
    pub path_key: String,
    pub restored_bytes: u64,
    pub output_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageOutboxView {
    pub message: MessageOutboxRecord,
    pub targets: Vec<MessageOutboxTargetRecord>,
}

// -----------------------------------------------------------------------------
// Topology Validation Helper
// -----------------------------------------------------------------------------

fn validate_topology_with_candidate(
    db: &Database,
    candidate_rule: &GroupSyncRuleRecord,
    candidate_binding: Option<&RuleTargetBindingRecord>,
) -> Result<()> {
    let existing_rules = db.list_group_sync_rules(None)?;
    let mut specs: Vec<RuleTopologySpec> = Vec::with_capacity(existing_rules.len() + 1);
    let mut replaced = false;

    for rule in existing_rules {
        let is_candidate = rule.rule_id == candidate_rule.rule_id;
        let effective_rule = if is_candidate {
            replaced = true;
            candidate_rule.clone()
        } else {
            rule
        };
        if !effective_rule.enabled {
            continue;
        }
        let mut bindings = db.list_rule_target_bindings(&effective_rule.rule_id)?;
        if is_candidate {
            if let Some(cb) = candidate_binding {
                if let Some(slot) = bindings.iter_mut().find(|b| b.binding_id == cb.binding_id) {
                    *slot = cb.clone();
                } else {
                    bindings.push(cb.clone());
                }
            }
        }
        let targets = bindings
            .into_iter()
            .filter(|b| b.enabled && b.revoked_at.is_none())
            .filter_map(|b| b.target_root.map(|root| (b.target_device_id, root)))
            .collect();
        specs.push(RuleTopologySpec {
            rule_id: effective_rule.rule_id,
            mode: effective_rule.mode,
            source_device_id: effective_rule.source_device_id,
            source_root: effective_rule.source_root,
            targets,
        });
    }

    if !replaced && candidate_rule.enabled {
        let mut targets = Vec::new();
        if let Some(cb) = candidate_binding {
            if cb.enabled && cb.revoked_at.is_none() {
                if let Some(ref root) = cb.target_root {
                    targets.push((cb.target_device_id.clone(), root.clone()));
                }
            }
        }
        specs.push(RuleTopologySpec {
            rule_id: candidate_rule.rule_id.clone(),
            mode: candidate_rule.mode,
            source_device_id: candidate_rule.source_device_id.clone(),
            source_root: candidate_rule.source_root.clone(),
            targets,
        });
    }

    let conflicts = detect_rule_topology_conflicts(&specs);
    if !conflicts.is_empty() {
        bail!("sync rule topology conflict: {}", conflicts.join("; "));
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// Device Groups
// -----------------------------------------------------------------------------

pub async fn list_device_groups() -> Result<Vec<DeviceGroupView>> {
    let state = runtime_state()?;
    let groups = state.client_db.list_device_groups()?;
    let mut views = Vec::with_capacity(groups.len());
    for group in groups {
        let members = state.client_db.list_group_members(&group.group_id)?;
        views.push(DeviceGroupView { group, members });
    }
    Ok(views)
}

pub async fn create_device_group(name: String) -> Result<DeviceGroupRecord> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("device group name must not be empty");
    }
    let state = runtime_state()?;
    let now = unix_now_secs();
    let group_id = new_v2_id("grp");
    let record = DeviceGroupRecord {
        group_id: group_id.clone(),
        name: trimmed.to_string(),
        coordinator_device_id: state.client_device_id.clone(),
        revision: 1,
        created_at: now,
        updated_at: now,
        deleted_at: None,
    };
    state.client_db.create_device_group(&record)?;
    state
        .client_db
        .add_group_member(&group_id, &state.client_device_id, "coordinator")?;
    state
        .client_db
        .get_device_group(&group_id)?
        .ok_or_else(|| anyhow!("device group `{group_id}` disappeared after creation"))
}

pub async fn rename_device_group(group_id: String, name: String) -> Result<DeviceGroupRecord> {
    let gid = group_id.trim();
    let trimmed = name.trim();
    if gid.is_empty() || trimmed.is_empty() {
        bail!("group_id and name must not be empty");
    }
    let state = runtime_state()?;
    if !state.client_db.rename_device_group(gid, trimmed)? {
        bail!("device group `{gid}` not found");
    }
    state
        .client_db
        .get_device_group(gid)?
        .ok_or_else(|| anyhow!("device group `{gid}` not found"))
}

pub async fn delete_device_group(group_id: String) -> Result<()> {
    let gid = group_id.trim();
    if gid.is_empty() {
        bail!("group_id must not be empty");
    }
    let state = runtime_state()?;
    if !state.client_db.delete_device_group(gid)? {
        bail!("device group `{gid}` not found");
    }
    Ok(())
}

pub async fn add_group_member(
    group_id: String,
    device_id: String,
    role: Option<String>,
) -> Result<()> {
    let gid = group_id.trim();
    let did = device_id.trim();
    if gid.is_empty() || did.is_empty() {
        bail!("group_id and device_id must not be empty");
    }
    let role_str = role
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("member");
    let state = runtime_state()?;
    state.client_db.add_group_member(gid, did, role_str)?;
    Ok(())
}

pub async fn remove_group_member(group_id: String, device_id: String) -> Result<()> {
    let gid = group_id.trim();
    let did = device_id.trim();
    if gid.is_empty() || did.is_empty() {
        bail!("group_id and device_id must not be empty");
    }
    let state = runtime_state()?;
    if !state.client_db.remove_group_member(gid, did)? {
        bail!("member `{did}` not found in device group `{gid}`");
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// Group Rules, Bindings & Mirror Roots
// -----------------------------------------------------------------------------

pub async fn list_group_rules(group_id: Option<String>) -> Result<Vec<GroupRuleView>> {
    let state = runtime_state()?;
    let gid_filter = group_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let rules = state.client_db.list_group_sync_rules(gid_filter)?;
    let mut views = Vec::with_capacity(rules.len());
    for rule in rules {
        let bindings = state.client_db.list_rule_target_bindings(&rule.rule_id)?;
        views.push(GroupRuleView { rule, bindings });
    }
    Ok(views)
}

pub async fn upsert_group_rule(input: UpsertGroupRuleInput) -> Result<GroupSyncRuleRecord> {
    let group_id = input.group_id.trim().to_string();
    let source_root = strip_windows_verbatim(&input.source_root);
    if group_id.is_empty() {
        bail!("group_id must not be empty");
    }
    if source_root.is_empty() {
        bail!("source_root must not be empty");
    }

    let state = runtime_state()?;
    let group = state
        .client_db
        .get_device_group(&group_id)?
        .ok_or_else(|| anyhow!("device group `{group_id}` not found"))?;

    let source_device_id = input
        .source_device_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| state.client_device_id.clone());

    if !state
        .client_db
        .is_active_group_member(&group_id, &source_device_id)?
    {
        bail!("source device `{source_device_id}` is not an active member of group `{group_id}`");
    }

    let rule_id = input
        .rule_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| new_v2_id("grule"));
    let existing = state.client_db.get_group_sync_rule(&rule_id)?;
    let now = unix_now_secs();
    let name = if input.name.trim().is_empty() {
        source_root.clone()
    } else {
        input.name.trim().to_string()
    };

    let candidate = GroupSyncRuleRecord {
        rule_id: rule_id.clone(),
        group_id,
        name,
        mode: input.mode,
        source_device_id,
        source_root,
        source_root_id: input.source_root_id,
        archive_retention_days: input.archive_retention_days.max(1),
        wifi_only: input.wifi_only,
        allowed_ssids: input.allowed_ssids,
        require_charging: input.require_charging,
        max_kbps: input.max_kbps,
        schedule_interval_secs: input.schedule_interval_secs,
        enabled: input.enabled,
        revision: existing.as_ref().map(|r| r.revision + 1).unwrap_or(1),
        group_revision: group.revision,
        needs_binding_confirm: false,
        created_at: existing.as_ref().map(|r| r.created_at).unwrap_or(now),
        updated_at: now,
    };

    validate_topology_with_candidate(&state.client_db, &candidate, None)?;
    state.client_db.upsert_group_sync_rule(&candidate)?;
    state
        .client_db
        .get_group_sync_rule(&rule_id)?
        .ok_or_else(|| anyhow!("group sync rule `{rule_id}` disappeared after save"))
}

pub async fn delete_group_rule(rule_id: String) -> Result<()> {
    let rid = rule_id.trim();
    if rid.is_empty() {
        bail!("rule_id must not be empty");
    }
    let state = runtime_state()?;
    if !state.client_db.delete_group_sync_rule(rid)? {
        bail!("group sync rule `{rid}` not found");
    }
    Ok(())
}

pub async fn toggle_group_rule(rule_id: String, enabled: bool) -> Result<()> {
    let rid = rule_id.trim();
    if rid.is_empty() {
        bail!("rule_id must not be empty");
    }
    let state = runtime_state()?;
    if !state.client_db.toggle_group_sync_rule(rid, enabled)? {
        bail!("group sync rule `{rid}` not found");
    }
    Ok(())
}

pub async fn upsert_rule_target_binding(
    input: UpsertBindingInput,
) -> Result<RuleTargetBindingRecord> {
    let rule_id = input.rule_id.trim().to_string();
    let target_device_id = input.target_device_id.trim().to_string();
    if rule_id.is_empty() || target_device_id.is_empty() {
        bail!("rule_id and target_device_id must not be empty");
    }

    let state = runtime_state()?;
    let rule = state
        .client_db
        .get_group_sync_rule(&rule_id)?
        .ok_or_else(|| anyhow!("group sync rule `{rule_id}` not found"))?;

    if !state
        .client_db
        .is_active_group_member(&rule.group_id, &target_device_id)?
    {
        bail!(
            "target device `{target_device_id}` is not an active member of group `{}`",
            rule.group_id
        );
    }

    let mut target_root = input
        .target_root
        .as_deref()
        .map(strip_windows_verbatim)
        .filter(|s| !s.is_empty());
    let mirror_root_id = input
        .mirror_root_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    if rule.mode == SyncRuleMode::MirrorSync {
        let mrid = mirror_root_id
            .as_deref()
            .ok_or_else(|| anyhow!("mirror_sync binding requires a registered mirror_root_id"))?;
        let reg = state
            .client_db
            .get_mirror_root(mrid)?
            .ok_or_else(|| anyhow!("mirror root `{mrid}` is not registered"))?;
        if reg.revoked_at.is_some() {
            bail!("mirror root `{mrid}` has been revoked");
        }
        if reg.device_id != target_device_id {
            bail!(
                "mirror root `{mrid}` belongs to device `{}`, not `{target_device_id}`",
                reg.device_id
            );
        }
        if let Some(ref supplied_root) = target_root {
            if normalize_root_key(supplied_root) != normalize_root_key(&reg.canonical_root) {
                bail!(
                    "target_root `{supplied_root}` does not match registered mirror root `{}`",
                    reg.canonical_root
                );
            }
        } else {
            target_root = Some(reg.canonical_root);
        }
    }

    let binding_id = input
        .binding_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| new_v2_id("bind"));
    let existing = state.client_db.get_rule_target_binding(&binding_id)?;
    let now = unix_now_secs();

    let candidate = RuleTargetBindingRecord {
        binding_id: binding_id.clone(),
        rule_id: rule.rule_id.clone(),
        group_id: rule.group_id.clone(),
        target_device_id,
        target_vault_id: input
            .target_vault_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned),
        mirror_root_id,
        target_root,
        enabled: input.enabled,
        revoked_at: None,
        applied_revision: existing.as_ref().map(|b| b.applied_revision).unwrap_or(0),
        last_source_gen: existing.as_ref().map(|b| b.last_source_gen).unwrap_or(0),
        status: existing
            .as_ref()
            .map(|b| b.status.clone())
            .unwrap_or_else(|| "pending".to_string()),
        last_error: None,
        needs_binding_confirm: false,
        created_at: existing.as_ref().map(|b| b.created_at).unwrap_or(now),
        updated_at: now,
    };

    validate_topology_with_candidate(&state.client_db, &rule, Some(&candidate))?;
    state.client_db.upsert_rule_target_binding(&candidate)?;
    state
        .client_db
        .get_rule_target_binding(&binding_id)?
        .ok_or_else(|| anyhow!("rule target binding `{binding_id}` disappeared after save"))
}

pub async fn disable_rule_target_binding(binding_id: String) -> Result<()> {
    let bid = binding_id.trim();
    if bid.is_empty() {
        bail!("binding_id must not be empty");
    }
    let state = runtime_state()?;
    if !state.client_db.disable_rule_target_binding(bid)? {
        bail!("rule target binding `{bid}` not found");
    }
    Ok(())
}

pub async fn register_mirror_root(
    canonical_path: String,
    display_name: String,
) -> Result<MirrorRootRegistration> {
    let raw = canonical_path.trim();
    if raw.is_empty() {
        bail!("canonical_path must not be empty");
    }
    let path = PathBuf::from(raw);
    let sym_meta = fs::symlink_metadata(&path)
        .with_context(|| format!("cannot stat mirror root `{}`", path.display()))?;
    if sym_meta.file_type().is_symlink() {
        bail!("mirror root `{}` must not be a symlink", path.display());
    }
    if !sym_meta.is_dir() {
        bail!(
            "mirror root `{}` must be an existing directory",
            path.display()
        );
    }
    let canonical = fs::canonicalize(&path)
        .with_context(|| format!("failed to canonicalize mirror root `{}`", path.display()))?;
    let canonical_root = strip_windows_verbatim(&canonical.to_string_lossy());
    let root_identity = extract_root_identity(&sym_meta);
    let label = if display_name.trim().is_empty() {
        canonical
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Mirror Root")
            .to_string()
    } else {
        display_name.trim().to_string()
    };

    let state = runtime_state()?;
    let reg = MirrorRootRegistration {
        mirror_root_id: new_v2_id("mroot"),
        device_id: state.client_device_id.clone(),
        canonical_root,
        root_identity,
        label,
        created_at: unix_now_secs(),
        revoked_at: None,
    };
    state.client_db.register_mirror_root(&reg)?;
    Ok(reg)
}

pub async fn list_mirror_roots() -> Result<Vec<MirrorRootRegistration>> {
    let state = runtime_state()?;
    Ok(state.client_db.list_mirror_roots(None)?)
}

pub async fn revoke_mirror_root(root_id: String) -> Result<()> {
    let rid = root_id.trim();
    if rid.is_empty() {
        bail!("root_id must not be empty");
    }
    let state = runtime_state()?;
    if !state.client_db.revoke_mirror_root(rid)? {
        bail!("mirror root `{rid}` not found or already revoked");
    }
    Ok(())
}

pub async fn preview_group_rule(rule_id: String) -> Result<Vec<MirrorPreviewReport>> {
    let rid = rule_id.trim().to_string();
    if rid.is_empty() {
        bail!("rule_id must not be empty");
    }
    let state = runtime_state()?;
    let rule = state
        .client_db
        .get_group_sync_rule(&rid)?
        .ok_or_else(|| anyhow!("group sync rule `{rid}` not found"))?;
    let bindings: Vec<RuleTargetBindingRecord> = state
        .client_db
        .list_rule_target_bindings(&rid)?
        .into_iter()
        .filter(|b| b.enabled && b.revoked_at.is_none())
        .collect();

    let source_root = PathBuf::from(&rule.source_root);
    let generation = tokio::task::spawn_blocking(move || scan_source_generation(&source_root, &[]))
        .await
        .context("preview source scan worker panicked")??;

    let mut reports = Vec::with_capacity(bindings.len());
    for binding in bindings {
        if rule.mode == SyncRuleMode::MirrorSync
            && binding.target_device_id == state.client_device_id
        {
            if let Some(ref target_root_str) = binding.target_root {
                let gen_clone = generation.clone();
                let target_root = PathBuf::from(target_root_str);
                let r_id = rule.rule_id.clone();
                let b_id = binding.binding_id.clone();
                let t_dev = binding.target_device_id.clone();
                let report = tokio::task::spawn_blocking(move || {
                    preview_mirror_sync(&gen_clone, &target_root, &r_id, &b_id, &t_dev)
                })
                .await
                .context("preview_mirror_sync worker panicked")??;
                reports.push(report);
                continue;
            }
        }

        let catalog_entries =
            state
                .client_db
                .list_catalog_entries_page(&binding.binding_id, None, 100_000, false)?;
        let plan = compute_two_pointer_merge(
            &generation.entries,
            &catalog_entries,
            rule.mode,
            generation.complete,
        );
        let mut add_count = 0usize;
        let mut overwrite_count = 0usize;
        let mut remove_count = 0usize;
        for action in &plan.actions {
            match action {
                MergeAction::Add(_) => add_count += 1,
                MergeAction::Update { .. } | MergeAction::ClearSourceDeleted { .. } => {
                    overwrite_count += 1
                }
                MergeAction::MarkSourceDeleted { .. } | MergeAction::ArchiveAndRemove { .. } => {
                    remove_count += 1
                }
            }
        }
        let mut conflicts: Vec<String> = generation
            .collisions
            .iter()
            .map(|c| format!("source collision on `{}`: {}", c.path, c.detail))
            .collect();
        conflicts.extend(
            generation
                .blocked_reasons
                .iter()
                .map(|r| format!("source blocked: {r}")),
        );
        let blocked_count = plan.blocked_deletions + conflicts.len();
        reports.push(MirrorPreviewReport {
            rule_id: rule.rule_id.clone(),
            binding_id: binding.binding_id,
            target_device_id: binding.target_device_id,
            add_count,
            overwrite_count,
            remove_count,
            unchanged_count: plan.unchanged_count,
            blocked_count,
            conflicts,
        });
    }

    Ok(reports)
}

pub async fn trigger_group_rule_sync(rule_id: String) -> Result<GroupRuleSyncOutcome> {
    let rid = rule_id.trim().to_string();
    if rid.is_empty() {
        bail!("rule_id must not be empty");
    }
    let state = runtime_state()?;
    if state.role_mode().await == RoleMode::ServerOnly {
        bail!(
            "cannot trigger group rule sync in `server_only` mode; switch role_mode to `client_only` or `dual` first"
        );
    }

    {
        let mut in_flight = state.sync_inflight.lock().await;
        if !in_flight.insert(rid.clone()) {
            bail!("group sync rule `{rid}` is already running");
        }
    }

    let result = trigger_group_rule_sync_inner(&state, &rid).await;
    state.sync_inflight.lock().await.remove(&rid);
    result
}

async fn trigger_group_rule_sync_inner(
    state: &Arc<crate::api::FfiRuntimeState>,
    rule_id: &str,
) -> Result<GroupRuleSyncOutcome> {
    let rule = state
        .client_db
        .get_group_sync_rule(rule_id)?
        .filter(|r| r.enabled)
        .ok_or_else(|| anyhow!("group sync rule `{rule_id}` not found or disabled"))?;

    let source_root = PathBuf::from(&rule.source_root);
    let generation = tokio::task::spawn_blocking(move || scan_source_generation(&source_root, &[]))
        .await
        .context("source generation scan worker panicked")??;

    let bindings: Vec<RuleTargetBindingRecord> = state
        .client_db
        .list_rule_target_bindings(rule_id)?
        .into_iter()
        .filter(|b| b.enabled)
        .collect();

    let policy = state.policy.read().await.clone();
    let environment = state.environment.read().await.clone();
    let vault_master_key = state.vault_master_key.read().await.clone();
    let mut outcomes = Vec::with_capacity(bindings.len());

    for binding in bindings {
        if binding.revoked_at.is_some()
            || !state
                .client_db
                .is_active_group_member(&rule.group_id, &binding.target_device_id)?
        {
            let msg = format!(
                "target device `{}` is revoked or no longer a member of group `{}`",
                binding.target_device_id, rule.group_id
            );
            state
                .client_db
                .update_binding_status(&binding.binding_id, "blocked", Some(&msg))?;
            outcomes.push(BindingSyncOutcome {
                binding_id: binding.binding_id,
                target_device_id: binding.target_device_id,
                status: "blocked".to_string(),
                error: Some(msg),
                diff_report: None,
                mirror_report: None,
            });
            continue;
        }

        if binding.needs_binding_confirm {
            outcomes.push(BindingSyncOutcome {
                binding_id: binding.binding_id,
                target_device_id: binding.target_device_id,
                status: "needs_confirm".to_string(),
                error: Some("binding requires explicit user confirmation".to_string()),
                diff_report: None,
                mirror_report: None,
            });
            continue;
        }

        // Local mirror sync path (same device target with registered mirror root).
        if rule.mode == SyncRuleMode::MirrorSync
            && binding.target_device_id == state.client_device_id
        {
            let mrid = match binding.mirror_root_id.as_deref() {
                Some(id) => id,
                None => {
                    let msg = "local mirror binding is missing mirror_root_id".to_string();
                    state.client_db.update_binding_status(
                        &binding.binding_id,
                        "blocked",
                        Some(&msg),
                    )?;
                    outcomes.push(BindingSyncOutcome {
                        binding_id: binding.binding_id,
                        target_device_id: binding.target_device_id,
                        status: "blocked".to_string(),
                        error: Some(msg),
                        diff_report: None,
                        mirror_report: None,
                    });
                    continue;
                }
            };
            let reg = state.client_db.get_mirror_root(mrid)?;
            let Some(reg) = reg.filter(|r| r.revoked_at.is_none()) else {
                let msg = format!("mirror root `{mrid}` is revoked or missing");
                state.client_db.update_binding_status(
                    &binding.binding_id,
                    "blocked",
                    Some(&msg),
                )?;
                outcomes.push(BindingSyncOutcome {
                    binding_id: binding.binding_id,
                    target_device_id: binding.target_device_id,
                    status: "blocked".to_string(),
                    error: Some(msg),
                    diff_report: None,
                    mirror_report: None,
                });
                continue;
            };

            let db_bg = Arc::clone(&state.client_db);
            let gen_bg = generation.clone();
            let target_root = PathBuf::from(&reg.canonical_root);
            let archive_root = client_data_dir().join("mirror_archives");
            let rule_bg = rule.clone();
            let binding_bg = binding.clone();
            let op_id = new_v2_id("mop");

            let mirror_res = tokio::task::spawn_blocking(move || {
                execute_mirror_sync(
                    &db_bg,
                    &gen_bg,
                    &target_root,
                    &archive_root,
                    &rule_bg,
                    &binding_bg,
                    &op_id,
                    None,
                )
            })
            .await
            .context("execute_mirror_sync worker panicked")?;

            match mirror_res {
                Ok(report) => {
                    let mut updated = binding.clone();
                    updated.applied_revision += 1;
                    updated.last_source_gen += 1;
                    updated.status = "synced".to_string();
                    updated.last_error = None;
                    updated.updated_at = unix_now_secs();
                    state.client_db.upsert_rule_target_binding(&updated)?;
                    outcomes.push(BindingSyncOutcome {
                        binding_id: binding.binding_id,
                        target_device_id: binding.target_device_id,
                        status: "synced".to_string(),
                        error: None,
                        diff_report: None,
                        mirror_report: Some(report),
                    });
                }
                Err(err) => {
                    let msg = err.to_string();
                    state.client_db.update_binding_status(
                        &binding.binding_id,
                        "error",
                        Some(&msg),
                    )?;
                    outcomes.push(BindingSyncOutcome {
                        binding_id: binding.binding_id,
                        target_device_id: binding.target_device_id,
                        status: "error".to_string(),
                        error: Some(msg),
                        diff_report: None,
                        mirror_report: None,
                    });
                }
            }
            continue;
        }

        if rule.mode == SyncRuleMode::MirrorSync {
            let msg = "remote mirror execution is unavailable until the target device exposes a fenced mirror-agent endpoint"
                .to_string();
            state
                .client_db
                .update_binding_status(&binding.binding_id, "blocked", Some(&msg))?;
            outcomes.push(BindingSyncOutcome {
                binding_id: binding.binding_id,
                target_device_id: binding.target_device_id,
                status: "blocked".to_string(),
                error: Some(msg),
                diff_report: None,
                mirror_report: None,
            });
            continue;
        }

        // Remote target path: strictly resolve from per-peer `peer_sessions` (never global `active_peer`).
        let peer_opt = state
            .peer_sessions
            .read()
            .await
            .get(&binding.target_device_id)
            .cloned();
        let Some(peer) = peer_opt else {
            let msg = format!(
                "target device `{}` has no active session (offline)",
                binding.target_device_id
            );
            state.client_db.update_binding_status(
                &binding.binding_id,
                "offline_pending",
                Some(&msg),
            )?;
            outcomes.push(BindingSyncOutcome {
                binding_id: binding.binding_id,
                target_device_id: binding.target_device_id,
                status: "offline_pending".to_string(),
                error: Some(msg),
                diff_report: None,
                mirror_report: None,
            });
            continue;
        };

        let mut effective_binding = binding.clone();
        if effective_binding.target_vault_id.is_none() {
            effective_binding.target_vault_id = match peer.default_vault_id.clone() {
                Some(vid) => Some(vid),
                None => {
                    fetch_remote_default_vault_id(
                        &state.http_client,
                        &peer.endpoint_base_url,
                        peer.session_cookie.as_deref(),
                    )
                    .await
                }
            };
        }

        let engine = SyncEngine::new(
            peer.endpoint_base_url.clone(),
            peer.session_cookie.clone(),
            state.client_device_id.clone(),
            vault_master_key.clone(),
            policy.clone(),
            environment.clone(),
            Some(Arc::clone(&state.rate_limiter)),
            peer.fingerprint,
        )?;

        match engine
            .sync_binding_v2(
                &rule,
                &effective_binding,
                &generation,
                &state.client_device_id,
            )
            .await
        {
            Ok(report) => {
                let mut updated = effective_binding.clone();
                if let Some(new_rev) = report.new_revision {
                    updated.applied_revision = new_rev;
                    updated.last_source_gen += 1;
                }
                updated.status = "synced".to_string();
                updated.last_error = None;
                updated.updated_at = unix_now_secs();
                state.client_db.upsert_rule_target_binding(&updated)?;
                outcomes.push(BindingSyncOutcome {
                    binding_id: binding.binding_id,
                    target_device_id: binding.target_device_id,
                    status: "synced".to_string(),
                    error: None,
                    diff_report: Some(report),
                    mirror_report: None,
                });
            }
            Err(lanvault_client::ClientError::Http(ref http_err))
                if http_err.is_connect() || http_err.is_timeout() =>
            {
                let msg = format!("target peer unreachable: {http_err}");
                state.client_db.update_binding_status(
                    &binding.binding_id,
                    "offline_pending",
                    Some(&msg),
                )?;
                outcomes.push(BindingSyncOutcome {
                    binding_id: binding.binding_id,
                    target_device_id: binding.target_device_id,
                    status: "offline_pending".to_string(),
                    error: Some(msg),
                    diff_report: None,
                    mirror_report: None,
                });
            }
            Err(err) => {
                let msg = err.to_string();
                warn!(
                    rule_id = %rule.rule_id,
                    binding_id = %binding.binding_id,
                    target = %binding.target_device_id,
                    "v2 binding sync failed: {msg}"
                );
                state
                    .client_db
                    .update_binding_status(&binding.binding_id, "error", Some(&msg))?;
                outcomes.push(BindingSyncOutcome {
                    binding_id: binding.binding_id,
                    target_device_id: binding.target_device_id,
                    status: "error".to_string(),
                    error: Some(msg),
                    diff_report: None,
                    mirror_report: None,
                });
            }
        }
    }

    Ok(GroupRuleSyncOutcome {
        rule_id: rule.rule_id,
        source_complete: generation.complete,
        scanned_entries: generation.entries.len(),
        blocked_reasons: generation.blocked_reasons,
        volatile_paths: generation.volatile_paths,
        bindings: outcomes,
    })
}

// -----------------------------------------------------------------------------
// Catalog & Historical Versions
// -----------------------------------------------------------------------------

pub async fn list_binding_catalog(
    binding_id: String,
    after_path_key: Option<String>,
    limit: Option<usize>,
    source_deleted_only: bool,
) -> Result<Vec<CatalogEntryRecord>> {
    let bid = binding_id.trim().to_string();
    if bid.is_empty() {
        bail!("binding_id must not be empty");
    }
    let page_limit = limit.unwrap_or(200).clamp(1, 1000);
    let state = runtime_state()?;
    let entries = state.client_db.list_catalog_entries_page(
        &bid,
        after_path_key.as_deref(),
        page_limit,
        source_deleted_only,
    )?;
    if !entries.is_empty() {
        return Ok(entries);
    }
    if let Some((db_path, _, _)) = resolve_local_server_db_path(&state).await? {
        if db_path.exists() {
            return tokio::task::spawn_blocking(move || -> Result<Vec<CatalogEntryRecord>> {
                let server_db = Database::shared(&db_path)?;
                Ok(server_db.list_catalog_entries_page(
                    &bid,
                    after_path_key.as_deref(),
                    page_limit,
                    source_deleted_only,
                )?)
            })
            .await
            .context("list_binding_catalog worker panicked")?;
        }
    }
    Ok(entries)
}

pub async fn list_binding_versions(
    binding_id: String,
    path_key: Option<String>,
    after_archived_at: Option<i64>,
    after_version_id: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<CatalogVersionRecord>> {
    let bid = binding_id.trim().to_string();
    if bid.is_empty() {
        bail!("binding_id must not be empty");
    }
    let page_limit = limit.unwrap_or(200).clamp(1, 1000);
    let state = runtime_state()?;
    let versions = state.client_db.list_catalog_versions_page(
        &bid,
        path_key.as_deref(),
        after_archived_at,
        after_version_id.as_deref(),
        page_limit,
    )?;
    if !versions.is_empty() {
        return Ok(versions);
    }
    if let Some((db_path, _, _)) = resolve_local_server_db_path(&state).await? {
        if db_path.exists() {
            return tokio::task::spawn_blocking(move || -> Result<Vec<CatalogVersionRecord>> {
                let server_db = Database::shared(&db_path)?;
                Ok(server_db.list_catalog_versions_page(
                    &bid,
                    path_key.as_deref(),
                    after_archived_at,
                    after_version_id.as_deref(),
                    page_limit,
                )?)
            })
            .await
            .context("list_binding_versions worker panicked")?;
        }
    }
    Ok(versions)
}

pub async fn restore_catalog_version(
    binding_id: String,
    version_id: String,
    target_dir: String,
) -> Result<RestoreCatalogVersionResult> {
    let bid = binding_id.trim().to_string();
    let vid = version_id.trim().to_string();
    let target_trim = strip_windows_verbatim(&target_dir);
    if bid.is_empty() || vid.is_empty() || target_trim.is_empty() {
        bail!("binding_id, version_id, and target_dir must not be empty");
    }

    let state = runtime_state()?;
    // Refuse restoring into the rule's own source_root to guarantee we never overwrite source blindly.
    if let Some(binding) = state.client_db.get_rule_target_binding(&bid)? {
        if let Some(rule) = state.client_db.get_group_sync_rule(&binding.rule_id)? {
            let src_norm = normalize_root_key(&rule.source_root);
            let dst_norm = normalize_root_key(&target_trim);
            if dst_norm == src_norm || dst_norm.starts_with(&format!("{src_norm}/")) {
                bail!(
                    "refusing to restore version `{vid}` inside rule source directory `{}`",
                    rule.source_root
                );
            }
        }
    }

    let server_db_path = resolve_local_server_db_path(&state)
        .await?
        .map(|(p, _, _)| p)
        .filter(|p| p.exists());
    let vault_key = state.vault_master_key.read().await.clone();
    let client_db = Arc::clone(&state.client_db);
    let archive_root = client_data_dir().join("mirror_archives");

    let result = tokio::task::spawn_blocking(move || -> Result<RestoreCatalogVersionResult> {
        let (version, vault_db_opt) = match client_db.get_catalog_version(&vid)? {
            Some(v) => (
                v,
                server_db_path
                    .as_deref()
                    .map(Database::shared)
                    .transpose()?,
            ),
            None => {
                let db_path =
                    server_db_path.ok_or_else(|| anyhow!("catalog version `{vid}` not found"))?;
                let server_db = Database::shared(&db_path)?;
                let v = server_db
                    .get_catalog_version(&vid)?
                    .ok_or_else(|| anyhow!("catalog version `{vid}` not found"))?;
                (v, Some(server_db))
            }
        };

        if version.binding_id != bid {
            bail!("catalog version `{vid}` does not belong to binding `{bid}`");
        }
        if version.recycled_at.is_some() {
            bail!("catalog version `{vid}` has already been recycled");
        }

        let out_base = PathBuf::from(&target_trim);
        if out_base.exists() && fs::symlink_metadata(&out_base)?.file_type().is_symlink() {
            bail!("restore target_dir `{target_trim}` must not be a symlink");
        }
        let rel_path = validate_relative_snapshot_path(&version.display_path)?;
        let restore_root = out_base.join(format!("restore_{}", version.version_id));
        let dest_path = restore_root.join(rel_path);
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let archive_file = archive_root.join(&bid).join(&vid);
        let restored_bytes = if archive_file.is_file() {
            let actual_hash = {
                let f = File::open(&archive_file)?;
                let mut reader = BufReader::with_capacity(256 * 1024, f);
                blake3_hash_reader(&mut reader)?
            };
            if let Some(ref expected) = version.content_hash {
                if actual_hash.as_slice() != expected.as_slice() {
                    bail!("mirror archive integrity check failed for version `{vid}`");
                }
            }
            fs::copy(&archive_file, &dest_path)?
        } else {
            let server_db = vault_db_opt.ok_or_else(|| {
                anyhow!("local server database is required to restore chunk-backed version `{vid}`")
            })?;
            let vault_store = VaultStore::new(server_db);
            let chunk_key = vault_key.derive_vault_chunk_key(&version.vault_id);
            let chunk_hexes: Vec<String> = serde_json::from_str(&version.chunk_hashes_json)
                .context("invalid chunk_hashes_json on catalog version")?;
            let mut dest_file = File::create(&dest_path).with_context(|| {
                format!("failed to create restored file {}", dest_path.display())
            })?;
            let mut written = 0u64;
            for h_hex in &chunk_hexes {
                let h_bytes = hex::decode(h_hex)
                    .with_context(|| format!("invalid chunk hash hex `{h_hex}`"))?;
                let hash: [u8; 32] = h_bytes
                    .try_into()
                    .map_err(|_| anyhow!("invalid chunk hash length"))?;
                let stored = vault_store.get_chunk(&version.vault_id, &hash)?;
                let plain = unpack_chunk(
                    &stored,
                    &hash,
                    Some(&chunk_key),
                    version.vault_id.as_bytes(),
                    CHUNK_MAX_SIZE as usize,
                )?;
                std::io::Write::write_all(&mut dest_file, &plain)?;
                written += plain.len() as u64;
            }
            std::io::Write::flush(&mut dest_file)?;
            drop(dest_file);
            if let Some(ref expected) = version.content_hash {
                let actual_hash = {
                    let f = File::open(&dest_path)?;
                    let mut reader = BufReader::with_capacity(256 * 1024, f);
                    blake3_hash_reader(&mut reader)?
                };
                if actual_hash.as_slice() != expected.as_slice() {
                    let _ = fs::remove_file(&dest_path);
                    bail!("restored file BLAKE3 verification failed for version `{vid}`");
                }
            }
            written
        };

        Ok(RestoreCatalogVersionResult {
            binding_id: bid,
            version_id: vid,
            path_key: version.path_key,
            restored_bytes,
            output_path: dest_path.to_string_lossy().into_owned(),
        })
    })
    .await
    .context("restore_catalog_version worker panicked")??;

    info!(
        binding_id = %result.binding_id,
        version_id = %result.version_id,
        bytes = result.restored_bytes,
        output = %result.output_path,
        "restored catalog version"
    );
    Ok(result)
}

// -----------------------------------------------------------------------------
// Non-Blocking Message Outbox
// -----------------------------------------------------------------------------

async fn freeze_outbox_targets(
    state: &Arc<crate::api::FfiRuntimeState>,
    target_device_ids: &[String],
) -> Result<(String, Vec<FrozenOutboxTarget>)> {
    let mut unique_ids = Vec::with_capacity(target_device_ids.len());
    let mut seen = HashSet::with_capacity(target_device_ids.len());
    for raw in target_device_ids {
        let id = raw.trim();
        if id.is_empty() {
            continue;
        }
        if seen.insert(id.to_string()) {
            unique_ids.push(id.to_string());
        }
    }
    if unique_ids.is_empty() {
        bail!("target_device_ids must contain at least one device_id");
    }

    let sessions = state.peer_sessions.read().await.clone();
    let mut frozen = Vec::with_capacity(unique_ids.len());
    for did in &unique_ids {
        let trusted = state.client_db.get_trusted_peer(did)?;
        if let Some(session) = sessions.get(did) {
            let device_name = trusted
                .as_ref()
                .map(|p| p.device_name.clone())
                .unwrap_or_else(|| did.clone());
            frozen.push(FrozenOutboxTarget {
                device_id: did.clone(),
                device_name,
                endpoint_base_url: session.endpoint_base_url.clone(),
                session_cookie: session.session_cookie.clone(),
                channel_key: session.channel_key.clone(),
            });
        } else if let Some(peer) = trusted.filter(|p| p.revoked_at.is_none()) {
            // Paired peer currently offline: freeze target so background outbox retries once session resumes.
            frozen.push(FrozenOutboxTarget {
                device_id: did.clone(),
                device_name: peer.device_name,
                endpoint_base_url: String::new(),
                session_cookie: None,
                channel_key: SymmetricKey::from_bytes([0u8; 32]),
            });
        } else {
            bail!("target device `{did}` is not a paired peer");
        }
    }

    let mut key_parts = unique_ids;
    key_parts.sort();
    let conversation_key = key_parts.join(",");
    Ok((conversation_key, frozen))
}

pub async fn enqueue_message_text(
    target_device_ids: Vec<String>,
    text: String,
) -> Result<MessageOutboxView> {
    if text.is_empty() {
        bail!("text message must not be empty");
    }
    let state = runtime_state()?;
    let (conversation_key, frozen_targets) =
        freeze_outbox_targets(&state, &target_device_ids).await?;
    let record = enqueue_text_message(&state.client_db, &conversation_key, &frozen_targets, &text)?;
    state.outbox_notify.notify_one();
    let targets = state.client_db.list_outbox_targets(&record.client_msg_id)?;
    Ok(MessageOutboxView {
        message: record,
        targets,
    })
}

pub async fn enqueue_message_file(
    target_device_ids: Vec<String>,
    file_path: String,
) -> Result<MessageOutboxView> {
    let trimmed = file_path.trim().to_string();
    if trimmed.is_empty() {
        bail!("file_path must not be empty");
    }
    let state = runtime_state()?;
    let (conversation_key, frozen_targets) =
        freeze_outbox_targets(&state, &target_device_ids).await?;
    let db = Arc::clone(&state.client_db);
    let staging_dir = client_data_dir().join("outbox_staging");
    let record = tokio::task::spawn_blocking(move || {
        enqueue_file_message(
            &db,
            &staging_dir,
            &conversation_key,
            &frozen_targets,
            Path::new(&trimmed),
        )
    })
    .await
    .context("enqueue_message_file staging worker panicked")??;
    state.outbox_notify.notify_one();
    let targets = state.client_db.list_outbox_targets(&record.client_msg_id)?;
    Ok(MessageOutboxView {
        message: record,
        targets,
    })
}

pub async fn retry_outbox_message(client_msg_id: String) -> Result<usize> {
    let msg_id = client_msg_id.trim();
    if msg_id.is_empty() {
        bail!("client_msg_id must not be empty");
    }
    let state = runtime_state()?;
    let reset = retry_failed_outbox_message(&state.client_db, msg_id)?;
    if reset > 0 {
        state.outbox_notify.notify_one();
    }
    Ok(reset)
}

pub async fn list_outbox_messages(limit: Option<usize>) -> Result<Vec<MessageOutboxView>> {
    let state = runtime_state()?;
    let page_limit = limit.unwrap_or(100).clamp(1, 1000);
    let messages = state.client_db.list_outbox_messages(None, page_limit)?;
    let mut views = Vec::with_capacity(messages.len());
    for message in messages {
        let targets = state
            .client_db
            .list_outbox_targets(&message.client_msg_id)?;
        views.push(MessageOutboxView { message, targets });
    }
    Ok(views)
}
