// 参考依据:
// - vendor_refs/android-libsu (Application 静态块初始化 Shell.setDefaultBuilder)
// - ARCHITECTURE_CONTRACTS.md 第 5 节 (Level 2/3 Root 通道初始化)

package com.lanvault.app

import android.app.Application
import com.topjohnwu.superuser.Shell

class LanVaultApplication : Application() {
    companion object {
        init {
            Shell.enableVerboseLogging = false
            Shell.setDefaultBuilder(
                Shell.Builder.create()
                    .setFlags(Shell.FLAG_MOUNT_MASTER)
                    .setTimeout(10),
            )
        }
    }
}
