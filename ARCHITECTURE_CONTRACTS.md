# EveryoneNAS 跨端架构与接口契约规范（Single Source of Truth）

> 所有子智能体必须严格遵循本契约，确保 Rust 核心、Rust 服务端、Rust 客户端/FFI/WinSvc、Web 管理台、Flutter 前端与 Android 原生保活层之间的类型、路由、JSON 字段、FFI 签名与视觉规范 100% 对齐。

## 1. 工程基线与代码风格（强制）
- **零过度防御（No Over-Defense）**：仅在外部边界（HTTP 请求、WebSocket 消息、FFI 入参、磁盘 I/O、环境变量、JNI/MethodChannel）做一次解析与校验；内部模块信任静态类型，禁止静默 `try/catch` 吞错、禁止内部 `unwrap_or_default()` 掩盖错误、禁止猜测式回退链（`a ?? b ?? "Unknown"`）。
- **零占位符**：严禁留白 `todo!()`、`unimplemented!()`、`// TODO` 敷衍核心逻辑。
- **依据参考仓库**：编写对应模块前，必须先查阅 `/working_dir/c_0c10370daccfa96e/lanvault/vendor_refs/` 中的真实源码，并在文件头注释注明参考依据。

## 2. Workspace 目录与 Crate 边界
- 根目录：`/working_dir/c_0c10370daccfa96e/lanvault`
- Crates:
  - `lanvault-core`: 共享领域模型、错误类型 (`LanVaultError`)、加解密 (`crypto.rs`)、分块压缩 (`chunker.rs`)、SQLite WAL (`db.rs`)、mDNS+UDP 发现 (`discovery.rs`)、传输协议帧 (`protocol.rs`)。
  - `lanvault-server`: 存储池与 Volume GUID 管理 (`vault.rs`)、Bit-Rot 巡检自愈 (`scrub.rs`)、GFS 快照与引用计数 GC (`gc.rs`)、Argon2id + 3次锁定60秒防爆破与 Cookie 会话 (`auth.rs`)、Axum HTTP/WS/静态资源内嵌与优雅重启 (`web.rs`)。
  - `lanvault-client`: `notify` 500ms 防抖监听与溢出回调 (`watcher.rs`)、全量元数据 Diff 对账 (`scanner.rs`)、FastCDC 增量同步与令牌桶限流/省电策略 (`sync_engine.rs`)、指数退避自愈重连 (`reconnect.rs`)、剪贴板与临时快传 (`clipboard.rs`)。
  - `lanvault-ffi`: `flutter_rust_bridge` v2 暴露层 (`api.rs`)。
  - `lanvault-win-svc`: Windows SCM Session 0 服务 (`main.rs`)、防火墙自动放行 (`firewall.rs`)、Win32 Volume GUID 解析 (`volume_guid.rs`)。

## 3. 核心数据契约（JSON / REST / WebSocket）

### 3.1 REST API 路由与载荷（挂载于 `lanvault-server`）
所有需鉴权接口校验 `lv_session` Cookie（`HttpOnly; SameSite=Strict; Path=/`），未认证返回 `401 Unauthorized`。

1. `POST /api/auth/login`（公开，防爆破）
   - Request: `{ "password": string }`
   - 200 OK: `Set-Cookie: lv_session=<64-hex>; HttpOnly; SameSite=Strict; Path=/` + `{ "authenticated": true, "expires_in_secs": 86400 }`
   - 401 Unauthorized: `{ "error": "invalid_password", "remaining_attempts": number }` (连续错 1~2 次)
   - 429 Too Many Requests: Header `Retry-After: <secs>` + `{ "error": "locked_out", "retry_after_secs": number }` (连续错 3 次立即锁定该 IP 60 秒)
2. `POST /api/auth/logout`（Cookie）
   - 200 OK: 清除 Cookie + `{ "authenticated": false }`
3. `GET /api/system/status`（Cookie）
   - 200 OK:
     ```json
     {
       "os": "windows|android|linux",
       "keep_alive_level": "LEVEL_3_MAGISK_DAEMON|LEVEL_2_ROOT_LIBSU|LEVEL_1_SHIZUKU|LEVEL_0_FOREGROUND|WIN_SERVICE",
       "lan_ip": "192.168.x.x",
       "port": 7842,
       "mdns_active": true,
       "uptime_secs": 3600,
       "throughput_bps": 0,
       "bit_rot_health": "healthy|degraded|corrupted",
       "corrupted_chunks": 0,
       "vault_used_bytes": 0,
       "vault_quota_bytes": 107374182400
     }
     ```
4. `POST /api/system/restart`（Cookie）
   - 202 Accepted: `{ "status": "restarting", "drain_secs": 3 }`（服务端停接新任务 -> WS 广播 `server_restarting` -> 等待 ≤3s 活跃块落盘 -> `PRAGMA wal_checkpoint(TRUNCATE)` -> 触发重启钩子）
5. `GET /api/vaults`（Cookie）
   - 200 OK: `{ "vaults": [ { "vault_id": string, "name": string, "pool_mode": "single|striped|mirror_raid1", "quota_bytes": number, "used_bytes": number, "e2ee_enabled": boolean, "volumes": [ { "volume_id": string, "mount_path": string, "volume_guid": string|null, "role": "primary|mirror|pool_ext", "is_online": boolean } ] } ] }`
6. `POST /api/vaults`（Cookie）
   - Request: `{ "name": string, "mount_path": string, "volume_guid": string|null, "pool_mode": "single|striped|mirror_raid1", "quota_bytes": number, "e2ee_enabled": boolean, "extra_volumes": [ { "mount_path": string, "volume_guid": string|null, "role": "mirror|pool_ext" } ] }`
   - 201 Created: 返回新建 Vault 对象
   - `PUT /api/vaults/{vault_id}/devices/{device_id}` 允许管理员向已配对设备授予库访问权限。重复授权返回 `204`，普通设备无法授予权限。
7. `GET /api/snapshots?vault_id=<id>`（Cookie）
   - 200 OK: `{ "snapshots": [ { "snapshot_id": string, "vault_id": string, "device_id": string, "retention_tier": "hourly|daily|weekly|monthly|pinned", "file_count": number, "total_bytes": number, "created_at": number } ] }`
8. `POST /api/scrub/trigger`（Cookie）
   - 202 Accepted: `{ "scrub_id": string, "checked_chunks": number, "corrupted_chunks": number, "healed_chunks": number, "status": "completed|running" }`
9. `GET /api/peers`（Cookie）
   - 200 OK: `{ "peers": [ { "device_id": string, "device_name": string, "os_type": string, "last_ip": string|null, "last_port": number|null, "auto_connect": boolean, "online": boolean, "paired_at": number } ], "pairing_pin": string, "pairing_qr_uri": string }`
10. `DELETE /api/peers/:device_id`（Cookie）
    - 200 OK: `{ "revoked": true, "device_id": string }`
11. `POST /api/clipboard`（Cookie）
    - Request: `{ "target_device_id": string|null, "kind": "text|file", "content": string, "filename": string|null }`
    - 200 OK: `{ "delivered_to": number, "timestamp": number }`
12. `GET /ws`（Cookie 鉴权 WebSocket）
    - 服务端每秒推送：
      - `{ "type": "telemetry", "payload": <同 /api/system/status 字段> }`
      - `{ "type": "server_restarting", "payload": { "drain_secs": 3 } }`
      - `{ "type": "bit_rot_alert", "payload": { "chunk_hash": string, "vault_id": string, "status": "corrupted|healed" } }`
      - `{ "type": "clipboard_received", "payload": { "from_device_id": string, "kind": "text|file", "content": string, "filename": string|null, "timestamp": number } }`

## 4. `flutter_rust_bridge` FFI 契约 (`crates/lanvault-ffi/src/api.rs`)
必须暴露以下 9 个函数及对应 DTO 结构体（`DiscoveredPeer`, `SyncProgressEvent`）：
- `pub fn start_mdns_scan(sink: StreamSink<DiscoveredPeer>) -> anyhow::Result<()>`
- `pub async fn pair_with_pin(ip: String, port: u16, pin: String) -> anyhow::Result<String>`
- `pub async fn enable_auto_reconnect(enabled: bool) -> anyhow::Result<()>`
- `pub async fn start_local_server(vault_path: String, quota_gb: u64, web_password: String) -> anyhow::Result<u16>`
- `pub async fn prepare_graceful_restart() -> anyhow::Result<()>`
- `pub fn subscribe_sync_progress(sink: StreamSink<SyncProgressEvent>) -> anyhow::Result<()>`
- `pub async fn trigger_incremental_scan(rule_id: String) -> anyhow::Result<()>`
- `pub async fn update_power_network_policy(wifi_only: bool, allowed_ssids: Vec<String>, require_charging: bool, max_kbps: u32) -> anyhow::Result<()>`
- `pub async fn push_clipboard_text(target_device_id: Option<String>, text: String) -> anyhow::Result<()>`

## 5. Android MethodChannel 与保活契约
- Channel 名称：`com.lanvault.app/keep_alive`
- Methods:
  - `detectKeepAliveLevel` -> 返回 `"LEVEL_3_MAGISK_DAEMON" | "LEVEL_2_ROOT_LIBSU" | "LEVEL_1_SHIZUKU" | "LEVEL_0_FOREGROUND"`
  - `startKeepAliveServer` (args: `vaultPath`, `quotaGb`, `port`) -> `Boolean`
  - `gracefulRestartServer` -> `Boolean`
  - `requestBatteryExemption` -> `Boolean`
  - `requestStoragePermission` -> `Boolean`

## 6. 改良新拟态设计 Token（Web 与 Flutter 共享）
- 基底色：`#E6E9EF`；浮层/卡片表面：`#E6E9EF`；正文主色：`#1E293B`（对比度 > 10:1）；次级文字：`#475569`（对比度 > 5.7:1）
- 135° 光源双阴影：亮阴影 `#FFFFFF`（`-6px -6px 12px`），暗阴影 `#C5CCD8`（`6px 6px 12px`）
- 语义状态色（≥ 3:1）：健康 `#0F766E` / `#15803D`，警告 `#B45309`，告警/锁定 `#B91C1C`，主强调 `#1D4ED8`
- 交互硬规：按钮凸起、输入/容量槽/日志凹陷；触控热区 ≥ 44×44px；间距遵循 4pt/8pt 网格；禁止无意义的装饰脉动圆点、左侧彩色装饰条与冗余 KPI 徽章。

## 7. 双端合一三态运行模式（`client_only` / `server_only` / `dual`）与 8C16T 发行版构建规范

### 7.1 三态运行模式契约
Windows 与 Android 双端共享同一套 Rust FFI 核心（`lanvault-ffi`），节点可在运行期按需切换三种工作角色，无需安装不同安装包：
- **`client_only`（纯客户端模式）**：仅启动 `lanvault-client`（`notify` 防抖监听、FastCDC 增量同步、mDNS 发现、剪贴板/快传与自动重连）；不绑定本地存储池与 Axum HTTP/WS 端口，零常驻服务端内存与磁盘占用。
- **`server_only`（纯服务端/存储节点模式）**：仅启动 `lanvault-server`（存储池、Bit-Rot 巡检、GFS 快照/GC、Argon2id 会话鉴权与内嵌 Web 管理台）；在 Windows 上优先托管于 `lanvault-win-svc`（SCM Session 0），在 Android 上按 `Level 3 Magisk -> Level 2 Root libsu -> Level 1 Shizuku -> Level 0 FGS` 梯级保活。
- **`dual`（双端合一全功能模式）**：同一节点同时承担本地存储库服务端（`lanvault-server`）与局域网同步客户端（`lanvault-client`），支持双向互备与局域网对等剪贴板/快传；优雅重启（`prepare_graceful_restart`）须先暂停客户端写入并等待服务端 ≤3s 落盘与 `PRAGMA wal_checkpoint(TRUNCATE)`。

### 7.2 8 核 16 线程（8C16T）极限性能发行版构建规范
- **并发度基线**：统一锁定 16 并发任务（`CARGO_BUILD_JOBS=16`、`CMAKE_BUILD_PARALLEL_LEVEL=16`、`org.gradle.workers.max=16`、MSVC `/MP16`）。
- **Android Release（`arm64-v8a` 单架构极限裁剪）**：
  - Gradle & Kotlin：`org.gradle.parallel=true`、`org.gradle.caching=true`、`org.gradle.configuration-cache=true`、`org.gradle.jvmargs=-Xmx6144m -XX:+UseParallelGC`、`kotlin.incremental=true`、`kotlin.daemon.jvmargs=-Xmx4096m`。
  - AGP 8.7 + Kotlin 2.0：`namespace = "com.lanvault.app"`（`AndroidManifest.xml` 移除已废弃 `package` 属性）、`compileSdk = 35`、`minSdk = 26`、`targetSdk = 35`、`buildFeatures { aidl = true; buildConfig = true }`、`abiFilters = ["arm64-v8a"]`。
  - R8 Full Mode：`android.enableR8.fullMode=true`、`isMinifyEnabled = true`、`isShrinkResources = true`，搭配 `proguard-rules.pro` 严格保留 `LanVaultApplication`、`com.lanvault.app.privilege.**`（含 `native*` JNI 方法、`ShizukuUserService`、`RootDaemonService`）、`IShizukuLanVaultService**`、`rikka.shizuku.**` 与 `com.topjohnwu.superuser.**`。
  - 一键构建：`scripts/build_android_release.ps1`（支持 `-BuildAab` 开关），通过 `cargo ndk -t arm64-v8a` 编译启用 `+neon,+aes,+sha2,+crc` 的 `liblanvault_ffi.so`，结合 `flutter build apk --release --target-platform android-arm64 --split-per-abi --obfuscate --split-debug-info=build/symbols/android`，最终归档至 `dist/android-release/`。
- **Windows Release（MSVC AVX2 + LTCG 全程序优化）**：
  - CMake（`flutter_app/windows/CMakeLists.txt`）：Release 编译器启用 `/O2 /Ob2 /Oi /Ot /GL /arch:AVX2 /MP16`，链接器启用 `/LTCG /OPT:REF /OPT:ICF`，并自动定位拷贝 `lanvault_ffi.dll`（优先 `target/x86_64-pc-windows-msvc/release/lanvault_ffi.dll`，次选 `target/release/lanvault_ffi.dll`）至产物目录。
  - 一键构建：`scripts/build_windows_release.ps1`（支持 `-SkipWinSvc` 开关），以 `cargo build --release -p lanvault-ffi -p lanvault-win-svc --target x86_64-pc-windows-msvc -j 16` 编译原生组件，再执行 `flutter build windows --release --obfuscate --split-debug-info=build/symbols/windows`，最终将主程序、`lanvault_ffi.dll` 与可选 `lanvault-win-svc.exe` 统一归档至 `dist/windows-release/`。
