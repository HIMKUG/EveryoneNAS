// 参考依据:
// - vendor_refs/android-shizuku-api/demo/src/main/java/rikka/shizuku/demo/service/UserService.java
//   (@Keep UserService(Context) 构造器、destroy() 退出进程、JNI 动态库加载)
// - lanvault_complete_blueprint.md 第 3.4 节 (UID 2000 独立进程 + cmd appops set <pkg> RUN_ANY_IN_BACKGROUND allow)

package com.lanvault.app.privilege

import android.content.Context
import android.os.Process
import android.system.Os
import androidx.annotation.Keep
import com.lanvault.app.IShizukuLanVaultService
import java.util.concurrent.TimeUnit

/**
 * Level 1 Shizuku UserService：运行在独立的 UID 2000 (shell) 进程中，
 * 脱离主应用进程生命周期，负责豁免后台限制并承载 Rust 核心服务。
 */
class ShizukuUserService : IShizukuLanVaultService.Stub {
    companion object {
        init {
            if (Os.getuid() == 2000 || Os.getuid() == 0) {
                System.loadLibrary("lanvault_ffi")
            }
        }

        @JvmStatic
        external fun nativeStartServer(
            vaultPath: String,
            quotaGb: Long,
            port: Int,
            webPassword: String,
        ): Boolean

        @JvmStatic
        external fun nativeGracefulRestart(): Boolean
    }

    @Suppress("unused")
    private val appContext: Context?

    private data class LaunchConfig(
        val vaultPath: String,
        val quotaGb: Long,
        val port: Int,
        val webPassword: String,
    )

    private var lastLaunch: LaunchConfig? = null

    @Keep
    constructor() {
        appContext = null
    }

    @Keep
    constructor(context: Context) {
        appContext = context
    }

    override fun destroy() {
        System.exit(0)
    }

    override fun getPid(): Int = Process.myPid()

    override fun getUid(): Int = Os.getuid()

    override fun startServer(
        vaultPath: String,
        quotaGb: Long,
        port: Int,
        webPassword: String,
    ): Boolean {
        val started = nativeStartServer(vaultPath, quotaGb, port, webPassword)
        if (started) {
            lastLaunch = LaunchConfig(vaultPath, quotaGb, port, webPassword)
        }
        return started
    }

    override fun gracefulRestart(): Boolean {
        val launch = lastLaunch ?: return false
        if (!nativeGracefulRestart()) {
            return false
        }
        val restarted = nativeStartServer(
            launch.vaultPath,
            launch.quotaGb,
            launch.port,
            launch.webPassword,
        )
        if (restarted) {
            lastLaunch = launch
        }
        return restarted
    }

    override fun stopServer(): Boolean {
        val stopped = nativeGracefulRestart()
        if (stopped) {
            lastLaunch = null
        }
        return stopped
    }

    override fun allowBackgroundAppOps(packageName: String): Boolean {
        require(Regex("^[a-zA-Z0-9._]+$").matches(packageName)) {
            "Invalid Android packageName: $packageName"
        }
        val proc = ProcessBuilder(
            "cmd",
            "appops",
            "set",
            packageName,
            "RUN_ANY_IN_BACKGROUND",
            "allow",
        ).redirectErrorStream(true).start()
        val drainThread = Thread {
            proc.inputStream.bufferedReader().use { it.readText() }
        }.apply {
            isDaemon = true
            start()
        }
        val finished = proc.waitFor(5, TimeUnit.SECONDS)
        if (!finished) {
            proc.destroyForcibly()
            return false
        }
        drainThread.join(500)
        return proc.exitValue() == 0
    }
}
