// 参考依据:
// - vendor_refs/android-shizuku-api/api/src/main/java/rikka/shizuku/Shizuku.java
//   (Shizuku.UserServiceArgs + Shizuku.bindUserService / unbindUserService)
// - ARCHITECTURE_CONTRACTS.md 第 5 节 (Level 1 Shizuku 保活通道)

package com.lanvault.app.privilege

import android.content.ComponentName
import android.content.Context
import android.content.ServiceConnection
import android.os.IBinder
import com.lanvault.app.IShizukuLanVaultService
import rikka.shizuku.Shizuku
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Level 1 Shizuku 绑定桥接器：通过 Shizuku.bindUserService 拉起独立 UID 2000 守护进程，
 * 并在连接建立后下发 RUN_ANY_IN_BACKGROUND allow 与 Rust 服务端启动参数。
 */
object ShizukuBridge {
    private data class PendingStart(
        val packageName: String,
        val vaultPath: String,
        val quotaGb: Long,
        val port: Int,
        val webPassword: String,
        val latch: CountDownLatch,
        val result: AtomicBoolean,
    )

    @Volatile
    private var remoteService: IShizukuLanVaultService? = null

    @Volatile
    private var pendingStart: PendingStart? = null

    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, service: IBinder?) {
            if (service == null || !service.pingBinder()) {
                completePending(false)
                return
            }
            val bound = IShizukuLanVaultService.Stub.asInterface(service)
            remoteService = bound
            val start = pendingStart
            if (start != null) {
                pendingStart = null
                Thread {
                    try {
                        bound.allowBackgroundAppOps(start.packageName)
                        start.result.set(
                            bound.startServer(
                                start.vaultPath,
                                start.quotaGb,
                                start.port,
                                start.webPassword,
                            ),
                        )
                    } finally {
                        start.latch.countDown()
                    }
                }.start()
            }
        }

        override fun onServiceDisconnected(name: ComponentName?) {
            remoteService = null
            completePending(false)
        }
    }

    private fun buildUserServiceArgs(context: Context): Shizuku.UserServiceArgs {
        return Shizuku.UserServiceArgs(
            ComponentName(context.packageName, ShizukuUserService::class.java.name),
        )
            .daemon(true)
            .processNameSuffix("shizuku_daemon")
            .debuggable(false)
            .version(2)
    }

    fun bindAndStart(
        context: Context,
        vaultPath: String,
        quotaGb: Long,
        port: Int,
        webPassword: String,
    ): Boolean {
        val latch = CountDownLatch(1)
        val result = AtomicBoolean(false)
        val start = PendingStart(
            packageName = context.packageName,
            vaultPath = vaultPath,
            quotaGb = quotaGb,
            port = port,
            webPassword = webPassword,
            latch = latch,
            result = result,
        )
        val existing = remoteService
        if (existing != null && existing.asBinder().pingBinder()) {
            pendingStart = null
            existing.allowBackgroundAppOps(start.packageName)
            return existing.startServer(
                start.vaultPath,
                start.quotaGb,
                start.port,
                start.webPassword,
            )
        }
        completePending(false)
        pendingStart = start
        Shizuku.bindUserService(buildUserServiceArgs(context), connection)
        val completed = latch.await(10, TimeUnit.SECONDS)
        if (!completed && pendingStart === start) {
            pendingStart = null
        }
        return completed && result.get()
    }

    private fun completePending(value: Boolean) {
        val start = pendingStart ?: return
        pendingStart = null
        start.result.set(value)
        start.latch.countDown()
    }

    fun ensureBound(context: Context) {
        val existing = remoteService
        if (existing != null && existing.asBinder().pingBinder()) return
        Shizuku.bindUserService(buildUserServiceArgs(context), connection)
    }

    fun gracefulRestart(): Boolean {
        val service = remoteService
            ?: throw IllegalStateException("ShizukuUserService is not bound.")
        return service.gracefulRestart()
    }

    fun stopServer(): Boolean {
        val service = remoteService
            ?: throw IllegalStateException("ShizukuUserService is not bound.")
        return service.stopServer()
    }

    fun unbind(context: Context, removeDaemon: Boolean = false) {
        Shizuku.unbindUserService(buildUserServiceArgs(context), connection, removeDaemon)
        remoteService = null
    }
}
