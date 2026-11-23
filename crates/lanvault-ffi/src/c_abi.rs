//! C-ABI JSON 分发层与 Android JNI 符号导出。
//!
//! - C-ABI 符号：`lanvault_ffi_invoke_json`、`lanvault_ffi_free_string`，
//!   与 `flutter_app/lib/bridge/lanvault_bridge.dart` 100% 对齐并覆盖 `api.rs` 全部导出函数（含三态模式控制）。
//! - Android JNI 符号（`#[cfg(target_os = "android")]`）：
//!   与 `ShizukuUserService.kt` 和 `RootDaemonService.kt` 的 `external fun` 声明精确匹配。

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
            let req: SetRoleModePayload = serde_json::from_str(payload_json)
                .context("invalid set_role_mode JSON payload")?;
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
                .unwrap_or(GetDebugLogsPayload { limit: 200 });
            let entries = api::block_on(api::get_debug_logs(req.limit))??;
            Ok(json!({ "ok": true, "debug_enabled": lanvault_core::is_debug_enabled(), "entries": entries }))
        }
        "clear_debug_logs" => {
            api::block_on(api::clear_debug_logs())??;
            Ok(json!({ "ok": true }))
        }
        "report_client_log" => {
            let req: ReportClientLogPayload = serde_json::from_str(payload_json)
                .context("invalid report_client_log JSON payload")?;
            api::block_on(api::report_client_log(req.level, req.source, req.message, req.detail))??;
            Ok(json!({ "ok": true }))
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

#[cfg(target_os = "android")]
mod android_jni {
    use std::ffi::{c_void, CStr};
    use std::os::raw::c_char;

    use tracing::error;

    use crate::api;

    type JniEnvPtr = *mut *const *const c_void;
    type JClass = *mut c_void;
    type JString = *mut c_void;
    type JBoolean = u8;

    const JNI_FALSE: JBoolean = 0;
    const JNI_TRUE: JBoolean = 1;

    // 标准 JNI `JNINativeInterface_` 函数表固定槽位（Android NDK / OpenJDK ABI 不可变）。
    const JNI_GET_STRING_UTF_CHARS_INDEX: usize = 169;
    const JNI_RELEASE_STRING_UTF_CHARS_INDEX: usize = 170;

    type GetStringUtfCharsFn =
        unsafe extern "C" fn(env: JniEnvPtr, string: JString, is_copy: *mut u8) -> *const c_char;
    type ReleaseStringUtfCharsFn =
        unsafe extern "C" fn(env: JniEnvPtr, string: JString, utf: *const c_char);

    unsafe fn jstring_to_rust_string(env: JniEnvPtr, jstr: JString) -> Option<String> {
        if env.is_null() || jstr.is_null() {
            return None;
        }
        let vtable = *env;
        if vtable.is_null() {
            return None;
        }

        let get_utf: GetStringUtfCharsFn =
            std::mem::transmute(*vtable.add(JNI_GET_STRING_UTF_CHARS_INDEX));
        let release_utf: ReleaseStringUtfCharsFn =
            std::mem::transmute(*vtable.add(JNI_RELEASE_STRING_UTF_CHARS_INDEX));

        let chars = get_utf(env, jstr, std::ptr::null_mut());
        if chars.is_null() {
            return None;
        }
        let parsed = CStr::from_ptr(chars).to_str().ok().map(str::to_owned);
        release_utf(env, jstr, chars);
        parsed
    }

    fn jni_start_server_impl(
        env: JniEnvPtr,
        vault_path: JString,
        quota_gb: i64,
        port: i32,
        supplied_web_password: Option<String>,
    ) -> JBoolean {
        let Some(path) = (unsafe { jstring_to_rust_string(env, vault_path) }) else {
            error!("JNI start server received null or non-UTF-8 vaultPath");
            return JNI_FALSE;
        };
        if quota_gb <= 0 {
            error!(quota_gb, "JNI start server received non-positive quotaGb");
            return JNI_FALSE;
        }
        if port < 0 || port > u16::MAX as i32 {
            error!(port, "JNI start server received out-of-range port");
            return JNI_FALSE;
        }
        let requested_port = if port > 0 { Some(port as u16) } else { None };

        let web_password = match supplied_web_password.map(|value| value.trim().to_string()) {
            Some(value) if value.len() >= 8 => value,
            _ => match std::env::var("LANVAULT_WEB_PASSWORD") {
                Ok(value) if value.len() >= 8 => value,
                _ => match api::load_or_generate_jni_web_password() {
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

        match api::block_on(api::start_local_server_with_port(
            path,
            quota_gb as u64,
            web_password,
            requested_port,
        )) {
            Ok(Ok(_port)) => JNI_TRUE,
            Ok(Err(err)) => {
                error!(error = %err, "JNI start_local_server_with_port failed");
                JNI_FALSE
            }
            Err(err) => {
                error!(error = %err, "JNI Tokio runtime error during start_local_server_with_port");
                JNI_FALSE
            }
        }
    }

    fn jni_graceful_restart_impl() -> JBoolean {
        match api::block_on(api::prepare_graceful_restart()) {
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
        jni_start_server_impl(
            env,
            vault_path,
            quota_gb,
            port,
            unsafe { jstring_to_rust_string(env, web_password) },
        )
    }

    #[no_mangle]
    pub extern "system" fn Java_com_lanvault_app_privilege_ShizukuUserService_nativeGracefulRestart(
        _env: JniEnvPtr,
        _class: JClass,
    ) -> JBoolean {
        jni_graceful_restart_impl()
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
        jni_start_server_impl(
            env,
            vault_path,
            quota_gb,
            port,
            unsafe { jstring_to_rust_string(env, web_password) },
        )
    }

    #[no_mangle]
    pub extern "system" fn Java_com_lanvault_app_privilege_RootDaemonService_nativeGracefulRestartRootServer(
        _env: JniEnvPtr,
        _class: JClass,
    ) -> JBoolean {
        jni_graceful_restart_impl()
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
        jni_start_server_impl(
            env,
            vault_path,
            quota_gb,
            port,
            unsafe { jstring_to_rust_string(env, web_password) },
        )
    }

    #[no_mangle]
    pub extern "system" fn Java_com_lanvault_app_privilege_KeepAliveService_nativeGracefulRestartKeepAliveServer(
        _env: JniEnvPtr,
        _class: JClass,
    ) -> JBoolean {
        jni_graceful_restart_impl()
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
