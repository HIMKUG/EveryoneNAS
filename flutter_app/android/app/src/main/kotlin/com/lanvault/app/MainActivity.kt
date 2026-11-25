// 参考依据:
// - ARCHITECTURE_CONTRACTS.md 第 5 节 (com.lanvault.app/keep_alive MethodChannel 契约)
// - 严格遵循无过度防御原则：仅在 MethodChannel 入参边界校验必填参数，异常直接通过 result.error 暴露给上层。

package com.lanvault.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.BatteryManager
import android.os.Build
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import java.io.File
import java.io.FileOutputStream
import android.provider.Settings
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import com.lanvault.app.privilege.KeepAliveLevel
import com.lanvault.app.privilege.KeepAliveService
import com.lanvault.app.privilege.PrivilegeDetector
import com.lanvault.app.privilege.RootDaemonService
import com.lanvault.app.privilege.ShizukuBridge
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

class MainActivity : FlutterActivity() {
    companion object {
        const val KEEP_ALIVE_CHANNEL = "com.lanvault.app/keep_alive"
        private const val LOCATION_PERMISSION_REQUEST = 7843
        private const val NOTIFICATION_PERMISSION_REQUEST = 7844
        private const val PICK_DIRECTORY_REQUEST = 7845
        private const val PICK_FILE_REQUEST = 7846
        private const val BATTERY_EXEMPTION_REQUEST = 7847
        private const val ALL_FILES_ACCESS_REQUEST = 7848
        private const val PERM_PREFS_NAME = "lanvault_permission_prefs"
        private const val KEY_LOCATION_DENIED_FOREVER = "location_denied_forever"
        private const val KEY_NOTIFICATION_DENIED_FOREVER = "notification_denied_forever"
    }

    private val backgroundExecutor: ExecutorService = Executors.newCachedThreadPool()
    private val mainHandler = Handler(Looper.getMainLooper())
    private var pendingDirectoryResult: MethodChannel.Result? = null
    private var pendingFileResult: MethodChannel.Result? = null
    private var pendingBatteryResult: MethodChannel.Result? = null
    private var pendingStorageResult: MethodChannel.Result? = null
    private var pendingLocationResult: MethodChannel.Result? = null
    private var pendingNotificationResult: MethodChannel.Result? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            KEEP_ALIVE_CHANNEL,
        ).setMethodCallHandler(::handleKeepAliveCall)
    }

    override fun onDestroy() {
        pendingDirectoryResult?.success(null)
        pendingDirectoryResult = null
        pendingFileResult?.success(null)
        pendingFileResult = null
        pendingBatteryResult?.success(false)
        pendingBatteryResult = null
        pendingStorageResult?.success(false)
        pendingStorageResult = null
        pendingLocationResult?.success(false)
        pendingLocationResult = null
        pendingNotificationResult?.success(false)
        pendingNotificationResult = null
        backgroundExecutor.shutdown()
        super.onDestroy()
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        if (requestCode == LOCATION_PERMISSION_REQUEST) {
            val pending = pendingLocationResult
            pendingLocationResult = null
            val granted = isLocationGranted()
            setPermissionDeniedForever(
                KEY_LOCATION_DENIED_FOREVER,
                !granted && !ActivityCompat.shouldShowRequestPermissionRationale(
                    this,
                    Manifest.permission.ACCESS_FINE_LOCATION,
                ),
            )
            pending?.success(granted)
            return
        }
        if (requestCode == NOTIFICATION_PERMISSION_REQUEST) {
            val pending = pendingNotificationResult
            pendingNotificationResult = null
            val granted = isNotificationGranted()
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                setPermissionDeniedForever(
                    KEY_NOTIFICATION_DENIED_FOREVER,
                    !granted && !ActivityCompat.shouldShowRequestPermissionRationale(
                        this,
                        Manifest.permission.POST_NOTIFICATIONS,
                    ),
                )
            }
            pending?.success(granted)
            return
        }
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
    }

    @Suppress("DEPRECATION")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode == BATTERY_EXEMPTION_REQUEST) {
            val pending = pendingBatteryResult
            pendingBatteryResult = null
            pending?.success(isBatteryExempted())
            return
        }
        if (requestCode == ALL_FILES_ACCESS_REQUEST) {
            val pending = pendingStorageResult
            pendingStorageResult = null
            pending?.success(isAllFilesGranted())
            return
        }
        if (requestCode == PICK_DIRECTORY_REQUEST) {
            val pending = pendingDirectoryResult
            pendingDirectoryResult = null
            if (pending == null) {
                return
            }
            if (resultCode != RESULT_OK || data?.data == null) {
                pending.success(null)
                return
            }
            val treeUri = data.data!!
            try {
                contentResolver.takePersistableUriPermission(
                    treeUri,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
                )
            } catch (_: SecurityException) {
                // 部分外部文档提供方不支持持久化授权，忽略并继续解析路径
            }
            pending.success(resolveTreeUriToPath(treeUri))
            return
        }
        if (requestCode == PICK_FILE_REQUEST) {
            val pending = pendingFileResult
            pendingFileResult = null
            if (pending == null) {
                return
            }
            if (resultCode != RESULT_OK || data?.data == null) {
                pending.success(null)
                return
            }
            val fileUri = data.data!!
            runInBackground(pending) {
                copyPickedUriToCachePath(fileUri)
            }
            return
        }
        super.onActivityResult(requestCode, resultCode, data)
    }

    private fun handleKeepAliveCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "detectKeepAliveLevel" -> {
                runInBackground(result) {
                    PrivilegeDetector.detect().wireValue
                }
            }

            "startKeepAliveServer" -> {
                val vaultPath = call.argument<String>("vaultPath")
                    ?: return result.error("INVALID_ARGS", "vaultPath is required", null)
                val quotaGb = call.argument<Number>("quotaGb")?.toLong()
                    ?: return result.error("INVALID_ARGS", "quotaGb is required", null)
                val port = call.argument<Number>("port")?.toInt()
                    ?: return result.error("INVALID_ARGS", "port is required", null)
                val webPassword = call.argument<String>("webPassword")
                    ?: return result.error("INVALID_ARGS", "webPassword is required", null)
                val skipNativeStart = call.argument<Boolean>("skipNativeStart") ?: false

                runInBackground(result) {
                    startServerByPrivilegeLevel(
                        vaultPath,
                        quotaGb,
                        port,
                        webPassword,
                        skipNativeStart,
                    )
                }
            }

            "gracefulRestartServer" -> {
                runInBackground(result) {
                    restartServerByPrivilegeLevel()
                }
            }

            "stopKeepAliveServer" -> {
                runInBackground(result) {
                    stopServerByPrivilegeLevel()
                }
            }

            "requestBatteryExemption" -> {
                try {
                    requestBatteryOptimizationExemption(result)
                } catch (t: Throwable) {
                    pendingBatteryResult = null
                    result.error("KEEP_ALIVE_ERROR", t.message ?: t.javaClass.simpleName, null)
                }
            }

            "requestStoragePermission" -> {
                try {
                    requestAllFilesAccessPermission(result)
                } catch (t: Throwable) {
                    pendingStorageResult = null
                    result.error("KEEP_ALIVE_ERROR", t.message ?: t.javaClass.simpleName, null)
                }
            }

            "requestLocationPermission" -> {
                try {
                    requestLocationPermissions(result)
                } catch (t: Throwable) {
                    pendingLocationResult = null
                    result.error("KEEP_ALIVE_ERROR", t.message ?: t.javaClass.simpleName, null)
                }
            }

            "requestNotificationPermission" -> {
                try {
                    requestPostNotificationsPermission(result)
                } catch (t: Throwable) {
                    pendingNotificationResult = null
                    result.error("KEEP_ALIVE_ERROR", t.message ?: t.javaClass.simpleName, null)
                }
            }

            "getPermissionStates" -> {
                try {
                    val locationGranted = isLocationGranted()
                    val notificationGranted = isNotificationGranted()
                    result.success(
                        mapOf(
                            "battery" to isBatteryExempted(),
                            "storage" to isAllFilesGranted(),
                            "location" to locationGranted,
                            "notification" to notificationGranted,
                            "locationDeniedForever" to (!locationGranted && isPermissionDeniedForever(KEY_LOCATION_DENIED_FOREVER)),
                            "notificationDeniedForever" to (!notificationGranted && isPermissionDeniedForever(KEY_NOTIFICATION_DENIED_FOREVER)),
                        ),
                    )
                } catch (t: Throwable) {
                    result.error("KEEP_ALIVE_ERROR", t.message ?: t.javaClass.simpleName, null)
                }
            }

            "openAppSettings" -> {
                try {
                    val intent = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                        data = Uri.parse("package:$packageName")
                    }
                    startActivity(intent)
                    result.success(null)
                } catch (t: Throwable) {
                    result.error("KEEP_ALIVE_ERROR", t.message ?: t.javaClass.simpleName, null)
                }
            }

            "pickDirectory" -> {
                try {
                    launchDirectoryPicker(result)
                } catch (t: Throwable) {
                    pendingDirectoryResult = null
                    result.error("KEEP_ALIVE_ERROR", t.message ?: t.javaClass.simpleName, null)
                }
            }

            "pickFile" -> {
                try {
                    launchFilePicker(result)
                } catch (t: Throwable) {
                    pendingFileResult = null
                    result.error("KEEP_ALIVE_ERROR", t.message ?: t.javaClass.simpleName, null)
                }
            }

            "getLastServerConfig" -> {
                val config = KeepAliveService.loadLastConfig(applicationContext)
                if (config == null) {
                    result.success(null)
                } else {
                    result.success(
                        mapOf(
                            "vaultPath" to config.vaultPath,
                            "quotaGb" to config.quotaGb,
                            "port" to config.port,
                            "webPassword" to config.webPassword,
                        ),
                    )
                }
            }

            "getDeviceEnvironment" -> {
                try {
                    val connectivity =
                        getSystemService(CONNECTIVITY_SERVICE) as ConnectivityManager
                    val capabilities =
                        connectivity.getNetworkCapabilities(connectivity.activeNetwork)
                    val onWifi =
                        capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true
                    val wifiManager =
                        applicationContext.getSystemService(WIFI_SERVICE) as WifiManager
                    @Suppress("DEPRECATION")
                    val rawSsid = wifiManager.connectionInfo.ssid
                    val currentSsid = rawSsid
                        ?.trim('"')
                        ?.takeUnless { it.isBlank() || it.contains("unknown", ignoreCase = true) }
                    val battery =
                        getSystemService(BATTERY_SERVICE) as BatteryManager
                    val isCharging = battery.isCharging
                    result.success(
                        mapOf(
                            "onWifi" to onWifi,
                            "currentSsid" to currentSsid,
                            "isCharging" to isCharging,
                        ),
                    )
                } catch (t: Throwable) {
                    result.error("KEEP_ALIVE_ERROR", t.message ?: t.javaClass.simpleName, null)
                }
            }

            else -> result.notImplemented()
        }
    }

    private fun <T> runInBackground(result: MethodChannel.Result, block: () -> T) {
        backgroundExecutor.execute {
            try {
                val value = block()
                mainHandler.post { result.success(value) }
            } catch (t: Throwable) {
                mainHandler.post {
                    result.error("KEEP_ALIVE_ERROR", t.message ?: t.javaClass.simpleName, null)
                }
            }
        }
    }

    private fun startServerByPrivilegeLevel(
        vaultPath: String,
        quotaGb: Long,
        port: Int,
        webPassword: String,
        skipNativeStart: Boolean,
    ): Boolean {
        KeepAliveService.rememberConfig(
            applicationContext,
            vaultPath,
            quotaGb,
            port,
            webPassword,
        )
        return when (PrivilegeDetector.detect()) {
            KeepAliveLevel.LEVEL_3_MAGISK_DAEMON -> PrivilegeDetector.startMagiskDaemon()
            KeepAliveLevel.LEVEL_2_ROOT_LIBSU -> {
                RootDaemonService.bindAndStart(
                    applicationContext,
                    vaultPath,
                    quotaGb,
                    port,
                    webPassword,
                )
            }
            KeepAliveLevel.LEVEL_1_SHIZUKU -> {
                ShizukuBridge.bindAndStart(
                    applicationContext,
                    vaultPath,
                    quotaGb,
                    port,
                    webPassword,
                )
            }
            KeepAliveLevel.LEVEL_0_FOREGROUND -> {
                KeepAliveService.startAndAwait(
                    applicationContext,
                    vaultPath,
                    quotaGb,
                    port,
                    webPassword,
                    skipNativeStart,
                )
            }
        }
    }

    private fun restartServerByPrivilegeLevel(): Boolean {
        return when (PrivilegeDetector.detect()) {
            KeepAliveLevel.LEVEL_3_MAGISK_DAEMON -> PrivilegeDetector.signalMagiskDaemonRestart()
            KeepAliveLevel.LEVEL_2_ROOT_LIBSU -> RootDaemonService.gracefulRestart(applicationContext)
            KeepAliveLevel.LEVEL_1_SHIZUKU -> ShizukuBridge.gracefulRestart(applicationContext)
            KeepAliveLevel.LEVEL_0_FOREGROUND -> {
                KeepAliveService.requestGracefulRestart(applicationContext)
            }
        }
    }

    private fun stopServerByPrivilegeLevel(): Boolean {
        KeepAliveService.cancelRecovery(applicationContext)
        return when (PrivilegeDetector.detect()) {
            KeepAliveLevel.LEVEL_3_MAGISK_DAEMON -> PrivilegeDetector.stopMagiskDaemon()
            KeepAliveLevel.LEVEL_2_ROOT_LIBSU -> {
                RootDaemonService.stopServer(applicationContext)
            }
            KeepAliveLevel.LEVEL_1_SHIZUKU -> {
                ShizukuBridge.stopServer(applicationContext)
            }
            KeepAliveLevel.LEVEL_0_FOREGROUND -> {
                val stopped = KeepAliveService.nativeGracefulRestartKeepAliveServer()
                applicationContext.stopService(Intent(applicationContext, KeepAliveService::class.java))
                stopped
            }
        }
    }

    private fun isBatteryExempted(): Boolean {
        val powerManager = getSystemService(POWER_SERVICE) as PowerManager
        return powerManager.isIgnoringBatteryOptimizations(packageName)
    }

    private fun isAllFilesGranted(): Boolean {
        return Build.VERSION.SDK_INT < Build.VERSION_CODES.R ||
            Environment.isExternalStorageManager()
    }

    private fun isLocationGranted(): Boolean {
        return ContextCompat.checkSelfPermission(
            this,
            Manifest.permission.ACCESS_FINE_LOCATION,
        ) == PackageManager.PERMISSION_GRANTED
    }

    private fun isNotificationGranted(): Boolean {
        return Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            ContextCompat.checkSelfPermission(
                this,
                Manifest.permission.POST_NOTIFICATIONS,
            ) == PackageManager.PERMISSION_GRANTED
    }

    private fun isPermissionDeniedForever(key: String): Boolean {
        return getSharedPreferences(PERM_PREFS_NAME, MODE_PRIVATE)
            .getBoolean(key, false)
    }

    private fun setPermissionDeniedForever(key: String, deniedForever: Boolean) {
        getSharedPreferences(PERM_PREFS_NAME, MODE_PRIVATE)
            .edit()
            .putBoolean(key, deniedForever)
            .apply()
    }

    @Suppress("DEPRECATION")
    private fun requestBatteryOptimizationExemption(result: MethodChannel.Result) {
        if (isBatteryExempted()) {
            result.success(true)
            return
        }
        pendingBatteryResult?.success(false)
        pendingBatteryResult = result
        val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
            data = Uri.parse("package:$packageName")
        }
        startActivityForResult(intent, BATTERY_EXEMPTION_REQUEST)
    }

    @Suppress("DEPRECATION")
    private fun requestAllFilesAccessPermission(result: MethodChannel.Result) {
        if (isAllFilesGranted()) {
            result.success(true)
            return
        }
        pendingStorageResult?.success(false)
        pendingStorageResult = result
        val intent = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION).apply {
            data = Uri.parse("package:$packageName")
        }
        startActivityForResult(intent, ALL_FILES_ACCESS_REQUEST)
    }

    private fun requestLocationPermissions(result: MethodChannel.Result) {
        if (isLocationGranted()) {
            setPermissionDeniedForever(KEY_LOCATION_DENIED_FOREVER, false)
            result.success(true)
            return
        }
        pendingLocationResult?.success(false)
        pendingLocationResult = result
        requestPermissions(
            arrayOf(
                Manifest.permission.ACCESS_FINE_LOCATION,
                Manifest.permission.ACCESS_COARSE_LOCATION,
            ),
            LOCATION_PERMISSION_REQUEST,
        )
    }

    private fun requestPostNotificationsPermission(result: MethodChannel.Result) {
        if (isNotificationGranted()) {
            setPermissionDeniedForever(KEY_NOTIFICATION_DENIED_FOREVER, false)
            result.success(true)
            return
        }
        pendingNotificationResult?.success(false)
        pendingNotificationResult = result
        requestPermissions(
            arrayOf(Manifest.permission.POST_NOTIFICATIONS),
            NOTIFICATION_PERMISSION_REQUEST,
        )
    }

    @Suppress("DEPRECATION")
    private fun launchDirectoryPicker(result: MethodChannel.Result) {
        pendingDirectoryResult?.success(null)
        pendingDirectoryResult = result
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
            addFlags(
                Intent.FLAG_GRANT_READ_URI_PERMISSION or
                    Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
                    Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION,
            )
        }
        startActivityForResult(intent, PICK_DIRECTORY_REQUEST)
    }

    private fun resolveTreeUriToPath(treeUri: Uri): String? {
        val docId = try {
            DocumentsContract.getTreeDocumentId(treeUri)
        } catch (_: IllegalArgumentException) {
            return null
        }
        if (docId.isBlank()) {
            return null
        }
        if (docId.startsWith("raw:")) {
            return docId.removePrefix("raw:").takeIf { it.isNotBlank() }
        }
        val colonIndex = docId.indexOf(':')
        val volume = if (colonIndex >= 0) docId.substring(0, colonIndex) else docId
        val relative = if (colonIndex >= 0) {
            docId.substring(colonIndex + 1).trimStart('/')
        } else {
            ""
        }
        val primaryRoot = Environment.getExternalStorageDirectory().absolutePath.trimEnd('/')
        val basePath = when {
            volume.equals("primary", ignoreCase = true) -> primaryRoot
            volume.equals("home", ignoreCase = true) -> "$primaryRoot/Documents"
            volume.isNotBlank() -> "/storage/$volume"
            else -> return null
        }
        return if (relative.isEmpty()) basePath else "$basePath/$relative"
    }

    @Suppress("DEPRECATION")
    private fun launchFilePicker(result: MethodChannel.Result) {
        pendingFileResult?.success(null)
        pendingFileResult = result
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = "*/*"
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        startActivityForResult(intent, PICK_FILE_REQUEST)
    }

    private fun copyPickedUriToCachePath(uri: Uri): String? {
        var displayName = "transfer_${System.currentTimeMillis()}.bin"
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) {
                val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (idx >= 0) {
                    val raw = cursor.getString(idx)
                    if (!raw.isNullOrBlank()) {
                        displayName = raw.replace(Regex("""[\\/:*?"<>|]"""), "_")
                    }
                }
            }
        }
        val cacheFolder = File(cacheDir, "picked_files")
        if (!cacheFolder.exists()) {
            cacheFolder.mkdirs()
        }
        val destFile = File(cacheFolder, displayName)
        contentResolver.openInputStream(uri)?.use { input ->
            FileOutputStream(destFile).use { output ->
                input.copyTo(output)
            }
        } ?: return null
        return destFile.absolutePath
    }
}
