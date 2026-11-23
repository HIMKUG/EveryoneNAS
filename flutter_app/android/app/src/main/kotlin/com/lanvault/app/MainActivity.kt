// 参考依据:
// - ARCHITECTURE_CONTRACTS.md 第 5 节 (com.lanvault.app/keep_alive MethodChannel 5 个方法)
// - 严格遵循无过度防御原则：仅在 MethodChannel 入参边界校验必填参数，异常直接通过 result.error 暴露给上层。

package com.lanvault.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.BatteryManager
import android.provider.Settings
import com.lanvault.app.privilege.KeepAliveLevel
import com.lanvault.app.privilege.KeepAliveService
import com.lanvault.app.privilege.PrivilegeDetector
import com.lanvault.app.privilege.RootDaemonService
import com.lanvault.app.privilege.ShizukuBridge
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import androidx.core.content.ContextCompat
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

class MainActivity : FlutterActivity() {
    companion object {
        const val KEEP_ALIVE_CHANNEL = "com.lanvault.app/keep_alive"
        private const val WIFI_SSID_PERMISSION_REQUEST = 7843
    }

    private val backgroundExecutor: ExecutorService = Executors.newSingleThreadExecutor()
    private val mainHandler = Handler(Looper.getMainLooper())

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            KEEP_ALIVE_CHANNEL,
        ).setMethodCallHandler(::handleKeepAliveCall)
    }

    override fun onDestroy() {
        backgroundExecutor.shutdown()
        super.onDestroy()
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

                runInBackground(result) {
                    startServerByPrivilegeLevel(vaultPath, quotaGb, port, webPassword)
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
                    result.success(requestBatteryOptimizationExemption())
                } catch (t: Throwable) {
                    result.error("KEEP_ALIVE_ERROR", t.message ?: t.javaClass.simpleName, null)
                }
            }

            "requestStoragePermission" -> {
                try {
                    result.success(requestAllFilesAccessPermission())
                } catch (t: Throwable) {
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
                    if (
                        onWifi &&
                        ContextCompat.checkSelfPermission(
                            this,
                            Manifest.permission.ACCESS_FINE_LOCATION,
                        ) != PackageManager.PERMISSION_GRANTED
                    ) {
                        requestPermissions(
                            arrayOf(Manifest.permission.ACCESS_FINE_LOCATION),
                            WIFI_SSID_PERMISSION_REQUEST,
                        )
                    }
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
    ): Boolean {
        KeepAliveService.rememberConfig(
            applicationContext,
            vaultPath,
            quotaGb,
            port,
            webPassword,
        )
        return when (PrivilegeDetector.detect()) {
            KeepAliveLevel.LEVEL_3_MAGISK_DAEMON -> true
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
                KeepAliveService.start(
                    applicationContext,
                    vaultPath,
                    quotaGb,
                    port,
                    webPassword,
                )
                true
            }
        }
    }

    private fun restartServerByPrivilegeLevel(): Boolean {
        return when (PrivilegeDetector.detect()) {
            KeepAliveLevel.LEVEL_3_MAGISK_DAEMON -> PrivilegeDetector.signalMagiskDaemonRestart()
            KeepAliveLevel.LEVEL_2_ROOT_LIBSU -> RootDaemonService.gracefulRestart(applicationContext)
            KeepAliveLevel.LEVEL_1_SHIZUKU -> ShizukuBridge.gracefulRestart()
            KeepAliveLevel.LEVEL_0_FOREGROUND -> {
                KeepAliveService.requestGracefulRestart(applicationContext)
                true
            }
        }
    }

    private fun stopServerByPrivilegeLevel(): Boolean {
        return when (PrivilegeDetector.detect()) {
            KeepAliveLevel.LEVEL_3_MAGISK_DAEMON -> false
            KeepAliveLevel.LEVEL_2_ROOT_LIBSU -> {
                RootDaemonService.stopServer(applicationContext)
            }
            KeepAliveLevel.LEVEL_1_SHIZUKU -> {
                ShizukuBridge.stopServer()
            }
            KeepAliveLevel.LEVEL_0_FOREGROUND -> {
                val stopped = KeepAliveService.nativeGracefulRestartKeepAliveServer()
                applicationContext.stopService(Intent(applicationContext, KeepAliveService::class.java))
                stopped
            }
        }
    }

    private fun requestBatteryOptimizationExemption(): Boolean {
        val powerManager = getSystemService(POWER_SERVICE) as PowerManager
        if (powerManager.isIgnoringBatteryOptimizations(packageName)) {
            return true
        }
        val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
            data = Uri.parse("package:$packageName")
        }
        startActivity(intent)
        return false
    }

    private fun requestAllFilesAccessPermission(): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            return true
        }
        if (Environment.isExternalStorageManager()) {
            return true
        }
        val intent = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION).apply {
            data = Uri.parse("package:$packageName")
        }
        startActivity(intent)
        return false
    }
}
