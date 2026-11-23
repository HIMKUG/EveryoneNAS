//! `lanvault-debug-cli`: AI 与开发者专用的无头全功能调试命令行工具。
//!
//! 直接复用 `lanvault_ffi::c_abi::lanvault_ffi_invoke_json`，可在无需启动 GUI 的情况下
//! 通过脚本驱动软件全部核心功能、开启 Debug 模式并输出微秒级探针日志。
//!
//! 用法示例：
//!   lanvault-debug-cli --data-dir ./tmp_dbg --debug self-check
//!   lanvault-debug-cli --data-dir ./tmp_dbg invoke get_role_mode "{}"
//!   lanvault-debug-cli --data-dir ./tmp_dbg batch scripts/debug_scenario_example.json
//!   lanvault-debug-cli --data-dir ./tmp_dbg logs --limit 100

use std::env;
use std::ffi::{CStr, CString};
use std::fs;
use std::process::ExitCode;
use std::time::Instant;

use serde::Deserialize;
use serde_json::{json, Value};

use lanvault_ffi::c_abi::{lanvault_ffi_free_string, lanvault_ffi_invoke_json};

#[derive(Deserialize)]
struct ScenarioStep {
    name: String,
    #[serde(rename = "fn")]
    fn_name: String,
    #[serde(default = "default_empty_obj")]
    payload: Value,
    #[serde(default)]
    expect_error: bool,
}

fn default_empty_obj() -> Value {
    json!({})
}

fn call_ffi(fn_name: &str, payload: &str) -> Value {
    let c_fn = match CString::new(fn_name) {
        Ok(value) => value,
        Err(_) => return json!({ "ok": false, "error": "fn_name must not contain NUL" }),
    };
    let c_payload = match CString::new(payload) {
        Ok(value) => value,
        Err(_) => return json!({ "ok": false, "error": "payload must not contain NUL" }),
    };
    let raw_ptr = unsafe { lanvault_ffi_invoke_json(c_fn.as_ptr(), c_payload.as_ptr()) };
    if raw_ptr.is_null() {
        return json!({ "ok": false, "error": "FFI returned null pointer" });
    }
    let text = unsafe { CStr::from_ptr(raw_ptr) }.to_string_lossy().into_owned();
    unsafe { lanvault_ffi_free_string(raw_ptr) };
    serde_json::from_str(&text)
        .unwrap_or_else(|_| json!({ "ok": false, "error": "invalid JSON from FFI", "raw": text }))
}

fn print_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(formatted) => println!("{formatted}"),
        Err(err) => eprintln!("error: failed to serialize JSON output: {err}"),
    }
}

fn response_ok(resp: &Value) -> bool {
    resp.get("ok").and_then(Value::as_bool).unwrap_or(false)
}

fn print_usage() {
    eprintln!(
        "Usage:
  lanvault-debug-cli [--data-dir <PATH>] [--debug] <COMMAND>

Commands:
  invoke <fn_name> [json_payload]   调用任意软件 FFI 功能并输出 JSON 结果
  batch <scenario.json>             顺序执行 JSON 测试脚本并输出每步耗时与断言结果
  self-check                        运行内置探针自检并返回诊断报告
  logs [--limit N]                  拉取统一 Debug/探针日志
  clear-logs                        清空 Debug 日志"
    );
}

fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let mut enable_debug = false;

    while !args.is_empty() {
        match args[0].as_str() {
            "--data-dir" => {
                if args.len() < 2 {
                    eprintln!("error: --data-dir requires a path");
                    return ExitCode::FAILURE;
                }
                env::set_var("LANVAULT_DATA_DIR", &args[1]);
                args.drain(0..2);
            }
            "--debug" => {
                enable_debug = true;
                args.remove(0);
            }
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            _ => break,
        }
    }

    if args.is_empty() {
        print_usage();
        return ExitCode::FAILURE;
    }

    if enable_debug {
        let _ = call_ffi("set_debug_mode", r#"{"enabled":true}"#);
    }

    match args[0].as_str() {
        "invoke" => {
            if args.len() < 2 {
                eprintln!("error: invoke requires <fn_name>");
                return ExitCode::FAILURE;
            }
            let fn_name = &args[1];
            let payload = args.get(2).map(String::as_str).unwrap_or("{}");
            let t0 = Instant::now();
            let resp = call_ffi(fn_name, payload);
            let elapsed_us = t0.elapsed().as_micros() as u64;
            let ok = response_ok(&resp);
            let out = json!({
                "fn": fn_name,
                "elapsed_us": elapsed_us,
                "response": resp,
            });
            print_json(&out);
            if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        "self-check" => {
            let resp = call_ffi("run_self_diagnostics", "{}");
            print_json(&resp);
            if response_ok(&resp) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        "logs" => {
            let limit: u32 = match args.get(1).map(String::as_str) {
                None => 200,
                Some("--limit") => match args.get(2) {
                    Some(raw) => match raw.parse::<u32>() {
                        Ok(val) => val,
                        Err(_) => {
                            eprintln!("error: invalid --limit value `{raw}`");
                            return ExitCode::FAILURE;
                        }
                    },
                    None => {
                        eprintln!("error: --limit requires an integer value");
                        return ExitCode::FAILURE;
                    }
                },
                Some(other) => {
                    eprintln!("error: unexpected argument `{other}` for logs");
                    return ExitCode::FAILURE;
                }
            };
            let resp = call_ffi("get_debug_logs", &json!({ "limit": limit }).to_string());
            print_json(&resp);
            if response_ok(&resp) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        "clear-logs" => {
            let resp = call_ffi("clear_debug_logs", "{}");
            print_json(&resp);
            if response_ok(&resp) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        "batch" => {
            if args.len() < 2 {
                eprintln!("error: batch requires <scenario.json>");
                return ExitCode::FAILURE;
            }
            let raw = match fs::read_to_string(&args[1]) {
                Ok(content) => content,
                Err(err) => {
                    eprintln!("error: failed to read scenario {}: {err}", args[1]);
                    return ExitCode::FAILURE;
                }
            };
            let steps: Vec<ScenarioStep> = match serde_json::from_str(&raw) {
                Ok(s) => s,
                Err(err) => {
                    eprintln!("error: invalid scenario JSON: {err}");
                    return ExitCode::FAILURE;
                }
            };
            let mut results = Vec::with_capacity(steps.len());
            let mut all_passed = true;
            for step in steps {
                let payload_str = step.payload.to_string();
                let t0 = Instant::now();
                let resp = call_ffi(&step.fn_name, &payload_str);
                let elapsed_us = t0.elapsed().as_micros() as u64;
                let ok = response_ok(&resp);
                let passed = if step.expect_error { !ok } else { ok };
                if !passed {
                    all_passed = false;
                }
                results.push(json!({
                    "name": step.name,
                    "fn": step.fn_name,
                    "expect_error": step.expect_error,
                    "passed": passed,
                    "elapsed_us": elapsed_us,
                    "response": resp,
                }));
            }
            let log_path = call_ffi("get_debug_log_path", "{}")
                .get("log_path")
                .cloned()
                .unwrap_or(Value::Null);
            let summary = json!({
                "all_passed": all_passed,
                "total_steps": results.len(),
                "log_path": log_path,
                "steps": results,
            });
            print_json(&summary);
            if all_passed { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        other => {
            eprintln!("error: unknown command `{other}`");
            print_usage();
            ExitCode::FAILURE
        }
    }
}
