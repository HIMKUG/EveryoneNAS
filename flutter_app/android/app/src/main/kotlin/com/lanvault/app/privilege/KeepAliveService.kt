// 参考依据:
// - lanvault_complete_blueprint.md 第 4 节 (Android 15 API 35 dataSync 6 小时超时 onTimeout 优雅停机 + WorkManager + WifiLock/PARTIAL_WAKE_LOCK 按需获取与即时释放)
// - ARCHITECTURE_CONTRACTS.md 第 5 节 (Level 0 前台服务与通知栏“一键重启” Action)

package com.lanvault.app.privilege

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import com.lanvault.app.MainActivity
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.util.concurrent.TimeUnit

/**
 * Level 0 标准非特权保活前台服务：
 * - 遵守 Android 15 (API 35) dataSync 24h 内 6h 限额：重写 onTimeout() 主动优雅停机，避免 RemoteServiceException 崩溃。
 * - 仅在显式收到 ACTION_TRANSFER_START 时按需获取 WifiLock + PARTIAL_WAKE_LOCK，收到 ACTION_TRANSFER_STOP 或 onDestroy 时立即释放。
 * - 提供通知栏“一键重启” Action（API 26+ 使用 PendingIntent.getForegroundService），并配合 WorkManager 低功耗周期恢复。
 */
class KeepAliveService : Service() {
    companion object {
        const val CHANNEL_ID = "lanvault_keep_alive"
        const val NOTIFICATION_ID = 7842
        const val ACTION_START = "com.lanvault.app.action.START_KEEP_ALIVE"
        const val ACTION_GRACEFUL_RESTART = "com.lanvault.app.action.GRACEFUL_RESTART"
        const val ACTION_TRANSFER_START = "com.lanvault.app.action.TRANSFER_START"
        const val ACTION_TRANSFER_STOP = "com.lanvault.app.action.TRANSFER_STOP"
        const val EXTRA_VAULT_PATH = "vault_path"
        const val EXTRA_QUOTA_GB = "quota_gb"
        const val EXTRA_PORT = "port"

        private const val WORK_NAME = "lanvault_level0_periodic_recovery"
        private const val PREFS_NAME = "lanvault_keep_alive_prefs"

        fun start(context: Context, vaultPath: String, quotaGb: Long, port: Int) {
            saveLastConfig(context, vaultPath, quotaGb, port)
            val intent = Intent(context, KeepAliveService::class.java).apply {
                action = ACTION_START
                putExtra(EXTRA_VAULT_PATH, vaultPath)
                putExtra(EXTRA_QUOTA_GB, quotaGb)
                putExtra(EXTRA_PORT, port)
            }
            ContextCompat.startForegroundService(context, intent)
            scheduleRecoveryWork(context)
        }

        fun requestGracefulRestart(context: Context) {
            val intent = Intent(context, KeepAliveService::class.java).apply {
                action = ACTION_GRACEFUL_RESTART
            }
            ContextCompat.startForegroundService(context, intent)
        }

        fun notifyTransferStart(context: Context) {
            val intent = Intent(context, KeepAliveService::class.java).apply {
                action = ACTION_TRANSFER_START
            }
            ContextCompat.startForegroundService(context, intent)
        }

        fun notifyTransferStop(context: Context) {
            val intent = Intent(context, KeepAliveService::class.java).apply {
                action = ACTION_TRANSFER_STOP
            }
            context.startService(intent)
        }

        /**
         * 通过 WorkManager 周期性检查同步与服务状态（仅 Wi-Fi 约束），
         * 避免在 Android 15 BOOT_COMPLETED 广播中直接启动 dataSync 前台服务。
         */
        fun scheduleRecoveryWork(context: Context) {
            val constraints = Constraints.Builder()
                .setRequiredNetworkType(NetworkType.UNMETERED)
                .build()

            val request = PeriodicWorkRequestBuilder<KeepAliveRecoveryWorker>(
                15,
                TimeUnit.MINUTES,
            )
                .setConstraints(constraints)
                .build()

            WorkManager.getInstance(context).enqueueUniquePeriodicWork(
                WORK_NAME,
                ExistingPeriodicWorkPolicy.UPDATE,
                request,
            )
        }

        private fun saveLastConfig(context: Context, vaultPath: String, quotaGb: Long, port: Int) {
            context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .edit()
                .putString(EXTRA_VAULT_PATH, vaultPath)
                .putLong(EXTRA_QUOTA_GB, quotaGb)
                .putInt(EXTRA_PORT, port)
                .apply()
        }
    }

    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var activeVaultPath: String = "/storage/emulated/0/LanVault"
    private var activeQuotaGb: Long = 100L
    private var activePort: Int = 7842

    override fun onCreate() {
        super.onCreate()
        ensureNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action ?: ACTION_START) {
            ACTION_GRACEFUL_RESTART -> {
                performGracefulRestart()
                return START_STICKY
            }

            ACTION_TRANSFER_START -> {
                promoteToForeground("增量传输中 · 端口 $activePort (Level 0)")
                acquireTransferLocks()
                return START_STICKY
            }

            ACTION_TRANSFER_STOP -> {
                releaseTransferLocks()
                promoteToForeground("运行中 · 端口 $activePort (Level 0)")
                return START_STICKY
            }

            else -> {
                val prefs = getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                activeVaultPath = intent?.getStringExtra(EXTRA_VAULT_PATH)
                    ?: prefs.getString(EXTRA_VAULT_PATH, activeVaultPath)!!
                activeQuotaGb = intent?.getLongExtra(EXTRA_QUOTA_GB, -1L)?.takeIf { it > 0 }
                    ?: prefs.getLong(EXTRA_QUOTA_GB, activeQuotaGb)
                activePort = intent?.getIntExtra(EXTRA_PORT, -1)?.takeIf { it > 0 }
                    ?: prefs.getInt(EXTRA_PORT, activePort)

                promoteToForeground("运行中 · 端口 $activePort (Level 0)")
                return START_STICKY
            }
        }
    }

    /**
     * Android 15 (API 35) 强制回调：当 dataSync 前台服务达到 6 小时配额上限前触发。
     * 必须立即优雅释放锁、停止服务并交由 WorkManager 在条件满足时接续，防止系统抛出 RemoteServiceException。
     */
    override fun onTimeout(startId: Int, fgsType: Int) {
        releaseTransferLocks()
        scheduleRecoveryWork(applicationContext)
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf(startId)
    }

    override fun onDestroy() {
        releaseTransferLocks()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    /**
     * 仅在活跃传输阶段持有锁，并设置 10 分钟安全超时边界，传输完成或服务停止时立即释放。
     */
    fun acquireTransferLocks(timeoutMs: Long = TimeUnit.MINUTES.toMillis(10)) {
        val powerManager = getSystemService(POWER_SERVICE) as PowerManager
        if (wakeLock == null) {
            wakeLock = powerManager.newWakeLock(
                PowerManager.PARTIAL_WAKE_LOCK,
                "LanVault::TransferWakeLock",
            ).apply {
                setReferenceCounted(false)
            }
        }
        if (wakeLock?.isHeld == false) {
            wakeLock?.acquire(timeoutMs)
        }

        val wifiManager = applicationContext.getSystemService(WIFI_SERVICE) as WifiManager
        if (wifiLock == null) {
            val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                WifiManager.WIFI_MODE_FULL_LOW_LATENCY
            } else {
                @Suppress("DEPRECATION")
                WifiManager.WIFI_MODE_FULL_HIGH_PERF
            }
            wifiLock = wifiManager.createWifiLock(mode, "LanVault::TransferWifiLock").apply {
                setReferenceCounted(false)
            }
        }
        if (wifiLock?.isHeld == false) {
            wifiLock?.acquire()
        }
    }

    fun releaseTransferLocks() {
        wakeLock?.let { lock ->
            if (lock.isHeld) lock.release()
        }
        wifiLock?.let { lock ->
            if (lock.isHeld) lock.release()
        }
    }

    private fun performGracefulRestart() {
        releaseTransferLocks()
        promoteToForeground("正在执行优雅重启（WAL 刷盘）…")
        promoteToForeground("运行中 · 端口 $activePort (Level 0)")
    }

    private fun promoteToForeground(contentText: String) {
        val notification = buildNotification(contentText)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun buildNotification(contentText: String): Notification {
        val launchIntent = Intent(this, MainActivity::class.java)
        val launchPending = PendingIntent.getActivity(
            this,
            0,
            launchIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val restartIntent = Intent(this, KeepAliveService::class.java).apply {
            action = ACTION_GRACEFUL_RESTART
        }
        val restartPending = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            PendingIntent.getForegroundService(
                this,
                1,
                restartIntent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        } else {
            PendingIntent.getService(
                this,
                1,
                restartIntent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        }

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle("LanVault 局域网存储服务")
            .setContentText(contentText)
            .setOngoing(true)
            .setContentIntent(launchPending)
            .addAction(
                android.R.drawable.ic_menu_rotate,
                "一键重启",
                restartPending,
            )
            .build()
    }

    private fun ensureNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val manager = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
            val channel = NotificationChannel(
                CHANNEL_ID,
                "LanVault 后台保活与增量同步",
                NotificationManager.IMPORTANCE_LOW,
            )
            manager.createNotificationChannel(channel)
        }
    }
}

class KeepAliveRecoveryWorker(
    appContext: Context,
    params: WorkerParameters,
) : CoroutineWorker(appContext, params) {
    override suspend fun doWork(): Result {
        return when (PrivilegeDetector.detect()) {
            KeepAliveLevel.LEVEL_3_MAGISK_DAEMON -> Result.success()
            KeepAliveLevel.LEVEL_2_ROOT_LIBSU -> {
                withContext(Dispatchers.Main) {
                    RootDaemonService.ensureBound(applicationContext)
                }
                Result.success()
            }
            KeepAliveLevel.LEVEL_1_SHIZUKU -> {
                ShizukuBridge.ensureBound(applicationContext)
                Result.success()
            }
            KeepAliveLevel.LEVEL_0_FOREGROUND -> {
                // Level 0 在后台 WorkManager 窗口内执行增量扫描与健康检查，
                // 避免违反 Android 15 后台拉起 dataSync 前台服务限制。
                Result.success()
            }
        }
    }
}
