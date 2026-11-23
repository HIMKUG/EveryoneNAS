# LanVault Android R8 Full Mode 混淆与保留规则
# 参考依据:
# - vendor_refs/android-libsu/core/proguard-rules.pro
# - vendor_refs/android-shizuku-api/provider/consumer-rules.pro
# - flutter_app/android/app/src/main/kotlin/com/lanvault/app/**

# ---------------------------------------------------------------------------
# 1. LanVault Application 与主入口组件
# ---------------------------------------------------------------------------
-keep class com.lanvault.app.LanVaultApplication {
    *;
}

# ---------------------------------------------------------------------------
# 2. 提权保活层（ShizukuUserService / RootDaemonService / KeepAliveService / JNI）
# 防止 R8 Full Mode 裁剪无直接 Java 引用的反射构造器、Companion 与 JNI native 方法
# ---------------------------------------------------------------------------
-keep class com.lanvault.app.privilege.** {
    *;
}

-keepclasseswithmembernames,includedescriptorclasses class com.lanvault.app.privilege.** {
    native <methods>;
}

-keepclassmembers class com.lanvault.app.privilege.ShizukuUserService {
    public <init>();
    public <init>(android.content.Context);
    public static native boolean nativeStartServer(java.lang.String, long, int, java.lang.String);
    public static native boolean nativeGracefulRestart();
}

-keepclassmembers class com.lanvault.app.privilege.RootDaemonService {
    public static native boolean nativeStartRootServer(java.lang.String, long, int, java.lang.String);
    public static native boolean nativeGracefulRestartRootServer();
}

-keepclassmembers class com.lanvault.app.privilege.KeepAliveService {
    public static native boolean nativeStartKeepAliveServer(java.lang.String, long, int, java.lang.String);
    public static native boolean nativeGracefulRestartKeepAliveServer();
}

# ---------------------------------------------------------------------------
# 3. AIDL 跨进程接口（Shizuku UserService Binder 桩）
# ---------------------------------------------------------------------------
-keep class com.lanvault.app.IShizukuLanVaultService** {
    *;
}

# ---------------------------------------------------------------------------
# 4. Shizuku API & Provider（BinderContainer Parcelable 与反射桥接）
# ---------------------------------------------------------------------------
-keep class rikka.shizuku.** {
    *;
}

-keep class moe.shizuku.** {
    *;
}

-keepnames class moe.shizuku.api.BinderContainer
-keepclassmembers class moe.shizuku.api.BinderContainer {
    public static final android.os.Parcelable$Creator CREATOR;
}

# ---------------------------------------------------------------------------
# 5. libsu (topjohnwu/superuser) RootService & Shell Initializer
# ---------------------------------------------------------------------------
-keep class com.topjohnwu.superuser.** {
    *;
}

-keep class * extends com.topjohnwu.superuser.Shell$Initializer {
    *;
}

-keep class * extends com.topjohnwu.superuser.ipc.RootService {
    *;
}

# ---------------------------------------------------------------------------
# 6. 通用 JNI / @Keep 保护（R8 Full Mode 必备）
# ---------------------------------------------------------------------------
-keepattributes Signature,InnerClasses,EnclosingMethod,RuntimeVisibleAnnotations
-keep @androidx.annotation.Keep class * {
    *;
}
-keepclassmembers class * {
    @androidx.annotation.Keep <methods>;
    @androidx.annotation.Keep <fields>;
}
-keepclasseswithmembernames,includedescriptorclasses class * {
    native <methods>;
}
