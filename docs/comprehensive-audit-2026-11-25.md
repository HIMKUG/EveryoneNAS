# EveryoneNAS 前后端综合审计报告

审计日期：2026-11-25（Asia/Shanghai）

审计基准：`D:\code\nas\EveryoneNAS-Merge`，分支 `integration/best-of-both`，提交 `9d195ff6a11f97d4c6946c9811881086dcc7a43b`。

本次为只读审计，未修复源码。`D:\code\nas\EveryoneNAS` 是已归档的旧工作树，仅用于确认归档标记，未将其源码作为当前实现依据。

## 1. 结论摘要

当前版本不是“完全不可用”：Rust 工作区检查与全部测试通过，Flutter 的 8 项 Widget 测试通过，Android Debug APK 也完成了三 ABI 构建。主要风险集中在四类：

1. **配对二维码可能不可扫**：正常长度的配对 URI 会被静默截断；Version 6 的纠错编码还有标准一致性疑点。
2. **授权与资源边界不完整**：已配对设备可用私钥持续续期，公开 challenge 接口缺少速率限制。
3. **前端错误与竞态处理不足**：加载失败被显示成空数据或锁屏，拒绝配对失败会显示成功，部分旧请求可覆盖新状态。
4. **Android 原生状态回报不可靠**：前台服务“已发起启动”被当成“启动成功”，失败清理与权限完成态不完整。

严重度口径：P0 为可立即造成数据丢失、大规模越权或服务不可用；P1 为高影响的安全、数据或核心功能缺陷；P2 为中影响的功能、可靠性、可用性或文档缺陷；P3 为低影响或工程治理问题。

## 2. 高优先级问题

### EN-01｜P1｜确认问题：配对二维码会静默截断正常 URI

- 位置：`web-ui/js/qr.js:15`、`web-ui/js/qr.js:18`、`web-ui/js/qr.js:20`
- 触发：配对 URI 的 UTF-8 长度超过 Version 6-L 的 134 字节数据容量。
- 证据：实现最高只提供 Version 6，超容量时执行 `dataBytes.slice(0, spec.data - 2)`，没有报错或降级提示。典型 IPv4 配对 URI 为 141 字节，会丢失末尾 7 字节指纹。
- 误判排除：这不是扫码器容错问题，而是编码器主动丢弃数据；URI 中的 `fp` 是设备身份校验依据，截断后无法得到完整配对信息。
- 影响：用户看到二维码，但扫码后得到不完整链接，首次配对失败或无法验证目标设备。
- 建议：超容量时明确失败；扩展 QR 版本，或改用更短的配对载荷并在扫码后补全信息。
- 验证：容量与截断路径已静态及 Node 校核；真实相机扫码未验证。

### EN-02｜P1｜疑似问题：Version 6 的 Reed-Solomon 分块与交错不符合标准

- 位置：`web-ui/js/qr.js:15`、`web-ui/js/qr.js:56`、`web-ui/js/qr.js:73`、`web-ui/js/qr.js:112`
- 触发：二维码容量进入 Version 6-L。
- 证据：Version 6-L 需要两个 RS 块及块间交错，当前实现生成一组 ECC 后直接拼接。Node 校核中 Version 1–5 的 RS syndromes 全为零，Version 6 两个块的全部 18 个 syndromes 非零。
- 误判排除：固定 mask 0 本身合法，格式位写入值也与标准映射一致；疑点集中在 Version 6 的 RS 分块与交错。
- 影响：Version 6 二维码可能无法被标准解码器读取。由于缺少独立解码器，最终可扫性尚未证明。
- 建议：按 QR 标准实现 Version 6-L 的双块生成与交错，统一格式信息，并加入标准二维码解码回归测试。
- 验证：算法级 Node 校核完成；独立解码与真机扫码未验证。

### EN-03｜P1｜确认问题：Android 前台服务把“已发起启动”报告为“启动成功”

- 位置：`flutter_app/android/app/src/main/kotlin/com/lanvault/app/MainActivity.kt:301`、`flutter_app/android/app/src/main/kotlin/com/lanvault/app/MainActivity.kt:310`、`flutter_app/lib/bridge/lanvault_bridge.dart:3687`、`flutter_app/lib/bridge/lanvault_bridge.dart:3693`
- 触发：Android Level 0 路径启动 Rust 服务或前台服务失败。
- 证据：原生 Level 0 分支调用返回 `Unit` 的 `KeepAliveService.start()` 后无条件返回 `true`；`start()` 只发起 `startForegroundService()`，不等待 `onStartCommand()` 中的 `startForeground()` 或原生启动结果。Dart 随后把 `serverRunningNotifier` 设为 `true`。
- 误判排除：Dart 忽略布尔值确实是问题，但即使检查布尔值也无法得到真实结果，根因在原生层把“请求已发送”当成“启动已完成”。
- 影响：界面显示服务运行，但后台服务可能没有建立；退出 App 后缺少可靠保活与恢复。
- 建议：通过 MethodChannel 回报 `onStartCommand()` 的真实结果；失败时回滚 UI 状态并提供重试。
- 验证：静态确认；未在 Android 真机模拟启动失败。

### EN-04｜P1｜确认问题：拒绝配对失败被吞掉并显示成功

- 位置：`flutter_app/lib/main.dart:385`、`flutter_app/lib/main.dart:387`、`flutter_app/lib/main.dart:398`
- 触发：用户在入站配对弹窗点击“拒绝”，后端拒绝请求失败。
- 证据：拒绝分支使用 `catch (_) {}`，随后只要页面仍 mounted 就弹出“已拒绝”的成功提示；同意分支在 `flutter_app/lib/main.dart:431` 有失败提示。
- 误判排除：两条分支的错误处理明显不对称；后端失败不会因前端 catch 而变成成功。
- 影响：用户误以为请求已拒绝，对端可能仍等待配对，形成安全认知偏差。
- 建议：仅在确认后端成功后提示成功；失败时保留请求并显示重试或明确错误。
- 验证：静态确认；现有测试只覆盖同意成功。

## 3. 中优先级问题

### EN-05｜P2｜确认问题：已配对设备私钥可无限续期 peer 会话

- 位置：`crates/lanvault-server/src/web/mod.rs:532`、`crates/lanvault-server/src/web/mod.rs:535`、`crates/lanvault-server/src/web/mod.rs:2884`、`crates/lanvault-server/src/web/mod.rs:2945`、`crates/lanvault-server/src/auth.rs:303`
- 触发：持有仍处于 paired 状态设备 Ed25519 私钥的攻击者持续请求 challenge 并签名续期。
- 证据：challenge 与 renew 都是公开路由；renew 不要求既有 Cookie，只验证登记公钥、设备未吊销和一次性挑战，然后签发新的 24 小时 peer 会话。
- 误判排除：这不是 admin 提权，设备吊销后也无法续期；但它确实使单个 token 的 24 小时 TTL 不能成为设备访问截止时间。
- 影响：设备私钥失窃后，攻击者可无限维持 peer 权限，直到管理员吊销设备或轮换密钥。
- 建议：明确设备私钥是长期凭证；若需要强制重新认证，应限制续期总时长或要求管理员批准。文档应明确“吊销设备”才是终止授权的手段。
- 验证：静态调用链确认；未进行网络攻击复现。

### EN-06｜P2｜确认问题：公开续期 challenge 接口缺少速率限制

- 位置：`crates/lanvault-server/src/web/mod.rs:2862`、`crates/lanvault-server/src/web/mod.rs:2321`
- 触发：攻击者知道任一有效 `device_id`，持续请求公开 challenge 接口。
- 证据：handler 不获取来源 IP，没有 IP 或设备级频率限制；每次有效请求都查询数据库、生成随机数并插入挑战表。全局容量满时驱逐最早到期项，而不是拒绝请求。
- 误判排除：容量上限只能限制内存，不能限制数据库与 CPU 消耗，也不能保护其他设备的待验证挑战。
- 影响：可造成持续资源消耗，并驱逐其他设备的 challenge，导致正常续期失败。
- 建议：按 IP 与 `device_id` 限流；满载时返回 429，不驱逐其他设备的挑战。
- 验证：静态确认；未压测。

### EN-07｜P2｜确认问题：设置页任一初始化接口失败会整体静默放弃

- 位置：`flutter_app/lib/features/settings/settings_screen.dart:64`、`flutter_app/lib/features/settings/settings_screen.dart:76`、`flutter_app/lib/features/settings/settings_screen.dart:89`
- 触发：串行读取的八个设置接口中任意一个失败。
- 证据：所有读取在 `try` 中串行执行，状态统一在最后一次 `setState` 提交；`catch (_) {}` 不提示、不重试，也不提交前面已成功读取的数据。
- 误判排除：不是“界面仍有旧值”这么简单，而是首次进入时可能持续显示默认值。
- 影响：用户可能依据错误显示覆盖真实后端配置。
- 建议：独立读取、逐项更新；失败项展示错误与重试，不阻断其他设置。
- 验证：静态确认；未注入接口失败。

### EN-08｜P2｜确认问题：快照页旧请求可覆盖新筛选状态

- 位置：`flutter_app/lib/features/snapshots/snapshots_screen.dart:87`、`flutter_app/lib/features/snapshots/snapshots_screen.dart:115`
- 触发：快速切换存储池、规则绑定或“仅看源已删除”，多个 `_loadData()` 尚未完成。
- 证据：外层 `_loadData()` 没有请求序号；旧请求在 `Future.wait` 后直接覆盖 `_vaults`、`_snapshots`、`_selectedBindingId`。内层目录请求有序号保护，但保护不了外层先写入错误选择。
- 误判排除：不是所有异步加载都缺保护，目录加载已有保护；问题 specifically 在外层快照加载。
- 影响：下拉框、列表与目录内容不一致，后续操作可能作用于错误 Vault 或绑定。
- 建议：为外层请求增加代次与筛选条件校验。
- 验证：静态确认；未构造延迟 Future 复现。

### EN-09｜P2｜确认问题：Vault 读取失败被显示成“空存储池”

- 位置：`flutter_app/lib/features/vault/vault_screen.dart:53`、`flutter_app/lib/features/vault/vault_screen.dart:393`
- 触发：初次加载失败，或服务状态变化触发 `silent: true` 刷新失败。
- 证据：catch 只清除 `_loading`，仅非 silent 请求提示错误；列表仍为空时进入“还没有存储池”空态。
- 误判排除：服务正在运行不能证明列表读取成功，空态与错误态被合并。
- 影响：网络或后端异常被误判为没有数据，用户可能重复创建或误以为数据丢失。
- 建议：增加独立错误态与重试；仅在成功且列表为空时显示空态。
- 验证：静态确认；现有测试只覆盖服务未启动。

### EN-10｜P2｜确认问题：Web 任一面板加载失败会进入锁屏或阻塞实时连接

- 位置：`web-ui/js/app.js:1645`、`web-ui/js/app.js:1686`、`web-ui/js/app.js:2029`
- 触发：状态、Vault 或设备等核心接口返回非 401/429 错误。
- 证据：`Promise.all` 中任一失败都会拒绝整体加载；错误被统一导向锁屏。登录后可能先显示空仪表盘，再因加载失败无法启动 WebSocket。
- 误判排除：401/429 导向锁屏是合理设计，问题是非认证错误也被同等处理。
- 影响：后端局部故障被伪装成未登录，实时遥测也可能不启动。
- 建议：按面板隔离错误，单独初始化实时连接，仅认证错误触发锁屏。
- 验证：静态确认；未做浏览器故障注入。

### EN-11｜P2｜确认问题：Web 目录与版本请求存在过期响应竞态

- 位置：`web-ui/js/app.js:971`、`web-ui/js/app.js:1004`、`web-ui/js/app.js:1087`、`web-ui/js/app.js:1149`
- 触发：快速切换绑定、切换文件或关闭目录时仍有请求在途。
- 证据：旧请求继续写入共享状态；版本下载链接使用当前绑定 ID，可能与旧响应中的版本组合。
- 误判排除：不是 XSS 或 URL 编码问题，而是请求生命周期与共享状态缺少绑定校验。
- 影响：展示错误目录或版本，用户可能下载非预期内容。
- 建议：增加请求代次，并在提交与下载前校验绑定 ID。
- 验证：静态确认；未做竞态复现。

### EN-12｜P2｜确认问题：Web 登出失败仍显示“已锁定”

- 位置：`web-ui/js/app.js:1953`、`web-ui/js/api.js:131`、`crates/lanvault-server/src/web/mod.rs:1121`
- 触发：登出时断网或服务端返回错误。
- 证据：`finally` 无条件显示锁屏，但服务端失败时不会吊销会话或清除 Cookie。
- 误判排除：本地隐藏 UI 不等于服务端注销，二者被混为一谈。
- 影响：用户误以为已安全登出，共享设备上的会话可能仍有效。
- 建议：区分“本地锁定”与“服务端吊销”；失败时明确提示并重试。
- 验证：静态确认；未做断网复现。

### EN-13｜P2｜确认问题：Web 组与历史加载错误被伪装成空数据

- 位置：`web-ui/js/app.js:1650`、`web-ui/js/app.js:1659`、`web-ui/js/app.js:1669`
- 触发：设备组、组详情或能力接口失败。
- 证据：组接口失败显示“暂无设备组”，组详情错误被空 `catch` 吞掉，能力失败回退为默认 `v2`。
- 误判排除：默认值可以作为兼容降级，但不能同时当作“确认支持”的依据。
- 影响：用户无法区分无数据、无权限、接口故障与真实能力。
- 建议：分离错误、空态与能力未知态。
- 验证：静态确认；未做故障注入。

### EN-14｜P2｜确认问题：Web `fetch` 没有超时与取消机制

- 位置：`web-ui/js/api.js:69`、`web-ui/js/api.js:83`、`web-ui/js/app.js:1682`
- 触发：响应永久挂起或网络半开。
- 证据：请求直接使用 `fetch`，没有 `AbortSignal` 或超时包装；登录按钮可能持续禁用，管理操作无限等待。
- 误判排除：浏览器最终可能超时，但应用层没有可控反馈与取消路径。
- 影响：界面长时间假死，用户无法区分慢请求与失败。
- 建议：增加明确超时，并区分用户取消与网络失败。
- 验证：静态确认；未做挂起连接测试。

### EN-15｜P3｜确认问题：动态附加卷控件缺少可访问名称

- 位置：`web-ui/js/app.js:584`、`web-ui/js/app.js:591`、`web-ui/js/app.js:597`
- 触发：使用读屏浏览动态附加卷配置。
- 证据：挂载路径、GUID 与角色控件没有 `label` 或 `aria-label`；占位符不能替代可访问名称。
- 误判排除：这是用户可操作表单，不是装饰元素。
- 影响：读屏用户无法可靠识别控件用途。
- 建议：补充稳定标签或 `aria-label`。
- 验证：静态确认；未做读屏实测。

### EN-16｜P3｜设计取舍：移动端核心操作触控区域仅 42px

- 位置：`flutter_app/lib/features/clipboard/clipboard_screen.dart:748`、`flutter_app/lib/features/clipboard/clipboard_screen.dart:753`
- 触发：Android 手机使用发送文件、粘贴或发送等圆形操作。
- 证据：`InkWell` 内部容器固定为 `42×42`，未通过约束扩大命中区域；外层 `Tooltip` 不扩大点击范围。
- 误判排除：`Tooltip` 会提供可访问语义，因此不是无语义裸控件；48dp 是常用无障碍建议而非强制合规基线。剩余问题是命中区域偏小，且部分读屏不一定朗读 tooltip。
- 影响：小屏或运动障碍用户误触、点不中的概率增加。
- 建议：保持 42px 视觉尺寸，将命中区域扩展到至少 48×48。
- 验证：静态确认；未做触控可用性实测。

### EN-17｜P3｜疑似问题：Android Level 0 启动或重启失败未显式清理前台通知

- 位置：`flutter_app/android/app/src/main/kotlin/com/lanvault/app/privilege/KeepAliveService.kt:265`、`flutter_app/android/app/src/main/kotlin/com/lanvault/app/privilege/KeepAliveService.kt:355`
- 触发：原生服务启动或优雅重启失败。
- 证据：初次失败调用 `stopSelf(startId)`，重启失败调用无参 `stopSelf()`，均未显式 `stopForeground`；`onDestroy()` 只释放传输锁。
- 误判排除：`stopSelf()` 结束后系统通常会移除该服务的通知，因此不能断言一定残留；但代码没有显式 `stopForeground`，通知生命周期仍依赖系统行为。`onTimeout()` 路径已有显式清理。
- 影响：不同系统版本或异常停止路径下，通知清理行为缺少确定性。
- 建议：统一失败清理路径，停止前台状态并回报 Dart。
- 验证：静态确认；未在设备模拟 JNI 失败。

### EN-18｜P2｜疑似问题：Android 15 Level 0 超时恢复受固定网络条件限制

- 位置：`flutter_app/android/app/src/main/kotlin/com/lanvault/app/privilege/KeepAliveService.kt:160`、`flutter_app/android/app/src/main/kotlin/com/lanvault/app/privilege/KeepAliveService.kt:279`
- 触发：Android 15 `dataSync` 服务触发 `onTimeout`，设备没有非计费 Wi-Fi 或系统延迟 WorkManager。
- 证据：恢复任务固定要求 `NetworkType.UNMETERED`，周期为 15 分钟；Worker 后台启动受限时捕获异常后仍返回 `Result.success()`。
- 误判排除：这符合“不得在 BOOT_COMPLETED 直接启动 dataSync FGS”的平台限制，不能简单改成立即后台启动；但失败被报告为成功。
- 影响：服务超时后可能长时间不可用，且用户无法感知恢复失败。
- 建议：明确支持的网络范围；启动受限时返回可重试结果并暴露恢复状态。
- 验证：静态疑似；Android 15 超时与厂商策略未实测。

### EN-19｜P2｜设计取舍：Android 权限入口不回报最终授权结果

- 位置：`flutter_app/android/app/src/main/kotlin/com/lanvault/app/MainActivity.kt:345`、`flutter_app/android/app/src/main/kotlin/com/lanvault/app/MainActivity.kt:357`、`flutter_app/android/app/src/main/kotlin/com/lanvault/app/MainActivity.kt:371`
- 触发：用户使用电池白名单、所有文件访问、定位或通知权限入口。
- 证据：电池和文件权限立即返回 `false`；定位和通知调用 `requestPermissions()` 后也立即返回 `false`，类中未实现 `onRequestPermissionsResult`。
- 误判排除：Dart 侧提示“请在系统设置中允许”，说明可能是有意的“仅打开设置”交互；但从设置返回后没有真实状态确认。
- 影响：用户已授权后 UI 仍可能显示未授权，或反复要求操作。
- 建议：页面恢复时重新查询权限；区分已发起、已拒绝与永久拒绝。
- 验证：静态设计取舍；未做设备验证。

### EN-20｜P2｜设计取舍：Web 端不能创建、编辑或启用 v2 规则与绑定

- 位置：`web-ui/index.html:339`、`web-ui/js/app.js:871`、`crates/lanvault-server/src/web/mod.rs:462`
- 触发：用户希望完全通过 Web 管理台配置 v2 同步。
- 证据：后端与 API 客户端具备 groups/rules/bindings 写接口，但页面只展示状态，没有配置入口。
- 误判排除：README 未明确承诺 Web 端完整配置，因此不是明确的承诺违约；但这是可见的功能边界。
- 影响：Web 与桌面端能力不一致，用户可能误以为配置未生效。
- 建议：明确标注“只读”，或补齐 Web 配置入口。
- 验证：静态确认。

### EN-21｜P2｜确认问题：Web 自定义按钮遗漏标准 `click` 激活路径

- 位置：`web-ui/js/app.js:336`、`web-ui/js/timeline.js:117`
- 触发：读屏或辅助技术派发标准 `click`。
- 证据：按钮只处理 `pointerup` 和 `keydown`，普通键盘可用，但标准 `click` 不触发动作。
- 误判排除：不是完全没有键盘支持，而是动作入口与 Web 按钮语义不一致。
- 影响：依赖程序化标准 `click` 的辅助技术路径无法激活操作；普通键盘 Enter/Space 路径存在。
- 建议：以 `click` 作为动作入口，指针事件仅负责按压反馈。
- 验证：静态确认；未做读屏实测。

### EN-22｜P2｜确认问题：恢复索引时发现离线镜像卷会把已恢复数据标记为“不完整”

- 位置：`crates/lanvault-server/src/lib.rs:521`、`crates/lanvault-server/src/lib.rs:530`、`crates/lanvault-server/src/lib.rs:644`
- 触发：数据库丢失后启动恢复，此时 Mirror RAID1 的一块镜像盘离线，但在线卷已包含完整 chunks 与 manifests。
- 证据：函数为每个 `is_online == false` 的卷直接设置 `complete = false`，即使所有 manifest 已从其他在线卷恢复完成。
- 误判排除：离线卷不影响“当前已恢复数据是否完整”，它只影响副本冗余度；两者被混为一个布尔值。
- 影响：完整恢复被标为不完整，可能触发不必要的告警或误导运维判断。
- 建议：分别记录“恢复完整性”和“副本冗余状态”。
- 验证：静态确认；未构造数据库丢失与离线盘场景复现。

## 4. 低优先级与工程治理问题

### EN-23｜P3｜确认问题：传输消息气泡缺少“复制”动作的可访问名称

- 位置：`flutter_app/lib/features/clipboard/clipboard_screen.dart:701`、`flutter_app/lib/features/clipboard/clipboard_screen.dart:706`
- 证据：整个气泡可点击复制，但没有 `Semantics`、`tooltip` 或按钮角色；正文会被朗读，但“点击可复制”不会自动说明。
- 建议：添加“复制消息”语义标签与按钮角色，同时保留正文可朗读。
- 验证：静态确认；未做 TalkBack 测试。

### EN-24｜P3｜确认问题：Android 权限永久拒绝后缺少恢复入口

- 位置：`flutter_app/android/app/src/main/kotlin/com/lanvault/app/MainActivity.kt:371`、`flutter_app/lib/features/settings/settings_screen.dart:210`
- 证据：原生只调用 `requestPermissions()`，不检查 `shouldShowRequestPermissionRationale`，也不提供应用设置跳转；Dart 只显示通用提示。
- 误判排除：定位拒绝后仍可手动输入 SSID，影响有限；但通知权限影响前台服务可见性。
- 建议：永久拒绝时提供“打开系统设置”。
- 验证：静态疑似；未模拟“不再询问”。

### EN-25｜P3｜确认问题：最近服务配置在原生读取抛错时不回退到 Rust

- 位置：`flutter_app/lib/bridge/lanvault_bridge.dart:3625`
- 证据：先读 Android 配置，读取异常直接向上抛出；只有原生返回 `null` 时才回退 Rust。
- 影响：即使另一侧仍有可用配置，重启、最近路径回填等功能仍可能失败。
- 建议：明确配置优先级与部分失败策略，区分权限、解析错误和真实无配置。
- 验证：静态确认；未注入故障。

### EN-26｜P3｜确认问题：测试覆盖无法防止多项前端与平台回归

- 位置：`flutter_app/test/widget_test.dart:570`、`flutter_app/test/widget_test.dart:862`、`README.md:414`、`README.md:787`
- 证据：Flutter 测试主要覆盖文案、样式、两个窄屏布局和配对同意成功；没有 Android MethodChannel 契约测试，也没有接口异常、请求乱序、超时或销毁测试。Web 端只有 `node --check`，不执行 UI 脚本。
- 影响：本次发现的 QR、竞态、拒绝失败和 Android 启动状态问题都能在测试全绿时存在。
- 建议：优先补桥接返回值、启动失败、权限状态、异步乱序、拒绝失败及 QR 解码测试。
- 验证：测试清单与运行结果已核对。

### EN-27｜P3｜确认问题：会话令牌文档与实现不符

- 位置：`README.md:262`、`ARCHITECTURE_CONTRACTS.md:28`、`crates/lanvault-server/src/auth.rs:353`
- 证据：文档称令牌为“64 位 hex 随机令牌”，实现是“十六进制载荷.十六进制签名”的点分格式。
- 误判排除：Cookie 安全属性正确，问题仅在文档描述，不影响当前鉴权。
- 建议：更新文档为实际格式与签名校验流程。
- 验证：静态确认。

### EN-28｜P3｜确认问题：静态分析存在 9 项告警

- 位置：`flutter_app/lib/bridge/lanvault_bridge.dart:3253`、`flutter_app/lib/features/dashboard/dashboard_screen.dart:597`、`flutter_app/lib/features/settings/settings_screen.dart:255`、`flutter_app/lib/features/settings/settings_screen.dart:280`、`flutter_app/lib/features/settings/settings_screen.dart:641`、`flutter_app/lib/features/settings/settings_screen.dart:883`、`flutter_app/lib/features/settings/settings_screen.dart:899`、`flutter_app/lib/features/settings/settings_screen.dart:1006`、`flutter_app/lib/features/settings/settings_screen.dart:1115`
- 证据：1 个未使用局部变量、4 个缺少花括号、4 个跨异步间隙使用 `BuildContext` 但检查了无关的 `mounted`。
- 影响：当前不构成编译失败，但增加误维护与生命周期风险。
- 建议：逐项修正，保持行为不变。
- 验证：`flutter analyze` 实际输出 9 issues，退出码 1。

### EN-29｜P3｜疑似问题：FFI 单 worker 串行派发存在长调用排队风险

- 位置：`flutter_app/lib/bridge/lanvault_bridge.dart:1953`、`flutter_app/lib/bridge/lanvault_bridge.dart:2055`
- 证据：worker 同步执行 FFI，注释明确“串行派发”；heavy 隔离仅包含扫描、文件推送、快照恢复和校验。
- 误判排除：这可能是避免全局状态锁竞争的有意设计，不能仅凭串行判定为 bug。
- 影响：若存在未分类的长耗时 FFI，进度、配对或环境轮询会排队。
- 建议：先测量调用时长，再决定是否扩大 heavy 分类；无数据时不要重构。
- 验证：静态疑似；未做性能基线。

## 5. 误判与设计边界

以下候选问题经核查后不列为缺陷：

| 候选结论 | 最终判定 | 证据与理由 |
| --- | --- | --- |
| Android 自启动绕过“随软件启动”开关 | **误判** | `crates/lanvault-ffi/src/api.rs:2303` 在 Rust 内读取开关，值为 `0` 时直接返回；Dart 构造函数调用并不代表绕过。 |
| 局域网仍使用明文 HTTP | **误判** | `crates/lanvault-server/src/tls.rs:110` 仅允许回环连接协商明文 HTTP，非回环必须 TLS；原生客户端在 `crates/lanvault-client/src/transport.rs:69` 校验设备公钥指纹。 |
| 关闭自启动会清除最近配置，导致无法恢复 | **误判** | `crates/lanvault-ffi/src/api.rs:2382` 只切换开关，不清除 `last_server_config`；这是保留配置、停止自动拉起的合理设计。 |
| pairing URI 经 `innerHTML` 造成 XSS | **误判** | `web-ui/js/qr.js:7` 只把 URI 送入 `TextEncoder`，SVG 插值仅含数字坐标与固定属性，见 `web-ui/js/qr.js:142`。 |
| 会话令牌存入 localStorage | **误判** | `web-ui/js/api.js:11` 只保存锁定截止时间，不保存令牌。 |
| groups/rules/mirror-roots 写接口缺少管理员校验 | **误判** | 所有变更 handler 均有 `require_admin`，如 `crates/lanvault-server/src/web/groups_routes.rs:248`、`:314`、`:346`、`:395`、`:447`、`:497`、`:556`、`:650`、`:708`、`:812`、`:881`、`:928`。 |
| Vault 授权接口允许 peer 给自己扩权 | **误判** | `handle_grant_vault_access` 在写入前调用 `require_admin`，见 `crates/lanvault-server/src/web/mod.rs:1235`。 |
| Vault 创建、校验触发、调试配置或配对请求管理允许 peer 越权 | **误判** | 这些 handler 均有 `require_admin`，见 `crates/lanvault-server/src/web/mod.rs:1269`、`:2100`、`:2730`、`:2750` 与 `crates/lanvault-server/src/web/debug_routes.rs:24`、`:37`；chunk 上传与快照创建属于受 Vault/设备约束的数据面设计。 |
| 所有 Flutter 异步回调都缺少 mounted 检查 | **误判** | Vault、Dashboard、Clipboard、Pairing 等主要路径已有检查。 |
| IndexedStack 每次切页都重建并丢失状态 | **误判** | `flutter_app/lib/main.dart:514` 复用页面列表，属于保留状态的设计。 |
| Android 开机后直接启动 dataSync 前台服务 | **误判** | `BootReceiver.kt:69` 对 Level 0 只安排 WorkManager，符合平台限制。 |
| 传输页静默刷新会覆盖本地乐观状态 | **误判** | `_outbox.addEntries()` 只更新已有客户端消息 ID，不覆盖新增服务端消息。 |
| 固定 QR mask 0 不合法 | **误判** | mask 0 合法；EN-02 的疑点在 Version 6 分块与格式位，不在固定 mask。 |
| 旧快照恢复、GC、镜像清单或明文下载问题仍原样存在 | **误判** | 当前已有事务化快照、按 rule/device 的 GFS、清单根哈希校验、镜像清单备份、索引恢复、显式密文块下载编码等实现。 |

## 6. 验证结果

本次只运行项目已有构建、测试与静态检查命令，未修复源码。

| 检查 | 结果 | 说明 |
| --- | --- | --- |
| `cargo check --workspace --all-targets` | 通过 | 仅 1 个 Cargo 未使用 `rust-version` 字段告警。 |
| `cargo test --workspace --all-targets` | 通过 | 21 + 13 + 2 + 5 + 3，共 44 项测试通过。 |
| `flutter analyze` | **失败** | 9 项告警，详见 EN-29。 |
| `flutter test` | 通过 | 8 项 Widget 测试全部通过。 |
| Android `assembleDebug` | 通过 | 三 ABI Debug APK 构建成功；有 Gradle/Kotlin 弃用告警。 |
| `node --check web-ui/js/*.js` | 通过 | 5 个 Web 脚本语法检查通过。 |
| 源码工作树 | 干净 | 本次仅新增本报告，未修改实现文件。 |

未完成的验证：

- 未运行真实浏览器故障注入、快速切换竞态复现、读屏或键盘可用性实测。
- 未使用独立二维码解码器或手机相机验证配对二维码。
- 未在 Android 真机验证 Root、Shizuku、Level 0 保活、权限永久拒绝、Android 15 超时与厂商省电策略。
- 未进行数据库丢失、磁盘掉电、离线镜像卷等故障注入。
- 未验证 Release 签名、发布包完整性与跨 CPU 分发兼容性。

## 7. 建议处理顺序

1. 先修 EN-01 至 EN-04：它们直接影响首次配对与核心状态可信度。
2. 再修 EN-05 至 EN-14：补授权续期边界、公开接口限流、前端错误态与竞态。
3. 处理 EN-15 至 EN-22：可访问性、Android 生命周期与恢复语义。
4. 最后清理 EN-23 至 EN-29，并建立能覆盖平台桥接与 Web 行为的回归测试。

本报告中的“确认问题”均要求给出代码触发路径；“疑似问题”明确保留了尚未完成的运行验证，不能当作已复现缺陷。
