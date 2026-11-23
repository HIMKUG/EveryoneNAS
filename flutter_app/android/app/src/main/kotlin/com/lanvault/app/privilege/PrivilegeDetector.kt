// 参考依据:
// - ARCHITECTURE_CONTRACTS.md 第 5 节 (KeepAliveLevel 枚举协议值)
// - lanvault_complete_blueprint.md 第 5.5 节 (Level 3 -> 2 -> 1 -> 0 顺序探测)
// - vendor_refs/android-libsu (Shell.getShell().isRoot / Shell.cmd)
// - vendor_refs/android-shizuku-api (Shizuku.pingBinder / checkSelfPermission)

package com.lanvault.app.privilege

import android.content.pm.PackageManager
import com.topjohnwu.superuser.Shell
import rikka.shizuku.Shizuku

enum class KeepAliveLevel(val wireValue: String) {
    LEVEL_3_MAGISK_DAEMON("LEVEL_3_MAGISK_DAEMON"),
    LEVEL_2_ROOT_LIBSU("LEVEL_2_ROOT_LIBSU"),
    LEVEL_1_SHIZUKU("LEVEL_1_SHIZUKU"),
    LEVEL_0_FOREGROUND("LEVEL_0_FOREGROUND"),
}

/**
 * 严格按 Level 3 -> Level 2 -> Level 1 -> Level 0 顺序探测当前设备可用最高保活通道。
 * 注意：调用方必须在后台线程执行，避免阻塞主线程。
 */
object PrivilegeDetector {
    private const val MAGISK_MODULE_DIR = "/data/adb/modules/lanvault_daemon"
    private const val MAGISK_DATA_DIR = "/data/adb/lanvault"
    private const val MAGISK_PID_FILE = "$MAGISK_DATA_DIR/server.pid"
    private const val MAGISK_WATCHDOG_PID_FILE = "$MAGISK_DATA_DIR/watchdog.pid"
    private const val MAGISK_STOP_SENTINEL = "$MAGISK_DATA_DIR/.stop_requested"

    fun detect(): KeepAliveLevel {
        if (Shell.getShell().isRoot) {
            if (isMagiskDaemonRunning()) {
                return KeepAliveLevel.LEVEL_3_MAGISK_DAEMON
            }
            return KeepAliveLevel.LEVEL_2_ROOT_LIBSU
        }
        if (isShizukuReady()) {
            return KeepAliveLevel.LEVEL_1_SHIZUKU
        }
        return KeepAliveLevel.LEVEL_0_FOREGROUND
    }

    /**
     * 通过 Root Shell 检查 Magisk 模块目录及守护进程（或停服态看门狗）是否处于活跃状态，
     * 避免非 Root 应用进程直接访问受 SELinux/DAC 保护的 /data/adb 目录。
     */
    fun isMagiskDaemonRunning(): Boolean {
        if (!Shell.getShell().isRoot) {
            return false
        }
        val result = Shell.cmd(
            "if [ -d '$MAGISK_MODULE_DIR' ] && [ ! -f '$MAGISK_MODULE_DIR/disable' ]; then " +
                "if [ -f '$MAGISK_PID_FILE' ]; then " +
                "pid=\$(cat '$MAGISK_PID_FILE' 2>/dev/null); " +
                "if [ -n \"\$pid\" ] && [ -d \"/proc/\$pid\" ]; then echo RUNNING; fi; " +
                "elif [ -f '$MAGISK_STOP_SENTINEL' ] && [ -f '$MAGISK_WATCHDOG_PID_FILE' ]; then " +
                "wd=\$(cat '$MAGISK_WATCHDOG_PID_FILE' 2>/dev/null); " +
                "if [ -n \"\$wd\" ] && [ -d \"/proc/\$wd\" ]; then echo RUNNING; fi; " +
                "fi; " +
                "fi",
        ).exec()
        return result.isSuccess && result.out.any { it.trim() == "RUNNING" }
    }

    /**
     * 清除 Level 3 停服哨兵并触发 service.sh 看门狗拉起 lanvault-server。
     */
    fun startMagiskDaemon(): Boolean {
        if (!Shell.getShell().isRoot) {
            return false
        }
        val result = Shell.cmd(
            "rm -f '$MAGISK_STOP_SENTINEL'",
            "wd=''; if [ -f '$MAGISK_WATCHDOG_PID_FILE' ]; then wd=\$(cat '$MAGISK_WATCHDOG_PID_FILE' 2>/dev/null); fi; " +
                "if [ -z \"\$wd\" ] || [ ! -d \"/proc/\$wd\" ]; then " +
                "if [ -f '$MAGISK_MODULE_DIR/service.sh' ]; then nohup sh '$MAGISK_MODULE_DIR/service.sh' >/dev/null 2>&1 & fi; " +
                "fi",
        ).exec()
        return result.isSuccess
    }

    /**
     * 写入停服哨兵 /data/adb/lanvault/.stop_requested 并向 lanvault-server 发送 SIGTERM，
     * 阻止 service.sh 看门狗在用户停服或切换 client_only 后重新拉起守护进程。
     */
    fun stopMagiskDaemon(): Boolean {
        if (!Shell.getShell().isRoot) {
            return false
        }
        val result = Shell.cmd(
            "mkdir -p '$MAGISK_DATA_DIR' && touch '$MAGISK_STOP_SENTINEL'",
            "if [ -f '$MAGISK_PID_FILE' ]; then " +
                "pid=\$(cat '$MAGISK_PID_FILE' 2>/dev/null); " +
                "if [ -n \"\$pid\" ] && [ -d \"/proc/\$pid\" ]; then kill -TERM \"\$pid\"; fi; " +
                "fi",
        ).exec()
        return result.isSuccess
    }

    /**
     * 向 Level 3 纯 Rust 守护进程发送 SIGTERM，由 service.sh 看门狗拉起重启。
     */
    fun signalMagiskDaemonRestart(): Boolean {
        if (!Shell.getShell().isRoot) {
            return false
        }
        val result = Shell.cmd(
            "rm -f '$MAGISK_STOP_SENTINEL'",
            "if [ -f '$MAGISK_PID_FILE' ]; then " +
                "pid=\$(cat '$MAGISK_PID_FILE' 2>/dev/null); " +
                "if [ -n \"\$pid\" ] && [ -d \"/proc/\$pid\" ]; then kill -TERM \"\$pid\"; fi; " +
                "fi",
        ).exec()
        return result.isSuccess
    }

    private fun isShizukuReady(): Boolean {
        if (!Shizuku.pingBinder()) {
            return false
        }
        if (Shizuku.isPreV11()) {
            return false
        }
        return Shizuku.checkSelfPermission() == PackageManager.PERMISSION_GRANTED
    }
}
