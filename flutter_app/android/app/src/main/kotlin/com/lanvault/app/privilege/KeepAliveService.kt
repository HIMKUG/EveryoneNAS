// 参考依据:
// - lanvault_complete_blueprint.md 第 4 节 (Android 15 API 35 dataSync 6 小时超时 onTimeout 优雅停机 + WorkManager + WifiLock/PARTIAL_WAKE_LOCK 按需获取与即时释放)
// - ARCHITECTURE_CONTRACTS.md 第 5 节 (Level 0 前台服务与通知栏“一键重启” Action)

package com.lanvault.app.privilege

import android.app.ForegroundServiceStartNotAllowedException
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
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.util.Log
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
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong

/**
 * Level 0 标准非特权保活前台服务：
 * - 遵守 Android 15 (API 35) dataSync 24h 内 6h 限额：重写 onTimeout() 主动优雅停机，避免 RemoteServiceException 崩溃。
 * - 仅在显式收到 ACTION_TRANSFER_START 时按需获取 WifiLock + PARTIAL_WAKE_LOCK，收到 ACTION_TRANSFER_STOP 或 onDestroy 时立即释放。
 * - 提供通知栏“一键重启” Action（API 26+ 使用 PendingIntent.getForegroundService），并配合 WorkManager 低功耗周期恢复。
 */
class KeepAliveService : Service() {
    companion object {
        private const val TAG = "LanVaultKeepAlive"
        const val CHANNEL_ID = "lanvault_keep_alive"
        const val NOTIFICATION_ID = 7842
        const val ACTION_START = "com.lanvault.app.action.START_KEEP_ALIVE"
        const val ACTION_GRACEFUL_RESTART = "com.lanvault.app.action.GRACEFUL_RESTART"
        const val ACTION_TRANSFER_START = "com.lanvault.app.action.TRANSFER_START"
        const val ACTION_TRANSFER_STOP = "com.lanvault.app.action.TRANSFER_STOP"
        const val EXTRA_VAULT_PATH = "vault_path"
        const val EXTRA_QUOTA_GB = "quota_gb"
        const val EXTRA_PORT = "port"
        const val EXTRA_WEB_PASSWORD = "web_password"
        const val EXTRA_SKIP_NATIVE_START = "skip_native_start"

        private const val WORK_NAME = "lanvault_level0_periodic_recovery"
        private const val PREFS_NAME = "lanvault_keep_alive_prefs"
        private const val KEY_USER_STOPPED = "user_stopped"
        private const val EXTRA_START_TOKEN = "start_token"
        private const val START_AWAIT_TIMEOUT_SECS = 10L
        private val nextStartToken = AtomicLong(1L)
        private val pendingStartCallbacks = ConcurrentHashMap<Long, CompletableFuture<Boolean>>()

        data class LastConfig(
            val vaultPath: String,
            val quotaGb: Long,
            val port: Int,
            val webPassword: String,
        )

        init {
            System.loadLibrary("lanvault_ffi")
        }

        @JvmStatic
        external fun nativeStartKeepAliveServer(
            vaultPath: String,
            quotaGb: Long,
            port: Int,
            webPassword: String,
        ): Boolean

        @JvmStatic
        external fun nativeGracefulRestartKeepAliveServer(): Boolean

        fun start(
            context: Context,
            vaultPath: String,
            quotaGb: Long,
            port: Int,
            webPassword: String,
            skipNativeStart: Boolean = false,
        ) {
            startInternal(
                context,
                vaultPath,
                quotaGb,
                port,
                webPassword,
                skipNativeStart,
                token = 0L,
            )
        }

        fun startAndAwait(
            context: Context,
            vaultPath: String,
            quotaGb: Long,
            port: Int,
            webPassword: String,
            skipNativeStart: Boolean = false,
        ): Boolean {
            val token = nextStartToken.getAndIncrement()
            val future = CompletableFuture<Boolean>()
            pendingStartCallbacks[token] = future
            val launched = startInternal(
                context,
                vaultPath,
                quotaGb,
                port,
                webPassword,
                skipNativeStart,
                token = token,
            )
            if (!launched) {
                pendingStartCallbacks.remove(token)
                return false
            }
            return try {
                future.get(START_AWAIT_TIMEOUT_SECS, TimeUnit.SECONDS)
            } catch (t: Throwable) {
                pendingStartCallbacks.remove(token)
                Log.w(TAG, "Timed out or interrupted waiting for KeepAliveService start: ${t.message}")
                false
            }
        }

        private fun startInternal(
            context: Context,
            vaultPath: String,
            quotaGb: Long,
            port: Int,
            webPassword: String,
            skipNativeStart: Boolean,
            token: Long,
        ): Boolean {
            setUserStopped(context, false)
            saveLastConfig(context, vaultPath, quotaGb, port, webPassword)
            val intent = Intent(context, KeepAliveService::class.java).apply {
                action = ACTION_START
                putExtra(EXTRA_VAULT_PATH, vaultPath)
                putExtra(EXTRA_QUOTA_GB, quotaGb)
                putExtra(EXTRA_PORT, port)
                putExtra(EXTRA_WEB_PASSWORD, webPassword)
                putExtra(EXTRA_SKIP_NATIVE_START, skipNativeStart)
                if (token > 0L) {
                    putExtra(EXTRA_START_TOKEN, token)
                }
            }
            return try {
                ContextCompat.startForegroundService(context, intent)
                scheduleRecoveryWork(context)
                true
            } catch (t: Throwable) {
                Log.w(TAG, "startForegroundService failed: ${t.message}")
                false
            }
        }

        fun rememberConfig(
            context: Context,
            vaultPath: String,
            quotaGb: Long,
            port: Int,
            webPassword: String,
        ) {
            setUserStopped(context, false)
            saveLastConfig(context, vaultPath, quotaGb, port, webPassword)
        }

        /**
         * 取消 WorkManager 周期恢复任务并写入 user_stopped 标记，防止用户停服或切 client_only 后被自动拉起。
         */
        fun cancelRecovery(context: Context) {
            setUserStopped(context, true)
            WorkManager.getInstance(context).cancelUniqueWork(WORK_NAME)
        }

        fun isUserStopped(context: Context): Boolean {
            return context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .getBoolean(KEY_USER_STOPPED, false)
        }

        private fun setUserStopped(context: Context, stopped: Boolean) {
            context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .edit()
                .putBoolean(KEY_USER_STOPPED, stopped)
                .apply()
        }

        fun requestGracefulRestart(context: Context): Boolean {
            val token = nextStartToken.getAndIncrement()
            val future = CompletableFuture<Boolean>()
            pendingStartCallbacks[token] = future
            val intent = Intent(context, KeepAliveService::class.java).apply {
                action = ACTION_GRACEFUL_RESTART
                putExtra(EXTRA_START_TOKEN, token)
            }
            val launched = try {
                ContextCompat.startForegroundService(context, intent)
                true
            } catch (t: Throwable) {
                Log.w(TAG, "requestGracefulRestart failed: ${t.message}")
                false
            }
            if (!launched) {
                pendingStartCallbacks.remove(token)
                return false
            }
            return try {
                future.get(START_AWAIT_TIMEOUT_SECS, TimeUnit.SECONDS)
            } catch (t: Throwable) {
                pendingStartCallbacks.remove(token)
                Log.w(TAG, "Timed out waiting for KeepAliveService restart: ${t.message}")
                false
            }
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
         * 通过 WorkManager 周期性检查同步与服务状态（要求任一可用网络连接），
         * 避免在 Android 15 BOOT_COMPLETED 广播中直接启动 dataSync 前台服务。
         */
        fun scheduleRecoveryWork(context: Context) {
            val constraints = Constraints.Builder()
                .setRequiredNetworkType(NetworkType.CONNECTED)
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

        private fun saveLastConfig(
            context: Context,
            vaultPath: String,
            quotaGb: Long,
            port: Int,
            webPassword: String,
        ) {
            context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .edit()
                .putString(EXTRA_VAULT_PATH, vaultPath)
                .putLong(EXTRA_QUOTA_GB, quotaGb)
                .putInt(EXTRA_PORT, port)
                .putString(EXTRA_WEB_PASSWORD, webPassword)
                .apply()
        }

        fun loadLastConfig(context: Context): LastConfig? {
            val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            val path = prefs.getString(EXTRA_VAULT_PATH, null) ?: return null
            val quotaGb = prefs.getLong(EXTRA_QUOTA_GB, 0L)
            val port = prefs.getInt(EXTRA_PORT, 0)
            val webPassword = prefs.getString(EXTRA_WEB_PASSWORD, null) ?: return null
            if (quotaGb <= 0L || port <= 0 || webPassword.length < 8) return null
            return LastConfig(path, quotaGb, port, webPassword)
        }
    }

    private val mainHandler = Handler(Looper.getMainLooper())
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var activeVaultPath: String = "/storage/emulated/0/LanVault"
    private var activeQuotaGb: Long = 100L
    private var activePort: Int = 7842
    private var activeWebPassword: String = ""

    override fun onCreate() {
        super.onCreate()
        ensureNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val startToken = intent?.getLongExtra(EXTRA_START_TOKEN, 0L) ?: 0L
        when (intent?.action ?: ACTION_START) {
            ACTION_GRACEFUL_RESTART -> {
                if (startToken > 0L && !pendingStartCallbacks.containsKey(startToken)) {
                    stopForegroundAndSelf(startId)
                    return START_NOT_STICKY
                }
                performGracefulRestart(startToken, startId)
                return START_STICKY
            }

            ACTION_TRANSFER_START -> {
                if (!promoteToForegroundSafely("增量传输中 · 端口 $activePort (Level 0)")) {
                    stopForegroundAndSelf(startId)
                    return START_NOT_STICKY
                }
                acquireTransferLocks()
                return START_STICKY
            }

            ACTION_TRANSFER_STOP -> {
                releaseTransferLocks()
                if (!promoteToForegroundSafely("运行中 · 端口 $activePort (Level 0)")) {
                    stopForegroundAndSelf(startId)
                    return START_NOT_STICKY
                }
                return START_STICKY
            }

            else -> {
                if (startToken > 0L && !pendingStartCallbacks.containsKey(startToken)) {
                    stopForegroundAndSelf(startId)
                    return START_NOT_STICKY
                }
                val prefs = getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                activeVaultPath = intent?.getStringExtra(EXTRA_VAULT_PATH)
                    ?: prefs.getString(EXTRA_VAULT_PATH, activeVaultPath)!!
                activeQuotaGb = intent?.getLongExtra(EXTRA_QUOTA_GB, -1L)?.takeIf { it > 0 }
                    ?: prefs.getLong(EXTRA_QUOTA_GB, activeQuotaGb)
                activePort = intent?.getIntExtra(EXTRA_PORT, -1)?.takeIf { it > 0 }
                    ?: prefs.getInt(EXTRA_PORT, activePort)
                activeWebPassword = intent?.getStringExtra(EXTRA_WEB_PASSWORD)
                    ?: prefs.getString(EXTRA_WEB_PASSWORD, activeWebPassword)!!
                val skipNativeStart = intent?.getBooleanExtra(EXTRA_SKIP_NATIVE_START, false) == true

                saveLastConfig(
                    this,
                    activeVaultPath,
                    activeQuotaGb,
                    activePort,
                    activeWebPassword,
                )
                if (!promoteToForegroundSafely("运行中 · 端口 $activePort (Level 0)")) {
                    completeStartToken(startToken, false)
                    stopForegroundAndSelf(startId)
                    return START_NOT_STICKY
                }
                if (skipNativeStart) {
                    completeStartToken(startToken, true)
                    return START_STICKY
                }
                val vaultPath = activeVaultPath
                val quotaGb = activeQuotaGb
                val port = activePort
                val webPassword = activeWebPassword
                Thread {
                    val started = try {
                        nativeStartKeepAliveServer(vaultPath, quotaGb, port, webPassword)
                    } catch (t: Throwable) {
                        Log.e(TAG, "nativeStartKeepAliveServer threw", t)
                        false
                    }
                    completeStartToken(startToken, started)
                    if (!started) {
                        mainHandler.post {
                            stopForegroundAndSelf(startId)
                        }
                    }
                }.start()
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

    private fun performGracefulRestart(startToken: Long = 0L, startId: Int? = null) {
        releaseTransferLocks()
        if (activeWebPassword.isBlank()) {
            val saved = loadLastConfig(this)
            if (saved == null) {
                Log.w(TAG, "Cannot graceful-restart Level 0 service: no persisted config found.")
                completeStartToken(startToken, false)
                stopForegroundAndSelf(startId)
                return
            }
            activeVaultPath = saved.vaultPath
            activeQuotaGb = saved.quotaGb
            activePort = saved.port
            activeWebPassword = saved.webPassword
        }
        if (!promoteToForegroundSafely("正在执行优雅重启（WAL 刷盘）…")) {
            completeStartToken(startToken, false)
            stopForegroundAndSelf(startId)
            return
        }
        val vaultPath = activeVaultPath
        val quotaGb = activeQuotaGb
        val port = activePort
        val webPassword = activeWebPassword
        Thread {
            val restarted = try {
                val stopped = nativeGracefulRestartKeepAliveServer()
                stopped && nativeStartKeepAliveServer(vaultPath, quotaGb, port, webPassword)
            } catch (t: Throwable) {
                Log.e(TAG, "Graceful restart native call threw", t)
                false
            }
            completeStartToken(startToken, restarted)
            mainHandler.post {
                if (restarted) {
                    promoteToForegroundSafely("运行中 · 端口 $port (Level 0)")
                } else {
                    stopForegroundAndSelf(startId)
                }
            }
        }.start()
    }

    private fun completeStartToken(token: Long, succeeded: Boolean) {
        if (token <= 0L) return
        pendingStartCallbacks.remove(token)?.complete(succeeded)
    }

    private fun promoteToForegroundSafely(contentText: String): Boolean {
        return try {
            promoteToForeground(contentText)
            true
        } catch (t: Throwable) {
            Log.e(TAG, "promoteToForeground failed: ${t.message}", t)
            false
        }
    }

    private fun stopForegroundAndSelf(startId: Int?) {
        releaseTransferLocks()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
        if (startId != null) {
            stopSelf(startId)
        } else {
            stopSelf()
        }
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
            .setContentTitle("EveryoneNAS 局域网存储服务")
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
                "EveryoneNAS 后台保活与增量同步",
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
    companion object {
        private const val TAG = "LanVaultRecoveryWorker"
    }

    override suspend fun doWork(): Result {
        if (KeepAliveService.isUserStopped(applicationContext)) {
            return Result.success()
        }
        return when (PrivilegeDetector.detect()) {
            KeepAliveLevel.LEVEL_3_MAGISK_DAEMON -> Result.success()
            KeepAliveLevel.LEVEL_2_ROOT_LIBSU -> {
                val config = KeepAliveService.loadLastConfig(applicationContext)
                if (config == null) {
                    Result.failure()
                } else {
                    val started = RootDaemonService.bindAndStart(
                        applicationContext,
                        config.vaultPath,
                        config.quotaGb,
                        config.port,
                        config.webPassword,
                    )
                    if (started) Result.success() else Result.retry()
                }
            }
            KeepAliveLevel.LEVEL_1_SHIZUKU -> {
                val config = KeepAliveService.loadLastConfig(applicationContext)
                if (config == null) {
                    Result.failure()
                } else {
                    val started = ShizukuBridge.bindAndStart(
                        applicationContext,
                        config.vaultPath,
                        config.quotaGb,
                        config.port,
                        config.webPassword,
                    )
                    if (started) Result.success() else Result.retry()
                }
            }
            KeepAliveLevel.LEVEL_0_FOREGROUND -> {
                val config = KeepAliveService.loadLastConfig(applicationContext)
                if (config == null) {
                    Result.failure()
                } else {
                    val started = KeepAliveService.startAndAwait(
                        applicationContext,
                        config.vaultPath,
                        config.quotaGb,
                        config.port,
                        config.webPassword,
                    )
                    if (started) Result.success() else Result.retry()
                }
            }
        }
    }
}
