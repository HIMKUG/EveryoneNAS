// 参考依据:
// - vendor_refs/android-shizuku-api/demo/src/main/aidl/rikka/shizuku/demo/IUserService.aidl
//   (保留 Shizuku 服务端约定的 destroy() = 16777114 事务号)

package com.lanvault.app;

interface IShizukuLanVaultService {
    // Shizuku server 保留的销毁事务码
    void destroy() = 16777114;

    int getPid() = 1;
    int getUid() = 2;
    boolean startServer(String vaultPath, long quotaGb, int port, String webPassword) = 3;
    boolean gracefulRestart() = 4;
    boolean allowBackgroundAppOps(String packageName) = 5;
    boolean stopServer() = 6;
}
