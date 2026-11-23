// 参考依据:
// - vendor_refs/android-libsu/service/src/main/java/com/topjohnwu/superuser/ipc/RootService.java
//   (CATEGORY_DAEMON_MODE 与 onUnbind 返回 true 保持 Daemon 进程常驻)
// - vendor_refs/android-libsu/example/src/main/java/com/topjohnwu/libsuexample/AIDLService.java
//   (仅在 Process.myUid() == 0 时通过 System.loadLibrary 加载 JNI .so)
// - lanvault_complete_blueprint.md 第 3.4 节 (写入 /proc/<pid>/oom_score_adj = -1000)

package com.lanvault.app.privilege

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Binder
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.Parcel
import android.os.Process
import android.util.Log
import com.topjohnwu.superuser.ipc.RootService
import java.io.File
import java.io.IOException

/**
 * Level 2 Root Daemon 模式保活服务（UID 0）：
 * - 通过 RootService.CATEGORY_DAEMON_MODE 启动独立 Root 进程。
 * - 重写 onUnbind() 返回 true，使主应用退出或解绑后 Root 守护进程持续常驻并在重连时触发 onRebind()。
 * - 启动时写入 /proc/<pid>/oom_score_adj = -1000 并通过 JNI 直接加载 Rust 核心库。
 */
class RootDaemonService : RootService() {
    companion object {
        private const val TAG = "LanVaultRootDaemon"
        private const val DESCRIPTOR = "com.lanvault.app.privilege.IRootDaemon"
        private const val TX_START_SERVER = IBinder.FIRST_CALL_TRANSACTION + 1
        private const val TX_GRACEFUL_RESTART = IBinder.FIRST_CALL_TRANSACTION + 2

        init {
            // 与 libsu AIDLService 示例一致：仅在 UID 0 的 Root 进程中加载 JNI .so，
            // 避免主应用非 Root 进程构造 Intent 引用此类时重复加载。
            if (Process.myUid() == 0) {
                System.loadLibrary("lanvault_ffi")
            }
        }

        @JvmStatic
        external fun nativeStartRootServer(vaultPath: String, quotaGb: Long, port: Int): Boolean

        @JvmStatic
        external fun nativeGracefulRestartRootServer(): Boolean

        private val mainHandler = Handler(Looper.getMainLooper())

        @Volatile
        private var remoteBinder: IBinder? = null

        @Volatile
        private var pendingLaunch: Triple<String, Long, Int>? = null

        private val connection = object : ServiceConnection {
            override fun onServiceConnected(name: ComponentName?, service: IBinder?) {
                if (service == null || !service.pingBinder()) return
                remoteBinder = service
                val pending = pendingLaunch
                if (pending != null) {
                    pendingLaunch = null
                    transactStart(service, pending.first, pending.second, pending.third)
                }
            }

            override fun onServiceDisconnected(name: ComponentName?) {
                remoteBinder = null
            }
        }

        private fun buildDaemonIntent(context: Context): Intent {
            return Intent(context, RootDaemonService::class.java).apply {
                addCategory(CATEGORY_DAEMON_MODE)
            }
        }

        private fun bindOnMainThread(context: Context) {
            val intent = buildDaemonIntent(context)
            if (Looper.myLooper() == Looper.getMainLooper()) {
                bind(intent, connection)
            } else {
                mainHandler.post { bind(intent, connection) }
            }
        }

        fun bindAndStart(context: Context, vaultPath: String, quotaGb: Long, port: Int) {
            val existing = remoteBinder
            if (existing != null && existing.pingBinder()) {
                pendingLaunch = null
                transactStart(existing, vaultPath, quotaGb, port)
                return
            }
            pendingLaunch = Triple(vaultPath, quotaGb, port)
            bindOnMainThread(context)
        }

        fun ensureBound(context: Context) {
            val existing = remoteBinder
            if (existing != null && existing.pingBinder()) return
            bindOnMainThread(context)
        }

        fun gracefulRestart(context: Context): Boolean {
            val binder = remoteBinder ?: run {
                ensureBound(context)
                return false
            }
            val data = Parcel.obtain()
            val reply = Parcel.obtain()
            try {
                data.writeInterfaceToken(DESCRIPTOR)
                binder.transact(TX_GRACEFUL_RESTART, data, reply, 0)
                reply.readException()
                return reply.readInt() == 1
            } finally {
                data.recycle()
                reply.recycle()
            }
        }

        private fun transactStart(
            binder: IBinder,
            vaultPath: String,
            quotaGb: Long,
            port: Int,
        ): Boolean {
            val data = Parcel.obtain()
            val reply = Parcel.obtain()
            try {
                data.writeInterfaceToken(DESCRIPTOR)
                data.writeString(vaultPath)
                data.writeLong(quotaGb)
                data.writeInt(port)
                binder.transact(TX_START_SERVER, data, reply, 0)
                reply.readException()
                return reply.readInt() == 1
            } finally {
                data.recycle()
                reply.recycle()
            }
        }
    }

    private val daemonBinder = object : Binder() {
        override fun onTransact(code: Int, data: Parcel, reply: Parcel?, flags: Int): Boolean {
            return when (code) {
                TX_START_SERVER -> {
                    data.enforceInterface(DESCRIPTOR)
                    val vaultPath = data.readString()!!
                    val quotaGb = data.readLong()
                    val port = data.readInt()
                    val ok = nativeStartRootServer(vaultPath, quotaGb, port)
                    reply?.writeNoException()
                    reply?.writeInt(if (ok) 1 else 0)
                    true
                }

                TX_GRACEFUL_RESTART -> {
                    data.enforceInterface(DESCRIPTOR)
                    val ok = nativeGracefulRestartRootServer()
                    reply?.writeNoException()
                    reply?.writeInt(if (ok) 1 else 0)
                    true
                }

                else -> super.onTransact(code, data, reply, flags)
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        protectAgainstLmk()
    }

    override fun onBind(intent: Intent): IBinder {
        return daemonBinder
    }

    override fun onRebind(intent: Intent) {
        protectAgainstLmk()
    }

    /**
     * 关键：Daemon 模式必须返回 true，确保客户端解绑后 RootService 进程不被销毁，
     * 下次绑定时通过 onRebind 复用同一常驻进程。
     */
    override fun onUnbind(intent: Intent): Boolean = true

    private fun protectAgainstLmk() {
        val pid = Process.myPid()
        try {
            File("/proc/$pid/oom_score_adj").writeText("-1000\n")
        } catch (e: IOException) {
            Log.w(TAG, "Unable to set /proc/$pid/oom_score_adj: ${e.message}")
        }
    }
}
