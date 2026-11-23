#!/usr/bin/env python3
"""
EveryoneNAS AI Automation & Debug Runner (Zero-Dependency Python ctypes Driver)

允许 AI（或开发者脚本）直接加载 `lanvault_ffi.dll` / `liblanvault_ffi.so` / `liblanvault_ffi.dylib`，
在隔离沙盒目录中自动调用软件的全部功能、开启 Debug 模式、采集微秒级探针日志并生成 JSON 诊断报告。

用法示例:
  python scripts/ai_debug_runner.py list
  python scripts/ai_debug_runner.py self-check --data-dir ./tmp_dbg
  python scripts/ai_debug_runner.py call set_role_mode '{"mode":"dual"}'
  python scripts/ai_debug_runner.py suite --report debug_reports/report.json
  python scripts/ai_debug_runner.py logs --limit 100 --level PROBE
"""

import argparse
import ctypes
import json
import os
import platform
import sys
import tempfile
import time
from pathlib import Path

FUNCTIONS_CATALOG = {
    "set_debug_mode": {"payload": {"enabled": True}, "desc": "开启/关闭全局 Debug 模式与微秒级探针"},
    "get_debug_mode": {"payload": {}, "desc": "查询当前 Debug 模式开关状态"},
    "get_debug_log_path": {"payload": {}, "desc": "获取磁盘 lanvault_debug.log 完整路径"},
    "get_debug_logs": {"payload": {"limit": 200}, "desc": "拉取统一探针与错误日志"},
    "clear_debug_logs": {"payload": {}, "desc": "清空内存与磁盘 Debug 日志"},
    "run_self_diagnostics": {"payload": {}, "desc": "运行内置全链路探针自检"},
    "report_client_log": {"payload": {"level": "INFO", "source": "ai.runner", "message": "ping"}, "desc": "写入前端或 AI 诊断日志"},
    "get_role_mode": {"payload": {}, "desc": "读取运行模式（client_only / server_only / dual）"},
    "set_role_mode": {"payload": {"mode": "dual"}, "desc": "切换运行模式"},
    "start_mdns_scan": {"payload": {}, "desc": "触发 mDNS + UDP 7843 局域网扫描并返回发现列表"},
    "pair_with_pin": {"payload": {"ip": "127.0.0.1", "port": 7842, "pin": "123456"}, "desc": "使用 6 位 PIN 配对设备"},
    "list_paired_peers": {"payload": {}, "desc": "列出已配对设备（快传下拉数据源）"},
    "enable_auto_reconnect": {"payload": {"enabled": True}, "desc": "开启/关闭断线自动重连"},
    "get_auto_reconnect": {"payload": {}, "desc": "查询断线自动重连状态"},
    "list_sync_rules": {"payload": {}, "desc": "列出需要自动备份的本地文件夹"},
    "upsert_sync_rule": {"payload": {"rule_id": "test-dir", "local_dir": "C:\\Temp", "target_vault_id": "default", "enabled": True}, "desc": "新增或更新备份文件夹"},
    "delete_sync_rule": {"payload": {"rule_id": "test-dir"}, "desc": "移除备份文件夹"},
    "trigger_incremental_scan": {"payload": {"rule_id": "test-dir"}, "desc": "触发指定规则增量扫描"},
    "subscribe_sync_progress": {"payload": {}, "desc": "获取最新同步进度快照"},
    "get_backup_storage_config": {"payload": {}, "desc": "读取主/备备份文件夹配置"},
    "set_backup_storage_config": {"payload": {"primary_dir": "D:\\EveryoneNAS_Primary", "fallback_dir": "E:\\EveryoneNAS_Backup"}, "desc": "保存主/备备份文件夹（含跨盘符校验）"},
    "get_power_network_policy": {"payload": {}, "desc": "读取网络与上传限速策略"},
    "update_power_network_policy": {"payload": {"wifi_only": False, "allowed_ssids": [], "require_charging": False, "max_kbps": 0}, "desc": "更新网络与上传限速策略"},
    "update_device_environment": {"payload": {"on_wifi": True, "current_ssid": None, "is_charging": True}, "desc": "注入当前网络与电源环境"},
    "push_clipboard_text": {"payload": {"target_device_id": None, "text": "hello"}, "desc": "通过加密信道发送快传消息"},
    "start_local_server": {"payload": {"vault_path": "./vault", "quota_gb": 10, "web_password": "TestPassword123!", "port": 0}, "desc": "启动本地存储与 Web 管理服务端"},
    "prepare_graceful_restart": {"payload": {}, "desc": "安全排空并停止本地服务端（刷盘 WAL）"},
    "export_vault_key_backup": {"payload": {"passphrase": "StrongPassphrase123!"}, "desc": "导出加密主密钥备份"},
    "import_vault_key_backup": {"payload": {"passphrase": "StrongPassphrase123!", "backup_json": "{}"}, "desc": "导入加密主密钥备份"},
}


def find_ffi_library(explicit_path: str | None = None) -> Path:
    if explicit_path:
        p = Path(explicit_path)
        if p.is_file():
            return p
        raise FileNotFoundError(f"FFI library not found at {explicit_path}")

    system = platform.system().lower()
    lib_name = (
        "lanvault_ffi.dll"
        if system == "windows"
        else ("liblanvault_ffi.dylib" if system == "darwin" else "liblanvault_ffi.so")
    )

    repo_root = Path(__file__).resolve().parent.parent
    candidates = [
        repo_root / "target" / "release" / lib_name,
        repo_root / "target" / "debug" / lib_name,
        repo_root / "flutter_app" / "build" / "windows" / "x64" / "runner" / "Release" / lib_name,
        repo_root / "flutter_app" / "build" / "windows" / "x64" / "runner" / "Debug" / lib_name,
        Path.cwd() / lib_name,
    ]
    for cand in candidates:
        if cand.is_file():
            return cand
    raise FileNotFoundError(
        f"找不到 {lib_name}。请先在仓库根目录运行: cargo build --release -p lanvault-ffi"
    )


class LanVaultDriver:
    def __init__(self, lib_path: Path):
        self.lib_path = lib_path
        self.lib = ctypes.CDLL(str(lib_path))
        self.lib.lanvault_ffi_invoke_json.argtypes = [ctypes.c_char_p, ctypes.c_char_p]
        self.lib.lanvault_ffi_invoke_json.restype = ctypes.c_void_p
        self.lib.lanvault_ffi_free_string.argtypes = [ctypes.c_void_p]
        self.lib.lanvault_ffi_free_string.restype = None

    def invoke(self, fn_name: str, payload: dict | None = None) -> tuple[dict, int]:
        payload_bytes = json.dumps(payload or {}, ensure_ascii=False).encode("utf-8")
        t0 = time.perf_counter()
        ptr = self.lib.lanvault_ffi_invoke_json(fn_name.encode("utf-8"), payload_bytes)
        elapsed_us = int((time.perf_counter() - t0) * 1_000_000)
        if not ptr:
            return {"ok": False, "error": "FFI returned NULL pointer"}, elapsed_us
        try:
            raw = ctypes.cast(ptr, ctypes.c_char_p).value.decode("utf-8")
            return json.loads(raw), elapsed_us
        finally:
            self.lib.lanvault_ffi_free_string(ptr)


def run_regression_suite(driver: LanVaultDriver, temp_root: Path) -> dict:
    steps = []

    def run_step(name: str, fn_name: str, payload: dict | None = None, expect_ok: bool = True):
        resp, elapsed_us = driver.invoke(fn_name, payload)
        actual_ok = bool(resp.get("ok"))
        passed = actual_ok == expect_ok
        steps.append({
            "name": name,
            "fn": fn_name,
            "expect_ok": expect_ok,
            "passed": passed,
            "elapsed_us": elapsed_us,
            "response": resp,
        })
        return resp

    sample_src_dir = temp_root / "sample_backup_src"
    sample_src_dir.mkdir(parents=True, exist_ok=True)
    (sample_src_dir / "hello.txt").write_text("EveryoneNAS debug probe test", encoding="utf-8")
    vault_dir = temp_root / "test_vault"
    vault_dir.mkdir(parents=True, exist_ok=True)

    run_step("1. 开启 Debug 模式", "set_debug_mode", {"enabled": True})
    run_step("2. 清空历史日志", "clear_debug_logs", {})
    run_step("3. 写入 AI 探针标记", "report_client_log", {
        "level": "INFO",
        "source": "ai.runner",
        "message": "AI automated regression suite started",
    })
    run_step("4. 切换双端模式", "set_role_mode", {"mode": "dual"})
    run_step("5. 验证 mDNS 扫描不闪退", "start_mdns_scan", {})
    run_step("6. 切换断线自动重连", "enable_auto_reconnect", {"enabled": True})
    run_step("7. 查询断线自动重连状态", "get_auto_reconnect", {})
    run_step("8. 添加测试备份文件夹", "upsert_sync_rule", {
        "rule_id": "ai-smoke-folder",
        "local_dir": str(sample_src_dir),
        "target_vault_id": "default",
        "enabled": True,
    })
    run_step("9. 读取备份文件夹列表", "list_sync_rules", {})
    run_step("10. 移除测试备份文件夹", "delete_sync_rule", {"rule_id": "ai-smoke-folder"})
    run_step("11. 校验同盘符主备拒绝拦截（反向用例）", "set_backup_storage_config", {
        "primary_dir": r"D:\EveryoneNAS_Primary",
        "fallback_dir": r"D:\EveryoneNAS_Backup",
    }, expect_ok=False)
    run_step("12. 保存跨盘符主备备份目录", "set_backup_storage_config", {
        "primary_dir": r"D:\EveryoneNAS_Primary",
        "fallback_dir": r"E:\EveryoneNAS_Backup",
    })
    run_step("13. 读取主备备份配置", "get_backup_storage_config", {})
    run_step("14. 更新网络与上传限速策略", "update_power_network_policy", {
        "wifi_only": False,
        "allowed_ssids": [],
        "require_charging": False,
        "max_kbps": 10240,
    })
    run_step("15. 读取已配对设备列表", "list_paired_peers", {})
    run_step("16. 启动隔离本地存储服务端（随机端口）", "start_local_server", {
        "vault_path": str(vault_dir),
        "quota_gb": 5,
        "web_password": "DebugPassword123!",
        "port": 0,
    })
    run_step("17. 执行全链路内置探针自检", "run_self_diagnostics", {})
    run_step("18. 优雅停止本地服务端并刷盘 WAL", "prepare_graceful_restart", {})
    log_path_resp = run_step("19. 查询磁盘 Debug 日志路径", "get_debug_log_path", {})
    logs_resp, _ = driver.invoke("get_debug_logs", {"limit": 500})

    passed_count = sum(1 for s in steps if s["passed"])
    return {
        "all_passed": passed_count == len(steps),
        "passed": passed_count,
        "total": len(steps),
        "lib_path": str(driver.lib_path),
        "log_file_path": log_path_resp.get("log_path"),
        "steps": steps,
        "probe_entries_count": len(logs_resp.get("entries", [])),
        "probe_entries": logs_resp.get("entries", []),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="EveryoneNAS AI 自动化全功能调试驱动")
    parser.add_argument("--lib", help="显式指定 lanvault_ffi 动态库路径")
    parser.add_argument("--data-dir", help="指定隔离的 LANVAULT_DATA_DIR（避免污染真实配置）")
    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("list", help="列出所有可供 AI 调用的软件功能与参数模板")

    p_call = sub.add_parser("call", help="直接调用任意单个 FFI 功能")
    p_call.add_argument("fn_name", help="FFI 函数名")
    p_call.add_argument("payload", nargs="?", default="{}", help="JSON 参数字符串")
    p_call.add_argument("--debug", action="store_true", help="调用前先开启 Debug 模式")

    sub.add_parser("self-check", help="运行内置探针自检")

    p_suite = sub.add_parser("suite", help="在隔离沙盒中运行全功能自动化调试与探针回归套件")
    p_suite.add_argument("--report", help="将完整 JSON 报告输出到指定文件")

    p_logs = sub.add_parser("logs", help="拉取并过滤统一 Debug/探针日志")
    p_logs.add_argument("--limit", type=int, default=200)
    p_logs.add_argument("--level", help="按等级过滤，如 PROBE / ERROR / DEBUG / INFO")

    args = parser.parse_args()

    if args.cmd == "list":
        print(json.dumps(FUNCTIONS_CATALOG, ensure_ascii=False, indent=2))
        return 0

    temp_ctx = None
    if args.data_dir:
        os.environ["LANVAULT_DATA_DIR"] = str(Path(args.data_dir).resolve())
    elif args.cmd == "suite":
        temp_ctx = tempfile.TemporaryDirectory(prefix="everyonenas_ai_debug_")
        os.environ["LANVAULT_DATA_DIR"] = temp_ctx.name

    try:
        driver = LanVaultDriver(find_ffi_library(args.lib))

        if args.cmd == "call":
            if args.debug:
                driver.invoke("set_debug_mode", {"enabled": True})
            payload = json.loads(args.payload)
            resp, elapsed_us = driver.invoke(args.fn_name, payload)
            print(json.dumps({"fn": args.fn_name, "elapsed_us": elapsed_us, "response": resp}, ensure_ascii=False, indent=2))
            return 0 if resp.get("ok") else 1

        if args.cmd == "self-check":
            resp, elapsed_us = driver.invoke("run_self_diagnostics", {})
            print(json.dumps({"elapsed_us": elapsed_us, "response": resp}, ensure_ascii=False, indent=2))
            return 0 if resp.get("ok") else 1

        if args.cmd == "logs":
            resp, _ = driver.invoke("get_debug_logs", {"limit": args.limit})
            entries = resp.get("entries", [])
            if args.level:
                target = args.level.upper()
                entries = [e for e in entries if e.get("level", "").upper() == target]
            print(json.dumps({"debug_enabled": resp.get("debug_enabled"), "count": len(entries), "entries": entries}, ensure_ascii=False, indent=2))
            return 0

        if args.cmd == "suite":
            work_dir = Path(os.environ["LANVAULT_DATA_DIR"])
            report = run_regression_suite(driver, work_dir)
            if args.report:
                out_path = Path(args.report)
                out_path.parent.mkdir(parents=True, exist_ok=True)
                out_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
            # 终端输出精简汇总，完整探针明细已写入 report 与 lanvault_debug.log
            summary = {k: v for k, v in report.items() if k != "probe_entries"}
            print(json.dumps(summary, ensure_ascii=False, indent=2))
            return 0 if report["all_passed"] else 1
    finally:
        if temp_ctx is not None:
            temp_ctx.cleanup()

    return 0


if __name__ == "__main__":
    sys.exit(main())
