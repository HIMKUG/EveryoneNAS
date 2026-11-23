// 参考依据:
// - lanvault_complete_blueprint.md 第 4 节 (Android 15 严禁在 BOOT_COMPLETED 直接启动 dataSync 前台服务)
// - ARCHITECTURE_CONTRACTS.md 第 5 节 (四级提权保活开机恢复分发)

package com.lanvault.app.receiver

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import com.lanvault.app.privilege.KeepAliveLevel
import com.lanvault.app.privilege.KeepAliveService
import com.lanvault.app.privilege.PrivilegeDetector
import com.lanvault.app.privilege.RootDaemonService
import com.lanvault.app.privilege.ShizukuBridge

/**
 * 开机自启恢复广播接收器：
 * 严格遵循 Android 15 (API 35) 限制——绝不在 BOOT_COMPLETED 中直接调用 startForegroundService 启动 dataSync FGS。
 * 使用 goAsync() 将权限探测与守护调度移至后台线程，避免阻塞广播主线程引发 ANR。
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) {
            return
        }

        val pendingResult = goAsync()
        val appContext = context.applicationContext
        Thread {
            try {
                if (KeepAliveService.isUserStopped(appContext)) {
                    return@Thread
                }
                val lastConfig = KeepAliveService.loadLastConfig(appContext)
                when (PrivilegeDetector.detect()) {
                    KeepAliveLevel.LEVEL_3_MAGISK_DAEMON -> {
                        // Level 3 由 /data/adb/modules/lanvault_daemon/service.sh 独立拉起与看门狗守护。
                    }
                    KeepAliveLevel.LEVEL_2_ROOT_LIBSU -> {
                        if (lastConfig != null) {
                            RootDaemonService.bindAndStart(
                                appContext,
                                lastConfig.vaultPath,
                                lastConfig.quotaGb,
                                lastConfig.port,
                                lastConfig.webPassword,
                            )
                        }
                    }
                    KeepAliveLevel.LEVEL_1_SHIZUKU -> {
                        if (lastConfig != null) {
                            ShizukuBridge.bindAndStart(
                                appContext,
                                lastConfig.vaultPath,
                                lastConfig.quotaGb,
                                lastConfig.port,
                                lastConfig.webPassword,
                            )
                        }
                    }
                    KeepAliveLevel.LEVEL_0_FOREGROUND -> {
                        // 严禁在此处调用 ContextCompat.startForegroundService(KeepAliveService)
                        KeepAliveService.scheduleRecoveryWork(appContext)
                    }
                }
            } finally {
                pendingResult.finish()
            }
        }.start()
    }
}
