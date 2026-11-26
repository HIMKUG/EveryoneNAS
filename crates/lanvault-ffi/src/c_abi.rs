//! C-ABI JSON 分发层与 Android JNI 符号导出。
//!
//! - C-ABI 符号：`lanvault_ffi_invoke_json`、`lanvault_ffi_free_string`，
//!   与 `flutter_app/lib/bridge/lanvault_bridge.dart` 100% 对齐并覆盖 `api.rs` 全部导出函数（含三态模式控制）。
//! - Android JNI 符号（`#[cfg(target_os = "android")]`）：
//!   与 `ShizukuUserService.kt`、`RootDaemonService.kt` 和 `KeepAliveService.kt` 的 `external fun` 声明精确匹配。

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api;

#[derive(Deserialize)]
struct PairWithPinPayload {
    ip: String,
    port: u16,
    pin: String,
}

#[derive(Deserialize)]
struct EnableAutoReconnectPayload {
    enabled: bool,
}

#[derive(Deserialize)]
struct StartLocalServerPayload {
    vault_path: String,
    quota_gb: u64,
    web_password: String,
    #[serde(default)]
    port: Option<u16>,
}

#[derive(Deserialize)]
struct TriggerIncrementalScanPayload {
    rule_id: String,
}

#[derive(Deserialize)]
struct UpdatePowerNetworkPolicyPayload {
    wifi_only: bool,
    allowed_ssids: Vec<String>,
    require_charging: bool,
    max_kbps: u32,
}

#[derive(Deserialize)]
struct UpdateDeviceEnvironmentPayload {
    on_wifi: bool,
    current_ssid: Option<String>,
    is_charging: bool,
}

#[derive(Deserialize)]
struct PushClipboardTextPayload {
    target_device_id: Option<String>,
    text: String,
}

#[derive(Deserialize)]
struct SetRoleModePayload {
    mode: String,
}

#[derive(Deserialize)]
struct UpsertSyncRulePayload {
    rule_id: String,
    local_dir: String,
    #[serde(default)]
    target_vault_id: String,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
struct DeleteSyncRulePayload {
    rule_id: String,
}

#[derive(Deserialize)]
struct KeyBackupPayload {
    passphrase: String,
}

#[derive(Deserialize)]
struct ImportKeyBackupPayload {
    passphrase: String,
    backup_json: String,
}

#[derive(Deserialize)]
struct RestoreSnapshotPayload {
    snapshot_id: String,
    target_dir: String,
}

#[derive(Deserialize, Default)]
struct ListTransferHistoryPayload {
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct SetBoolPayload {
    enabled: bool,
}

#[derive(Deserialize)]
struct SetSyncPausedPayload {
    paused: bool,
}

#[derive(Deserialize)]
struct RequestPairingPayload {
    ip: String,
    port: u16,
}

#[derive(Deserialize)]
struct RespondPairRequestPayload {
    request_id: String,
    accept: bool,
}

#[derive(Deserialize)]
struct SetSyncRuleSchedulePayload {
    rule_id: String,
    schedule_mode: String,
}

fn dispatch_json(fn_name: &str, payload_json: &str) -> Result<Value> {
    let _probe = lanvault_core::ProbeGuard::start(
        "ffi.c_abi",
        "ffi.invoke_json",
        Some(format!("fn={fn_name}")),
    );
    let res = dispatch_json_inner(fn_name, payload_json);
    if let Err(ref err) = res {
        lanvault_core::record_event_always(
            "ERROR",
            "rust.ffi",
            Some("ffi.error"),
            None,
            format!("FFI `{fn_name}` failed: {err:#}"),
            None,
        );
    }
    res
}

fn dispatch_json_inner(fn_name: &str, payload_json: &str) -> Result<Value> {
    match fn_name {
        "start_mdns_scan" => {
            let peers = api::poll_mdns_peers()?;
            Ok(json!({ "ok": true, "peers": peers }))
        }
        "pair_with_pin" => {
            let req: PairWithPinPayload =
                serde_json::from_str(payload_json).context("invalid pair_with_pin JSON payload")?;
            let device_id = api::block_on(api::pair_with_pin(req.ip, req.port, req.pin))??;
            Ok(json!({ "ok": true, "device_id": device_id }))
        }
        "request_pairing" => {
            let req: RequestPairingPayload = serde_json::from_str(payload_json)
                .context("invalid request_pairing JSON payload")?;
            let device_id = api::block_on(api::request_pairing(req.ip, req.port))??;
            Ok(json!({ "ok": true, "device_id": device_id }))
        }
        "list_pending_pair_requests" => {
            let requests = api::block_on(api::list_pending_pair_requests())??;
            Ok(json!({ "ok": true, "requests": requests }))
        }
        "respond_pair_request" => {
            let req: RespondPairRequestPayload = serde_json::from_str(payload_json)
                .context("invalid respond_pair_request JSON payload")?;
            let accepted = api::block_on(api::respond_pair_request(req.request_id, req.accept))??;
            Ok(json!({ "ok": true, "accepted": accepted }))
        }
        "scan_windows_networks" => {
            let scan = api::block_on(api::scan_windows_networks())??;
            let mut obj = serde_json::to_value(scan)?;
            if let Some(map) = obj.as_object_mut() {
                map.insert("ok".to_string(), json!(true));
            }
            Ok(obj)
        }
        "enable_auto_reconnect" => {
            let req: EnableAutoReconnectPayload = serde_json::from_str(payload_json)
                .context("invalid enable_auto_reconnect JSON payload")?;
            api::block_on(api::enable_auto_reconnect(req.enabled))??;
            Ok(json!({ "ok": true }))
        }
        "start_local_server" => {
            let req: StartLocalServerPayload = serde_json::from_str(payload_json)
                .context("invalid start_local_server JSON payload")?;
            let port = api::block_on(api::start_local_server_with_port(
                req.vault_path,
                req.quota_gb,
                req.web_password,
                req.port,
            ))??;
            Ok(json!({ "ok": true, "port": port }))
        }
        "prepare_graceful_restart" => {
            api::block_on(api::prepare_graceful_restart())??;
            Ok(json!({ "ok": true }))
        }
        "clear_web_password" => {
            let cleared = api::block_on(api::clear_web_password())??;
            Ok(json!({ "ok": true, "cleared": cleared }))
        }
        "subscribe_sync_progress" => {
            let ev = api::latest_sync_progress()?;
            Ok(json!({
                "ok": true,
                "event": {
                    "rule_id": ev.rule_id,
                    "file_path": ev.current_file,
                    "bytes_processed": ev.bytes_processed,
                    "total_bytes": ev.total_bytes,
                    "chunks_uploaded": ev.chunks_uploaded,
                    "chunks_deduplicated": ev.chunks_deduplicated,
                    "throughput_bps": ev.throughput_bps,
                    "status": ev.status,
                    "bit_rot_health": ev.bit_rot_health,
                    "corrupted_chunks": ev.corrupted_chunks,
                }
            }))
        }
        "trigger_incremental_scan" => {
            let req: TriggerIncrementalScanPayload = serde_json::from_str(payload_json)
                .context("invalid trigger_incremental_scan JSON payload")?;
            api::block_on(api::trigger_incremental_scan(req.rule_id))??;
            Ok(json!({ "ok": true }))
        }
        "update_power_network_policy" => {
            let req: UpdatePowerNetworkPolicyPayload = serde_json::from_str(payload_json)
                .context("invalid update_power_network_policy JSON payload")?;
            api::block_on(api::update_power_network_policy(
                req.wifi_only,
                req.allowed_ssids,
                req.require_charging,
                req.max_kbps,
            ))??;
            Ok(json!({ "ok": true }))
        }
        "update_device_environment" => {
            let req: UpdateDeviceEnvironmentPayload = serde_json::from_str(payload_json)
                .context("invalid update_device_environment JSON payload")?;
            api::block_on(api::update_device_environment(
                req.on_wifi,
                req.current_ssid,
                req.is_charging,
            ))??;
            Ok(json!({ "ok": true }))
        }
        "push_clipboard_text" => {
            let req: PushClipboardTextPayload = serde_json::from_str(payload_json)
                .context("invalid push_clipboard_text JSON payload")?;
            api::block_on(api::push_clipboard_text(req.target_device_id, req.text))??;
            Ok(json!({ "ok": true }))
        }
        "set_role_mode" => {
            let req: SetRoleModePayload =
                serde_json::from_str(payload_json).context("invalid set_role_mode JSON payload")?;
            let role_mode = api::block_on(api::set_role_mode(req.mode))??;
            Ok(json!({ "ok": true, "role_mode": role_mode }))
        }
        "get_role_mode" => {
            let role_mode = api::block_on(api::get_role_mode())??;
            Ok(json!({ "ok": true, "role_mode": role_mode }))
        }
        "list_sync_rules" => {
            let rules = api::block_on(api::list_sync_rules())??;
            Ok(json!({ "ok": true, "rules": rules }))
        }
        "upsert_sync_rule" => {
            let req: UpsertSyncRulePayload = serde_json::from_str(payload_json)
                .context("invalid upsert_sync_rule JSON payload")?;
            let rule = api::block_on(api::upsert_sync_rule(
                req.rule_id,
                req.local_dir,
                req.target_vault_id,
                req.enabled,
            ))??;
            Ok(json!({ "ok": true, "rule": rule }))
        }
        "delete_sync_rule" => {
            let req: DeleteSyncRulePayload = serde_json::from_str(payload_json)
                .context("invalid delete_sync_rule JSON payload")?;
            let deleted = api::block_on(api::delete_sync_rule(req.rule_id))??;
            Ok(json!({ "ok": true, "deleted": deleted }))
        }
        "export_vault_key_backup" => {
            let req: KeyBackupPayload = serde_json::from_str(payload_json)
                .context("invalid export_vault_key_backup JSON payload")?;
            let backup = api::block_on(api::export_vault_key_backup(req.passphrase))??;
            Ok(json!({ "ok": true, "backup": backup }))
        }
        "import_vault_key_backup" => {
            let req: ImportKeyBackupPayload = serde_json::from_str(payload_json)
                .context("invalid import_vault_key_backup JSON payload")?;
            api::block_on(api::import_vault_key_backup(
                req.passphrase,
                req.backup_json,
            ))??;
            Ok(json!({ "ok": true }))
        }
        "set_debug_mode" => {
            let req: SetDebugModePayload = serde_json::from_str(payload_json)
                .context("invalid set_debug_mode JSON payload")?;
            let enabled = api::block_on(api::set_debug_mode(req.enabled))??;
            Ok(json!({ "ok": true, "debug_enabled": enabled }))
        }
        "get_debug_mode" => {
            let enabled = api::block_on(api::get_debug_mode())??;
            Ok(json!({ "ok": true, "debug_enabled": enabled }))
        }
        "get_debug_logs" => {
            let req: GetDebugLogsPayload = serde_json::from_str(payload_json)
                .context("invalid get_debug_logs JSON payload")?;
            let entries = api::block_on(api::get_debug_logs(req.limit))??;
            Ok(
                json!({ "ok": true, "debug_enabled": lanvault_core::is_debug_enabled(), "entries": entries }),
            )
        }
        "clear_debug_logs" => {
            api::block_on(api::clear_debug_logs())??;
            Ok(json!({ "ok": true }))
        }
        "report_client_log" => {
            let req: ReportClientLogPayload = serde_json::from_str(payload_json)
                .context("invalid report_client_log JSON payload")?;
            api::block_on(api::report_client_log(
                req.level,
                req.source,
                req.message,
                req.detail,
            ))??;
            Ok(json!({ "ok": true }))
        }
        "list_paired_peers" => {
            let peers = api::block_on(api::list_paired_peers())??;
            Ok(json!({ "ok": true, "peers": peers }))
        }
        "get_power_network_policy" => {
            let policy = api::block_on(api::get_power_network_policy())??;
            Ok(json!({ "ok": true, "policy": policy }))
        }
        "get_auto_reconnect" => {
            let enabled = api::block_on(api::get_auto_reconnect())??;
            Ok(json!({ "ok": true, "enabled": enabled }))
        }
        "get_backup_storage_config" => {
            let config = api::block_on(api::get_backup_storage_config())??;
            Ok(json!({ "ok": true, "config": config }))
        }
        "set_backup_storage_config" => {
            let req: SetBackupStorageConfigPayload = serde_json::from_str(payload_json)
                .context("invalid set_backup_storage_config JSON payload")?;
            let config = api::block_on(api::set_backup_storage_config(
                req.primary_dir,
                req.fallback_dir,
            ))??;
            Ok(json!({ "ok": true, "config": config }))
        }
        "get_debug_log_path" => {
            let log_path = api::block_on(api::get_debug_log_path())??;
            Ok(json!({ "ok": true, "log_path": log_path }))
        }
        "get_last_server_config" => {
            let config = api::block_on(api::get_last_server_config())??;
            Ok(json!({ "ok": true, "config": config }))
        }
        "autostart_local_server" => {
            let port = api::block_on(api::autostart_local_server())??;
            Ok(json!({ "ok": true, "port": port }))
        }
        "get_autostart_server" => {
            let enabled = api::block_on(api::get_autostart_server())??;
            Ok(json!({ "ok": true, "enabled": enabled }))
        }
        "set_autostart_server" => {
            let req: SetBoolPayload = serde_json::from_str(payload_json)
                .context("invalid set_autostart_server JSON payload")?;
            let enabled = api::block_on(api::set_autostart_server(req.enabled))??;
            Ok(json!({ "ok": true, "enabled": enabled }))
        }
        "run_self_diagnostics" => {
            let report = api::block_on(api::run_self_diagnostics())??;
            Ok(json!({ "ok": true, "report": report }))
        }
        "get_system_overview" => {
            let overview = api::block_on(api::get_system_overview())??;
            let mut obj = serde_json::to_value(overview)?;
            if let Some(map) = obj.as_object_mut() {
                map.insert("ok".to_string(), json!(true));
            }
            Ok(obj)
        }
        "list_vaults" => {
            let vaults = api::block_on(api::list_vaults())??;
            Ok(json!({ "ok": true, "vaults": vaults }))
        }
        "create_vault" => {
            let req: CreateVaultPayload =
                serde_json::from_str(payload_json).context("invalid create_vault JSON payload")?;
            let vault = api::block_on(api::create_vault(
                req.name,
                req.mount_path,
                req.pool_mode,
                req.quota_gb,
                req.e2ee_enabled,
                req.extra_mount_path,
            ))??;
            let mut obj = serde_json::to_value(vault)?;
            if let Some(map) = obj.as_object_mut() {
                map.insert("ok".to_string(), json!(true));
            }
            Ok(obj)
        }
        "list_snapshots" => {
            let req: ListSnapshotsPayload = if payload_json.trim().is_empty() {
                ListSnapshotsPayload::default()
            } else {
                serde_json::from_str(payload_json).context("invalid list_snapshots JSON payload")?
            };
            let snapshots = api::block_on(api::list_snapshots(req.vault_id))??;
            Ok(json!({ "ok": true, "snapshots": snapshots }))
        }
        "trigger_bit_rot_scrub" => {
            let report = api::block_on(api::trigger_bit_rot_scrub())??;
            let mut obj = serde_json::to_value(report)?;
            if let Some(map) = obj.as_object_mut() {
                map.insert("ok".to_string(), json!(true));
            }
            Ok(obj)
        }
        "revoke_paired_peer" => {
            let req: RevokePairedPeerPayload = serde_json::from_str(payload_json)
                .context("invalid revoke_paired_peer JSON payload")?;
            let res = api::block_on(api::revoke_paired_peer(req.device_id))??;
            Ok(json!({ "ok": true, "revoked": res.revoked, "device_id": res.device_id }))
        }
        "get_pairing_info" => {
            let info = api::block_on(api::get_pairing_info())??;
            let mut obj = serde_json::to_value(info)?;
            if let Some(map) = obj.as_object_mut() {
                map.insert("ok".to_string(), json!(true));
            }
            Ok(obj)
        }
        "toggle_sync_rule" => {
            let req: ToggleSyncRulePayload = serde_json::from_str(payload_json)
                .context("invalid toggle_sync_rule JSON payload")?;
            let res = api::block_on(api::toggle_sync_rule(req.rule_id, req.enabled))??;
            Ok(json!({ "ok": true, "rule_id": res.rule_id, "enabled": res.enabled }))
        }
        "push_clipboard_file" => {
            let req: PushClipboardFilePayload = serde_json::from_str(payload_json)
                .context("invalid push_clipboard_file JSON payload")?;
            let res = api::block_on(api::push_clipboard_file(
                req.target_device_id,
                req.file_path,
            ))??;
            Ok(json!({ "ok": true, "sent": res.sent, "file_path": res.file_path }))
        }
        "restore_snapshot" => {
            let req: RestoreSnapshotPayload = serde_json::from_str(payload_json)
                .context("invalid restore_snapshot JSON payload")?;
            let res = api::block_on(api::restore_snapshot(req.snapshot_id, req.target_dir))??;
            let mut obj = serde_json::to_value(res)?;
            if let Some(map) = obj.as_object_mut() {
                map.insert("ok".to_string(), json!(true));
            }
            Ok(obj)
        }
        "list_transfer_history" => {
            let req: ListTransferHistoryPayload = if payload_json.trim().is_empty() {
                ListTransferHistoryPayload::default()
            } else {
                serde_json::from_str(payload_json)
                    .context("invalid list_transfer_history JSON payload")?
            };
            let records = api::block_on(api::list_transfer_history(req.limit))??;
            Ok(json!({ "ok": true, "records": records }))
        }
        "set_fast_backup_mode" => {
            #[derive(Deserialize)]
            struct FastModePayload {
                enabled: bool,
            }
            let req: FastModePayload = serde_json::from_str(payload_json)
                .context("invalid set_fast_backup_mode JSON payload")?;
            let applied = api::block_on(api::set_fast_backup_mode(req.enabled))??;
            Ok(json!({ "ok": true, "enabled": applied }))
        }
        "set_sync_paused" => {
            let req: SetSyncPausedPayload = serde_json::from_str(payload_json)
                .context("invalid set_sync_paused JSON payload")?;
            let paused = api::block_on(api::set_sync_paused(req.paused))??;
            Ok(json!({ "ok": true, "paused": paused }))
        }
        "get_sync_paused" => {
            let paused = api::block_on(api::get_sync_paused())??;
            Ok(json!({ "ok": true, "paused": paused }))
        }
        "set_bit_rot_auto_scrub" => {
            let req: SetBoolPayload = serde_json::from_str(payload_json)
                .context("invalid set_bit_rot_auto_scrub JSON payload")?;
            let enabled = api::block_on(api::set_bit_rot_auto_scrub(req.enabled))??;
            Ok(json!({ "ok": true, "enabled": enabled }))
        }
        "get_bit_rot_auto_scrub" => {
            let enabled = api::block_on(api::get_bit_rot_auto_scrub())??;
            Ok(json!({ "ok": true, "enabled": enabled }))
        }
        "set_sync_rule_schedule" => {
            let req: SetSyncRuleSchedulePayload = serde_json::from_str(payload_json)
                .context("invalid set_sync_rule_schedule JSON payload")?;
            let mode = api::block_on(api::set_sync_rule_schedule(req.rule_id, req.schedule_mode))??;
            Ok(json!({ "ok": true, "schedule_mode": mode }))
        }
        // -----------------------------------------------------------------
        // v2 Device Groups, Group Sync Rules, Catalog & Message Outbox
        // -----------------------------------------------------------------
        "list_device_groups" => {
            let groups = api::block_on(api::list_device_groups())??;
            Ok(json!({ "ok": true, "groups": groups }))
        }
        "create_device_group" => {
            let req: CreateDeviceGroupPayload = serde_json::from_str(payload_json)
                .context("invalid create_device_group JSON payload")?;
            let group = api::block_on(api::create_device_group(req.name))??;
            Ok(json!({ "ok": true, "group": group }))
        }
        "rename_device_group" => {
            let req: RenameDeviceGroupPayload = serde_json::from_str(payload_json)
                .context("invalid rename_device_group JSON payload")?;
            let group = api::block_on(api::rename_device_group(req.group_id, req.name))??;
            Ok(json!({ "ok": true, "group": group }))
        }
        "delete_device_group" => {
            let req: GroupIdPayload = serde_json::from_str(payload_json)
                .context("invalid delete_device_group JSON payload")?;
            api::block_on(api::delete_device_group(req.group_id))??;
            Ok(json!({ "ok": true }))
        }
        "add_group_member" => {
            let req: AddGroupMemberPayload = serde_json::from_str(payload_json)
                .context("invalid add_group_member JSON payload")?;
            api::block_on(api::add_group_member(req.group_id, req.device_id, req.role))??;
            Ok(json!({ "ok": true }))
        }
        "remove_group_member" => {
            let req: RemoveGroupMemberPayload = serde_json::from_str(payload_json)
                .context("invalid remove_group_member JSON payload")?;
            api::block_on(api::remove_group_member(req.group_id, req.device_id))??;
            Ok(json!({ "ok": true }))
        }
        "list_group_rules" => {
            let req: ListGroupRulesPayload = if payload_json.trim().is_empty() {
                ListGroupRulesPayload::default()
            } else {
                serde_json::from_str(payload_json)
                    .context("invalid list_group_rules JSON payload")?
            };
            let rules = api::block_on(api::list_group_rules(req.group_id))??;
            Ok(json!({ "ok": true, "rules": rules }))
        }
        "upsert_group_rule" => {
            let req: api::UpsertGroupRuleInput = serde_json::from_str(payload_json)
                .context("invalid upsert_group_rule JSON payload")?;
            let rule = api::block_on(api::upsert_group_rule(req))??;
            Ok(json!({ "ok": true, "rule": rule }))
        }
        "delete_group_rule" => {
            let req: DeleteSyncRulePayload = serde_json::from_str(payload_json)
                .context("invalid delete_group_rule JSON payload")?;
            api::block_on(api::delete_group_rule(req.rule_id))??;
            Ok(json!({ "ok": true }))
        }
        "toggle_group_rule" => {
            let req: ToggleSyncRulePayload = serde_json::from_str(payload_json)
                .context("invalid toggle_group_rule JSON payload")?;
            api::block_on(api::toggle_group_rule(req.rule_id, req.enabled))??;
            Ok(json!({ "ok": true }))
        }
        "upsert_rule_target_binding" => {
            let req: api::UpsertBindingInput = serde_json::from_str(payload_json)
                .context("invalid upsert_rule_target_binding JSON payload")?;
            let binding = api::block_on(api::upsert_rule_target_binding(req))??;
            Ok(json!({ "ok": true, "binding": binding }))
        }
        "disable_rule_target_binding" => {
            let req: BindingIdPayload = serde_json::from_str(payload_json)
                .context("invalid disable_rule_target_binding JSON payload")?;
            api::block_on(api::disable_rule_target_binding(req.binding_id))??;
            Ok(json!({ "ok": true }))
        }
        "register_mirror_root" => {
            let req: RegisterMirrorRootPayload = serde_json::from_str(payload_json)
                .context("invalid register_mirror_root JSON payload")?;
            let mirror_root = api::block_on(api::register_mirror_root(
                req.canonical_path,
                req.display_name,
            ))??;
            Ok(json!({ "ok": true, "mirror_root": mirror_root }))
        }
        "list_mirror_roots" => {
            let mirror_roots = api::block_on(api::list_mirror_roots())??;
            Ok(json!({ "ok": true, "mirror_roots": mirror_roots }))
        }
        "revoke_mirror_root" => {
            let req: RevokeMirrorRootPayload = serde_json::from_str(payload_json)
                .context("invalid revoke_mirror_root JSON payload")?;
            api::block_on(api::revoke_mirror_root(req.root_id))??;
            Ok(json!({ "ok": true }))
        }
        "preview_group_rule" => {
            let req: DeleteSyncRulePayload = serde_json::from_str(payload_json)
                .context("invalid preview_group_rule JSON payload")?;
            let previews = api::block_on(api::preview_group_rule(req.rule_id))??;
            Ok(json!({ "ok": true, "previews": previews }))
        }
        "trigger_group_rule_sync" => {
            let req: DeleteSyncRulePayload = serde_json::from_str(payload_json)
                .context("invalid trigger_group_rule_sync JSON payload")?;
            let outcome = api::block_on(api::trigger_group_rule_sync(req.rule_id))??;
            Ok(json!({ "ok": true, "outcome": outcome }))
        }
        "list_binding_catalog" => {
            let req: ListBindingCatalogPayload = serde_json::from_str(payload_json)
                .context("invalid list_binding_catalog JSON payload")?;
            let entries = api::block_on(api::list_binding_catalog(
                req.binding_id,
                req.after_path_key,
                req.limit,
                req.source_deleted_only,
            ))??;
            Ok(json!({ "ok": true, "entries": entries }))
        }
        "list_binding_versions" => {
            let req: ListBindingVersionsPayload = serde_json::from_str(payload_json)
                .context("invalid list_binding_versions JSON payload")?;
            let versions = api::block_on(api::list_binding_versions(
                req.binding_id,
                req.path_key,
                req.after_archived_at,
                req.after_version_id,
                req.limit,
            ))??;
            Ok(json!({ "ok": true, "versions": versions }))
        }
        "restore_catalog_version" => {
            let req: RestoreCatalogVersionPayload = serde_json::from_str(payload_json)
                .context("invalid restore_catalog_version JSON payload")?;
            let result = api::block_on(api::restore_catalog_version(
                req.binding_id,
                req.version_id,
                req.target_dir,
            ))??;
            Ok(json!({ "ok": true, "result": result }))
        }
        "enqueue_message_text" => {
            let req: EnqueueMessageTextPayload = serde_json::from_str(payload_json)
                .context("invalid enqueue_message_text JSON payload")?;
            let outbox =
                api::block_on(api::enqueue_message_text(req.target_device_ids, req.text))??;
            Ok(json!({ "ok": true, "outbox": outbox }))
        }
        "enqueue_message_file" => {
            let req: EnqueueMessageFilePayload = serde_json::from_str(payload_json)
                .context("invalid enqueue_message_file JSON payload")?;
            let outbox = api::block_on(api::enqueue_message_file(
                req.target_device_ids,
                req.file_path,
            ))??;
            Ok(json!({ "ok": true, "outbox": outbox }))
        }
        "retry_outbox_message" => {
            let req: RetryOutboxMessagePayload = serde_json::from_str(payload_json)
                .context("invalid retry_outbox_message JSON payload")?;
            let reset_targets = api::block_on(api::retry_outbox_message(req.client_msg_id))??;
            Ok(json!({ "ok": true, "reset_targets": reset_targets }))
        }
        "list_outbox_messages" => {
            let req: ListOutboxMessagesPayload = if payload_json.trim().is_empty() {
                ListOutboxMessagesPayload::default()
            } else {
                serde_json::from_str(payload_json)
                    .context("invalid list_outbox_messages JSON payload")?
            };
            let messages = api::block_on(api::list_outbox_messages(req.limit))??;
            Ok(json!({ "ok": true, "messages": messages }))
        }
        other => bail!("unsupported FFI function: `{other}`"),
    }
}

fn into_c_json_ptr(value: Value) -> *mut c_char {
    let serialized = value.to_string();
    CString::new(serialized)
        .unwrap_or_else(|_| {
            CString::new(r#"{"ok":false,"error":"FFI JSON contained interior NUL byte"}"#)
                .expect("static JSON string contains no NUL bytes")
        })
        .into_raw()
}

/// C-ABI 统一 JSON 调用入口，供 Dart `NativeLanVaultRustFfi` 通过 `DynamicLibrary.lookupFunction` 调用。
///
/// 返回的 UTF-8 C 字符串必须由调用方通过 `lanvault_ffi_free_string` 释放。
///
/// # Safety
///
/// `fn_name` 与 `payload_json` 必须各自指向以 NUL 结尾、在本次调用期间保持存活的 UTF-8 C 字符串，
/// 或为 NULL（此时返回错误 JSON 而不解引用）。
#[no_mangle]
pub unsafe extern "C" fn lanvault_ffi_invoke_json(
    fn_name: *const c_char,
    payload_json: *const c_char,
) -> *mut c_char {
    let outcome = std::panic::catch_unwind(|| -> Result<Value> {
        if fn_name.is_null() {
            bail!("fn_name pointer must not be null");
        }
        if payload_json.is_null() {
            bail!("payload_json pointer must not be null");
        }

        let fn_str = unsafe { CStr::from_ptr(fn_name) }
            .to_str()
            .context("fn_name is not valid UTF-8")?;
        let payload_str = unsafe { CStr::from_ptr(payload_json) }
            .to_str()
            .context("payload_json is not valid UTF-8")?;

        dispatch_json(fn_str, payload_str)
    });

    match outcome {
        Ok(Ok(val)) => into_c_json_ptr(val),
        Ok(Err(err)) => into_c_json_ptr(json!({ "ok": false, "error": format!("{err:#}") })),
        Err(_) => into_c_json_ptr(json!({ "ok": false, "error": "panic across FFI boundary" })),
    }
}

/// 释放由 `lanvault_ffi_invoke_json` 分配的 C 字符串。
///
/// # Safety
///
/// `ptr` 必须是由 `lanvault_ffi_invoke_json` 返回且尚未释放的指针，或为 NULL。
#[no_mangle]
pub unsafe extern "C" fn lanvault_ffi_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            drop(CString::from_raw(ptr));
        }
    }
}

/// 将捆绑的 `.woff2` 字体字节流在 Rust 原生层解压为 SFNT (`.otf`/`.ttf`)，供 Flutter `FontLoader` 跨平台注册。
/// 返回 `0` 表示成功（调用方须调用 `lanvault_ffi_free_bytes(*out_ptr, *out_len)` 释放），非 0 表示失败。
///
/// # Safety
///
/// `woff2_ptr` 须指向至少 `woff2_len` 字节的只读内存；`out_ptr` 与 `out_len` 须为有效可写指针。
#[no_mangle]
pub unsafe extern "C" fn lanvault_ffi_decode_woff2(
    woff2_ptr: *const u8,
    woff2_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    let outcome = std::panic::catch_unwind(|| -> i32 {
        if woff2_ptr.is_null() || out_ptr.is_null() || out_len.is_null() || woff2_len == 0 {
            return -1;
        }
        let input = unsafe { std::slice::from_raw_parts(woff2_ptr, woff2_len) };
        match lanvault_core::decode_woff2_to_sfnt(input) {
            Ok(sfnt) => {
                let mut boxed = sfnt.into_boxed_slice();
                let len = boxed.len();
                let ptr = boxed.as_mut_ptr();
                std::mem::forget(boxed);
                unsafe {
                    *out_ptr = ptr;
                    *out_len = len;
                }
                0
            }
            Err(_) => -2,
        }
    });
    outcome.unwrap_or(-3)
}

/// 释放由 `lanvault_ffi_decode_woff2` 分配的字节缓冲区。
///
/// # Safety
///
/// `ptr` 与 `len` 必须来自 `lanvault_ffi_decode_woff2` 的输出，且仅释放一次。
#[no_mangle]
pub unsafe extern "C" fn lanvault_ffi_free_bytes(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
        }
    }
}

#[cfg(target_os = "android")]
mod android_jni {
    use std::ffi::c_void;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use tracing::error;

    use crate::api;

    type JniEnvPtr = *mut *const *const c_void;
    type JClass = *mut c_void;
    type JString = *mut c_void;
    type JBoolean = u8;

    const JNI_FALSE: JBoolean = 0;
    const JNI_TRUE: JBoolean = 1;

    // 标准 JNI `JNINativeInterface_` UTF-16 函数表固定槽位（Android NDK / OpenJDK ABI 不可变）。
    const JNI_GET_STRING_LENGTH_INDEX: usize = 164;
    const JNI_GET_STRING_CHARS_INDEX: usize = 165;
    const JNI_RELEASE_STRING_CHARS_INDEX: usize = 166;

    type GetStringLengthFn = unsafe extern "C" fn(env: JniEnvPtr, string: JString) -> i32;
    type GetStringCharsFn =
        unsafe extern "C" fn(env: JniEnvPtr, string: JString, is_copy: *mut u8) -> *const u16;
    type ReleaseStringCharsFn =
        unsafe extern "C" fn(env: JniEnvPtr, string: JString, chars: *const u16);

    unsafe fn jstring_to_rust_string(env: JniEnvPtr, jstr: JString) -> Option<String> {
        if env.is_null() || jstr.is_null() {
            return None;
        }
        let vtable = *env;
        if vtable.is_null() {
            return None;
        }

        let get_len: GetStringLengthFn =
            std::mem::transmute(*vtable.add(JNI_GET_STRING_LENGTH_INDEX));
        let get_chars: GetStringCharsFn =
            std::mem::transmute(*vtable.add(JNI_GET_STRING_CHARS_INDEX));
        let release_chars: ReleaseStringCharsFn =
            std::mem::transmute(*vtable.add(JNI_RELEASE_STRING_CHARS_INDEX));

        let len = get_len(env, jstr);
        if len < 0 {
            return None;
        }
        if len == 0 {
            return Some(String::new());
        }

        let chars = get_chars(env, jstr, std::ptr::null_mut());
        if chars.is_null() {
            return None;
        }
        let slice = std::slice::from_raw_parts(chars, len as usize);
        let parsed = String::from_utf16(slice).ok();
        release_chars(env, jstr, chars);
        parsed
    }

    fn jni_start_server_impl(
        env: JniEnvPtr,
        vault_path: JString,
        quota_gb: i64,
        port: i32,
        supplied_web_password: Option<String>,
        daemon_mode: bool,
    ) -> JBoolean {
        let Some(path) = (unsafe { jstring_to_rust_string(env, vault_path) }) else {
            error!("JNI start server received null or invalid UTF-16 vaultPath");
            return JNI_FALSE;
        };
        if quota_gb <= 0 {
            error!(quota_gb, "JNI start server received non-positive quotaGb");
            return JNI_FALSE;
        }
        if port > u16::MAX as i32 {
            error!(port, "JNI start server received out-of-range port");
            return JNI_FALSE;
        }
        let requested_port = if port < 0 { None } else { Some(port as u16) };

        let web_password = match supplied_web_password.map(|value| value.trim().to_string()) {
            Some(value) if value.len() >= 8 => value,
            _ => match std::env::var("LANVAULT_WEB_PASSWORD") {
                Ok(value) if value.trim().len() >= 8 => value.trim().to_string(),
                _ => match api::load_or_generate_jni_web_password(daemon_mode) {
                    Ok(generated) => generated,
                    // The Kotlin signature is `Boolean`, so the only failure channel is `false`;
                    // the cause is logged rather than encoded in the return value.
                    Err(err) => {
                        error!(error = %err, "JNI web password generation failed");
                        return JNI_FALSE;
                    }
                },
            },
        };

        let outcome = if daemon_mode {
            api::block_on_daemon(api::start_local_server_for_daemon(
                path,
                quota_gb as u64,
                web_password,
                requested_port,
            ))
        } else {
            api::block_on(api::start_local_server_with_port(
                path,
                quota_gb as u64,
                web_password,
                requested_port,
            ))
        };

        match outcome {
            Ok(Ok(_port)) => JNI_TRUE,
            Ok(Err(err)) => {
                error!(error = %err, daemon_mode, "JNI start local server failed");
                JNI_FALSE
            }
            Err(err) => {
                error!(error = %err, daemon_mode, "JNI Tokio runtime error during start local server");
                JNI_FALSE
            }
        }
    }

    fn jni_graceful_restart_impl(daemon_mode: bool) -> JBoolean {
        let outcome = if daemon_mode {
            api::block_on_daemon(api::prepare_graceful_restart())
        } else {
            api::block_on(api::prepare_graceful_restart())
        };
        match outcome {
            Ok(Ok(())) => JNI_TRUE,
            Ok(Err(err)) => {
                error!(error = %err, "JNI prepare_graceful_restart failed");
                JNI_FALSE
            }
            Err(err) => {
                error!(error = %err, "JNI Tokio runtime error during prepare_graceful_restart");
                JNI_FALSE
            }
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_com_lanvault_app_privilege_ShizukuUserService_nativeStartServer(
        env: JniEnvPtr,
        _class: JClass,
        vault_path: JString,
        quota_gb: i64,
        port: i32,
        web_password: JString,
    ) -> JBoolean {
        catch_unwind(AssertUnwindSafe(|| {
            jni_start_server_impl(
                env,
                vault_path,
                quota_gb,
                port,
                unsafe { jstring_to_rust_string(env, web_password) },
                true,
            )
        }))
        .unwrap_or_else(|_| {
            error!("panic across JNI boundary in ShizukuUserService_nativeStartServer");
            JNI_FALSE
        })
    }

    #[no_mangle]
    pub extern "system" fn Java_com_lanvault_app_privilege_ShizukuUserService_nativeGracefulRestart(
        _env: JniEnvPtr,
        _class: JClass,
    ) -> JBoolean {
        catch_unwind(|| jni_graceful_restart_impl(true)).unwrap_or_else(|_| {
            error!("panic across JNI boundary in ShizukuUserService_nativeGracefulRestart");
            JNI_FALSE
        })
    }

    #[no_mangle]
    pub extern "system" fn Java_com_lanvault_app_privilege_RootDaemonService_nativeStartRootServer(
        env: JniEnvPtr,
        _class: JClass,
        vault_path: JString,
        quota_gb: i64,
        port: i32,
        web_password: JString,
    ) -> JBoolean {
        catch_unwind(AssertUnwindSafe(|| {
            jni_start_server_impl(
                env,
                vault_path,
                quota_gb,
                port,
                unsafe { jstring_to_rust_string(env, web_password) },
                true,
            )
        }))
        .unwrap_or_else(|_| {
            error!("panic across JNI boundary in RootDaemonService_nativeStartRootServer");
            JNI_FALSE
        })
    }

    #[no_mangle]
    pub extern "system" fn Java_com_lanvault_app_privilege_RootDaemonService_nativeGracefulRestartRootServer(
        _env: JniEnvPtr,
        _class: JClass,
    ) -> JBoolean {
        catch_unwind(|| jni_graceful_restart_impl(true)).unwrap_or_else(|_| {
            error!(
                "panic across JNI boundary in RootDaemonService_nativeGracefulRestartRootServer"
            );
            JNI_FALSE
        })
    }

    #[no_mangle]
    pub extern "system" fn Java_com_lanvault_app_privilege_KeepAliveService_nativeStartKeepAliveServer(
        env: JniEnvPtr,
        _class: JClass,
        vault_path: JString,
        quota_gb: i64,
        port: i32,
        web_password: JString,
    ) -> JBoolean {
        catch_unwind(AssertUnwindSafe(|| {
            jni_start_server_impl(
                env,
                vault_path,
                quota_gb,
                port,
                unsafe { jstring_to_rust_string(env, web_password) },
                false,
            )
        }))
        .unwrap_or_else(|_| {
            error!("panic across JNI boundary in KeepAliveService_nativeStartKeepAliveServer");
            JNI_FALSE
        })
    }

    #[no_mangle]
    pub extern "system" fn Java_com_lanvault_app_privilege_KeepAliveService_nativeGracefulRestartKeepAliveServer(
        _env: JniEnvPtr,
        _class: JClass,
    ) -> JBoolean {
        catch_unwind(|| jni_graceful_restart_impl(false)).unwrap_or_else(|_| {
            error!("panic across JNI boundary in KeepAliveService_nativeGracefulRestartKeepAliveServer");
            JNI_FALSE
        })
    }
}

#[derive(Deserialize)]
struct SetDebugModePayload {
    enabled: bool,
}

#[derive(Deserialize)]
struct GetDebugLogsPayload {
    #[serde(default = "default_log_limit")]
    limit: u32,
}

fn default_log_limit() -> u32 {
    200
}

#[derive(Deserialize)]
struct ReportClientLogPayload {
    level: String,
    source: String,
    message: String,
    detail: Option<String>,
}

#[derive(Deserialize)]
struct SetBackupStorageConfigPayload {
    primary_dir: String,
    #[serde(default)]
    fallback_dir: Option<String>,
}

#[derive(Deserialize)]
struct CreateVaultPayload {
    name: String,
    mount_path: String,
    pool_mode: String,
    quota_gb: u64,
    e2ee_enabled: bool,
    #[serde(default)]
    extra_mount_path: Option<String>,
}

#[derive(Deserialize, Default)]
struct ListSnapshotsPayload {
    #[serde(default)]
    vault_id: Option<String>,
}

#[derive(Deserialize)]
struct RevokePairedPeerPayload {
    device_id: String,
}

#[derive(Deserialize)]
struct ToggleSyncRulePayload {
    rule_id: String,
    enabled: bool,
}

#[derive(Deserialize)]
struct PushClipboardFilePayload {
    #[serde(default)]
    target_device_id: Option<String>,
    file_path: String,
}
#[derive(Deserialize)]
struct CreateDeviceGroupPayload {
    name: String,
}

#[derive(Deserialize)]
struct RenameDeviceGroupPayload {
    group_id: String,
    name: String,
}

#[derive(Deserialize)]
struct GroupIdPayload {
    group_id: String,
}

#[derive(Deserialize)]
struct AddGroupMemberPayload {
    group_id: String,
    device_id: String,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Deserialize)]
struct RemoveGroupMemberPayload {
    group_id: String,
    device_id: String,
}

#[derive(Deserialize, Default)]
struct ListGroupRulesPayload {
    #[serde(default)]
    group_id: Option<String>,
}

#[derive(Deserialize)]
struct BindingIdPayload {
    binding_id: String,
}

#[derive(Deserialize)]
struct RegisterMirrorRootPayload {
    canonical_path: String,
    #[serde(default)]
    display_name: String,
}

#[derive(Deserialize)]
struct RevokeMirrorRootPayload {
    root_id: String,
}

#[derive(Deserialize)]
struct ListBindingCatalogPayload {
    binding_id: String,
    #[serde(default)]
    after_path_key: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    source_deleted_only: bool,
}

#[derive(Deserialize)]
struct ListBindingVersionsPayload {
    binding_id: String,
    #[serde(default)]
    path_key: Option<String>,
    #[serde(default)]
    after_archived_at: Option<i64>,
    #[serde(default)]
    after_version_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct RestoreCatalogVersionPayload {
    binding_id: String,
    version_id: String,
    target_dir: String,
}

#[derive(Deserialize)]
struct EnqueueMessageTextPayload {
    target_device_ids: Vec<String>,
    text: String,
}

#[derive(Deserialize)]
struct EnqueueMessageFilePayload {
    target_device_ids: Vec<String>,
    file_path: String,
}

#[derive(Deserialize)]
struct RetryOutboxMessagePayload {
    client_msg_id: String,
}

#[derive(Deserialize, Default)]
struct ListOutboxMessagesPayload {
    #[serde(default)]
    limit: Option<usize>,
}
