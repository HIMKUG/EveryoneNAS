# EveryoneNAS 前端设计审计报告（Web 控制台 + Flutter 双端）

审计日期：2026-11-26（Asia/Shanghai）

审计基准：`D:\code\nas\EveryoneNAS-Merge`，分支 `integration/best-of-both`（未提交工作树），Flutter 包 `lanvault_app 0.2.0+2`。

方法：以 `draft-to-opus` 的三阶渐进审查（叶级信号 → 中观归并 → 宏观就绪）为骨架，取 `impeccable` 的 craft-floor 验证项、Nielsen 十启发式、认知负荷八项、五画像与 P0–P3 分级为判据。全程只读，未修改任何源码。

审计对象与规模：

| 端 | 文件 | 行数 |
| --- | --- | --- |
| Web 控制台 | `web-ui/index.html` + 2 CSS + 5 JS | 4 999 |
| Flutter | `flutter_app/lib/**`（20 个 Dart 文件） | 20 573 |

两端比约 1 : 4.1。Flutter 前 5 个文件占全库 60.9%（`lanvault_bridge.dart` 4 458 行、`backup_screen.dart` 2 549、`neu_theme.dart` 2 249、`settings_screen.dart` 1 727、`snapshots_screen.dart` 1 543）。

---

## 1. 评分总表

按 `draft-to-opus` 的 100 分健康度量表打分，两端分列（同一量表的维度权重固定，便于横向比较）。

| 维度 | 权重 | Web 控制台 | Flutter 双端 | 关键证据 |
| --- | --- | --- | --- | --- |
| UI 控件收敛度 | 15 | 6 | 11 | Web：29 个手写 `<button>` 直接拼装；Flutter：15 个 `Neu*` 组件 + `NeuButton` 复用 106 次 |
| 反补丁与中观模块化 | 15 | 5 | 8 | Web：`app.js` 2 173 行 / 55 函数承载 5 块面板，单例 `state` 31 字段被引用 164 次；Flutter：桥接单类 264 个异步方法、3 处「新增 9 个…」注释、同一分区标题出现两次 |
| 性能与数学健康度 | 20 | 10 | 12 | Web：16 处 `textContent = ''` 整块重建、19 个 live 区、零虚拟化；Flutter：0 处 `ListView.builder`、7 个轮询器无生命周期门控、11.4 MB 单字重字体 |
| 架构秩序与韧性 | 20 | 9 | 12 | Web：仅 4 类请求有序号守卫，vaults/peers/groups 3 类裸刷新；Flutter：状态三态混同（加载/失败/空）、7 个屏的错误只发一次性 toast |
| 安全、数据完整与可观测 | 20 | 12 | 13 | 两端：删除/解绑/恢复类操作零确认；App 建库 `poolMode:'raid1'` 与后端 `mirror_raid1` 不符；Web E2EE 复选框可取消但服务端强制 true |
| 发布就绪、依赖与可测性 | 10 | 3 | 6 | Web：0 个测试文件、无 `color-scheme`/`prefers-color-scheme`；Flutter：测试桩 56 个空实现 + 3 个 `UnimplementedError`，无 CI |
| **合计** | **100** | **45** | **62** | Web「差」、Flutter「可接受」 |

判定档位：0–49 重大返工；50–69 可接受但需实质整改；70–84 良好；85–100 优秀。

补充评分（`impeccable audit` 的 5 维 0–4 制，只衡量可验证项）：

| 维度 | Web | Flutter | 说明 |
| --- | --- | --- | --- |
| 无障碍 A11y | 1 | 2 | Web：模态只声明不约束、6 处裸 InkWell、焦点管理全站 2 处 `.focus()`；Flutter：组件语义齐全但 9 个屏只有 0–3 处 `Semantics(` |
| 性能 | 2 | 2 | 见上表 |
| 主题 | 1 | 4 | Web 无暗色、无皮肤、2 个死令牌；Flutter 78 套皮肤 + 亮暗成对 + 对比度守护 |
| 响应式 | 2 | 3 | Web 仅 900/600 两断点；Flutter 统一 `breakpointWide = 840` 覆盖 8 屏 + 外壳 |
| 实现完整性 | 2 | 3 | 两端皆有「能力已实现但前端未接入」的点 |

---

## 2. 五条主线（先看这里）

**主线一：两端投入严重不对称，Web 端是「能用」而非「做完」。**
Web 4 999 行要覆盖与 Flutter 20 573 行相同的 5 个功能块。后果是可量化的：0 个 `@keyframes`、1 条 `:hover` 规则、2 处 `transition`、0 处 `color-scheme`、0 个测试。Flutter 端则相反——15 个可复用组件被复用 48 次、78 套皮肤、亮暗双调板。

**主线二：组件库健全，页面装配层没跟上。**
Flutter 的 `NeuButton` / `NeuSwitch` / `NeuSegmentedControl` 自带语义与键盘路径，但页面层大量绕开：设置页 8 个开关另写 `_SettingsSwitchRow`（裸 `Switch`、无 `MergeSemantics`）、11 处 `InkWell` 中 6 处无 `Semantics`、`NeuCard` 9 处 `onTap` 仅 1 处带 `semanticLabel`。修组件默认值比逐页补标签便宜得多。

**主线三：状态与失败被同一种 Toast 兜住。**
Flutter 侧 57 处 `NeuToast.error`、12 处「重试」文案（总览/快照/备份三屏为 0）；`NeuToast` 本身不支持动作按钮。Web 侧 20 处 `disabled`、0 处 `aria-busy`；16 处清空重建配 19 个 live 区。结果：失败一闪而过，用户无从恢复。

**主线四：破坏性操作零确认是两端共性。**
Flutter：删组规则、删备份任务、恢复快照、解绑设备、移除成员、停服、清日志、清网络限制、重置皮肤——全部单击即执行；全库仅 6 处 `NeuButtonTone.danger`。Web：删组、吊销信任、移除成员、登出、重启——全部无确认，`bindPressAction` 26 处复用只做 `disabled` 守卫。

**主线五：跨端能力漂移（有三处已构成功能缺陷）。**
① App 建库段值 `'raid1'` 直达 `PoolMode::parse`，后端只认 `mirror_raid1` → 镜像池必失败；② 后端已实现 `Striped`（`vault.rs:356` 有条带写入），App 无可选入口；③ Web 提供可取消的 E2EE 复选框，`web/mod.rs:1282` 却强制 `e2ee_enabled == true`，取消即报英文错。

---

## 3. 问题清单（按主题归并）

格式：`[编号] 标题 · 严重度 · 实例数 · 证据 · 影响 · 修复方向`。

### 3.1 功能与契约（最优先）

**[F1] App 建镜像池必然失败** · P0 · ×4
证据：`vault_screen.dart:259`（段值 `'raid1'`）、`:262`、`:542`；`lanvault_bridge.dart:3908`（`trim()` 直传）；`crates/lanvault-core/src/db.rs:28-40`（只接受 `single`/`striped`/`mirror_raid1`）。
影响：用户选「双盘镜像」→ 后端 `invalid pool_mode`，功能完全不可用，且测试桩 `widget_test.dart:171` 直接抛 `UnimplementedError` 无人发现。
修复：引入 `enum PoolMode { single, striped, mirrorRaid1 }` 并集中映射到 wire 值；桥接层拒绝未知值。

**[F2] 条带模式在后端已实现但 App 不可达** · P1 · ×3
证据：`crates/lanvault-server/src/vault.rs:356`、`:436`（读写两路都走 `striped_target_volume`）、`api.rs:4318`；App 端零处 `striped`。
影响：扩容能力对移动端/桌面端用户不存在，同一 NAS 在 Web 能建、在 App 不能。
修复：同 F1 的枚举落地后补第三个段值。

**[F3] E2EE 复选框可取消，服务端强制必须为真** · P1 · ×4
证据：`web-ui/index.html:220`（可勾选 checkbox）、`web-ui/js/app.js:1896`（原样提交）、`crates/lanvault-server/src/web/mod.rs:1282`（`if !req.e2ee_enabled` 直接报错）、`vault_screen.dart:182`（App 写死 `true`）。
影响：用户在 Web 取消勾选 → 提交失败并显示英文技术串；App 端则根本没有选择权，与「主密钥备份」这套加密叙事不符。
修复：策略下沉到核心层或统一为不可关闭；若保留可选，则两侧都要真正支持。

**[F4] `poolMode` 走裸 String，类型系统拦不住上述漂移** · P2 · ×7
证据：`lanvault_bridge.dart:430`、`:1718`、`:2666`、`:3894`；`vault_screen.dart:160`、`:184`、`:262`。
影响：字面量散落 7 处，改一处漏一处，缺陷靠人工 review 兜。
修复：与 F1 同批，用枚举替换字符串参数。

**[F5] 皮肤 id 字面量重复而非引用常量** · P3 · ×4
证据：`appearance_controller.dart:44`、`:129`、`:190`；常量定义在 `skins.dart:22`（`defaultSkinId`）。

### 3.2 交互安全（破坏性操作）

**[C1] Flutter 破坏性操作零确认** · P1 · ×9
证据：`backup_screen.dart:467`（删组规则）、`:612`（删备份任务）、`snapshots_screen.dart:319`（恢复直接写盘）、`pairing_screen.dart:171`（解绑）、`:461`（移除成员）、`settings_screen.dart:87`（重置皮肤）、`:737`（重启服务）、`:787`（清空网络限制）、`:926`（清空日志）。
影响：单击即不可逆；`danger` 语义全库仅 6 处，危险按钮与普通按钮视觉同权。
修复：抽一个 `confirmDestructive(context, title, body, {requireTyping})`，正文写清「绑定会停用/数据保留」，默认焦点在取消。

**[C2] Web 破坏性操作零确认且无撤销** · P1 · ×6
证据：`app.js:1990`（删组）、`:723`（吊销信任）、`:855`（移除成员）、`:2055`（登出）、`:1813`（重启）、`index.html:389`。
影响：与 C1 同源；且无任何 undo/回收站。
修复：同上，Web 侧可复用 `<dialog>` + 30 秒撤销条。

**[C3] 恢复流程无进度、不可取消、无结果校验** · P1 · ×4
证据：`snapshots_screen.dart:319-328`（选完目录即执行）、`:324`（仅一条会消失的 toast）、`:317`（恢复中静默丢弃点击）；`app.js:626-634`（`anchor.click()` 后立即提示「已触发回滚下载」）。
影响：大快照耗时期间用户无法判断进展；Web 端 401/404 也显示成功。
修复：恢复前确认（来源/大小/目标），恢复中给可取消进度，结束后回读结果。

**[C4] 提交中弹窗可被遮罩关闭** · P2 · ×20
证据：`neu_theme.dart:919`（`barrierDismissible = true` 为默认）；21 处 `showNeuDialog` 中仅 `main.dart:428` 显式设为 `false`。
影响：提交中断、已填内容丢失。
修复：默认改 `false`，或由调用方在 `submitting` 时切换。

### 3.3 状态完整性（加载/失败/空）

**[S1] Flutter 加载中/失败/空三态混同** · P0 · ×6
证据：`dashboard_screen.dart:322`（`serverPort == null` 即渲染「点击进入设置启动」）、`:417`、`:89-94`；`pairing_screen.dart:1295`（`info == null` 一律「服务未启动」）、`vault_screen.dart:514`（`volumes.isEmpty` 被当作「全部在线」）。
影响：网络或后端故障被误报为「服务未启动 / 没有数据」，用户按错误提示去操作。
修复：显式 `error` 分支 + 重试；空态只在「成功返回空集」时出现。

**[S2] Web 失败退化为旧数据或空列表** · P1 · ×5
证据：`app.js:483`、`:764`、`:1054`（错误与空态同一样式）、`:1690`、`:1696`（刷新失败保留旧值且无行内错误）。
影响：用户分不清「没有」与「取不到」。
修复：容器内错误占位 + 重试按钮，与空态视觉区分。

**[S3] 错误只发一次性 Toast，无就地重试** · P1 · ×57
证据：`neu_toast.dart:15-31`（四类静态方法，无动作参数）；`features/**` 57 处 `NeuToast.error`，仅 12 处「重试」文案，总览/快照/备份为 0。
影响：错过 toast 就没有第二次机会。
修复：给 `NeuToast.error` 增加可选 `action`，长耗时失败改为行内错误区。

**[S4] Web 整块容器当 live 区** · P2 · ×10
证据：`index.html:164`、`:178`、`:257`、`:263`、`:296`、`:328`、`:345`、`:368`、`:373`、`:428`；配合 `app.js` 16 处 `textContent = ''` 重建。
影响：每次刷新读屏整段朗读列表。
修复：live 区只放一句状态摘要，列表本体移除 `aria-live`。

**[S5] 无 loading / 无 `aria-busy` 的刷新按钮** · P2 · ×7（Web）+ ×2（Flutter）
证据：`app.js:1804`、`:1828`、`:1922`、`:1952`（无 pending）；全站 0 处 `aria-busy`；`vault_screen.dart:402`（仅空列表才显加载）。
影响：连点产生并发请求，旧响应覆盖新数据。
修复：`withPending(button, fn)` 统一包裹（Web 侧 26 处 `bindPressAction` 是唯一改动点）。

**[S6] 设置页 Web 密码开关是本地伪状态** · P0 · ×2
证据：`settings_screen.dart:645`（关闭只改布尔 + 提示「已关闭提示」）、`:589`（无服务时直接 `setState(true)` 报「已启用」）、`:1302`。
影响：安全开关给假保证——用户以为受保护，实际服务端密码未变。
修复：走桥接启停并回读真实状态，失败回滚开关。

### 3.4 无障碍

**[A1] 可点元素缺按钮语义** · P1 · ×10
证据：`neu_theme.dart:818-824`（`NeuCard` 仅在传 `semanticLabel` 时包 `Semantics`）；features 9 处 `onTap` 仅 1 处带 label；`vault_screen.dart:350`（「点击启用」徽标挂在裸 `GestureDetector`）；`clipboard_screen.dart:346`、`pairing_screen.dart:954`（整行切换目标设备，无 button/selected）；`settings_screen.dart:1601`、`backup_screen.dart:2515`（FAB 无 Focus）。
影响：读屏听到无角色的文本块，键盘无法激活。
修复：`onTap != null` 时强制包 `Semantics(button: true, label: ...)`，label 回退到子文本。

**[A2] 焦点可见性只覆盖 2/5 可交互组件** · P0 · ×3
证据：`neu_theme.dart:738-754`（`NeuCard` 有描边）；`:1156`（Button 的 `Focus` 无装饰）、`:1318`（Switch 同）、`:1488`（SegmentedControl 同）；`:1657`（`NeuInsetField` 有描边）。
影响：键盘用户无法定位焦点。
修复：把 Card 的焦点描边逻辑抽成装饰器，四类组件统一套用。

**[A3] 焦点管理几乎缺位** · P1 · ×6
证据：Web 全站仅 2 处 `.focus()`（`app.js:358` 登录框、`timeline.js:153` 回滚输入）；`app.js:792`、`:855`、`:1098`、`:1990`（重建后焦点落回 body）；`index.html:16` 声明 `aria-modal` 但 JS 0 处 `inert`/焦点陷阱。
影响：键盘与读屏用户每次操作后被抛回页首。
修复：模态实现真实陷阱；列表更新后把焦点还原到对应新节点。

**[A4] 开关行语义未合并** · P1 · ×8
证据：`settings_screen.dart:1527-1578`（`_SettingsSwitchRow` 用裸 `Switch`，标题与开关是两个节点，全库 0 处 `MergeSemantics`）。
影响：读屏先读标题，再读一个无名开关。
修复：改用同项目已有的 `NeuSwitch(title:, subtitle:)`（`backup_screen.dart:1102` 的写法是正确范式）。

**[A5] 屏级语义覆盖极不均** · P2 · 梯度 26:0
证据：`semanticLabel` 出现次数——备份 26、配对 16、快照 8、设置 2、存储 1、总览 0、剪贴板 0。
影响：同一产品内无障碍体验断崖。
修复：以备份/配对为模板回填其余屏。

**[A6] 对比度不足（实测）** · P1 · 两端各 3 处
Web（背景 `#E6E9EF`）：`--lv-status-warn #B45309` = **4.13**（14px 粗体仍需 4.5）、`--lv-status-ok #0F766E` = **4.50**（零余量）、`--lv-status-ok-alt #15803D` = **4.12**；边框/凹槽与底色仅 **1.33–1.40**（`#C5CCD8`、`#DCE1EA`）。
Flutter（默认手工皮肤，白底）：`healthy #10B981` = **2.54**、`warning #D97706` = **3.19**、`danger #EF4444` = **3.76**；`textSecondary #64748B` 在画布 `#F3F6FC` 上 = **4.40**。
影响：状态文字（尤其 Web 顶部「连接中…」）与小号次级文本不达 AA；App 状态图标低于非文本 3:1。
修复：Web 提高 warn/ok 亮度对比；Flutter 让 `textSecondary` 与状态色也过一遍已有的 `ensure*` 守护（`skins.dart:481` 目前是固定 0.52 混色，未过守护）。

**[A7] 禁用态两端不一致且 Web 不可读** · P2 · ×2
证据：Web `neumorphism-tokens.css:165`（`opacity: 0.52`，等效对比 **2.48**）；Flutter `disabledLook` 用深底浅字（**9.85**）。
修复：Web 改为实色 `--lv-surface-sunk` + `--lv-ink-secondary`。

**[A8] 触控目标低于自家基线** · P2 · ×5
证据：`neu_theme.dart:169-170`（`minTouchTarget = 42`、`minMobileTouchTarget = 46`，低于 Apple 44 / Material 48）；`:1505` 分段控件高 38；`:1340` 紧凑开关约 36；Web `--lv-touch-min: 44px` 但 `.neu-btn--compact` 只有 14px 字号配 12px 内距。
影响：单手移动端误触；WCAG 2.5.8 的 24px 底线满足，属设计取舍而非违规。

**[A9] 大字缩放无兜底** · P2 · ×3
证据：全库 0 处 `textScaler`、0 处 `FittedBox`；`dashboard_screen.dart:347`、`:384`、`:484`（固定图标 + `headlineSmall` 数字）。
影响：系统字号 2.0 时三列 KPI 溢出。

**[A10] 键盘激活缺失（Web）** · P1 · ×1
证据：`timeline.js:167`（只监听 `pointerup`）、`:175`（手写 `keydown`）；AT 只派发 `click`，回滚面板对读屏用户完全不可用。

### 3.5 竞态与性能

**[R1] Web 三类刷新无请求序号** · P1 · ×3
证据：`app.js:1690`（`refreshVaults`）、`:1696`（`refreshPeers`）、`:1241`（`refreshGroups`）——对照已有守卫的四类（`:131-143` 的 `*RequestSeq`）。
影响：吊销设备后旧列表晚到会回填。
修复：复用现成的序号模式。

**[R2] 快传列表每条状态帧全量重建** · P1 · ×1
证据：`app.js:1552`（`renderClipboardFeed` 全量）、`:1564`（每次全量排序）。
修复：按 `client_msg_id` 局部更新单卡片。

**[R3] 无限增长与无虚拟化** · P2 · ×6
证据：`app.js:599`（巡检日志无上限）、`:1418`（`clipboardMessages` Map 无淘汰）；`backup_screen.dart:812`（`ListView(children: [...])`）、`vault_screen.dart:465`/`:479`、`snapshots_screen.dart:782`；全库 0 处 `ListView.builder`。

**[R4] 轮询无生命周期门控** · P2 · ×7
证据：`lanvault_bridge.dart:2235`（2s）、`:2395`（1s）、`:3177`（15s）、`:3181`（2s）；`clipboard_screen.dart:44`、`dashboard_screen.dart:55`（6s）、`pairing_screen.dart:51`。仅设置页与主题控制器实现 `WidgetsBindingObserver`。
影响：后台持续 IPC 与耗电（移动端尤甚）。
修复：统一 `AppLifecycle` 暂停/恢复。

**[R5] 数据新鲜度不可见** · P2 · ×1
证据：`dashboard_screen.dart:55-58` 每 6s 静默刷新，界面无「更新于」。
修复：状态卡补相对时间，离线时显式标注。

### 3.6 令牌与视觉工艺

**[T1] 排版未令牌化** · P2 · ×37
证据：全库 `fontSize:` 37 处（主题层 17、屏层 20），实际取值 11/12/13/14/15/16/20/23/24 九档；`TextTheme` 仅 7 档（12/13/14/16/24，`neu_theme.dart:384-421`）；Web `font-size` 37 处裸写、8 档。
影响：层级比例靠就近取值，无统一节奏。
修复：补 `labelSmall/titleLarge/headlineMedium` 与 `display` 档，屏层全部改引用。

**[T2] 字体：单字重 11.4 MB，两端各带一份** · P1 · ×4
证据：`web-ui/assets/fonts/SourceHanSansSC-Regular.woff2` = 11 428 300 B，与 `flutter_app/assets/fonts/` 同一文件（md5 均为 `321e41b6…`）；`pubspec.yaml` 无 `fonts:` 声明，靠 `main.dart:32-42` 的 `FontLoader` 解码；Web 端无 `preload`，`font-display: swap`。
影响：首屏要等 11 MB；且全库用 `w400` 之外的 `w500/w600/w700/w800` 共 44 处，浏览器只会做合成加粗（笔画变糊）。
修复：按字号/字频子集化（预计可降到 1–2 MB），或改用系统中文栈；字重改为变量字体或提供 500/700 两份子集。

**[T3] 动效令牌存在但未贯彻** · P2 · ×7
证据：Flutter 有 5 条曲线 + 6 档时长（`neu_theme.dart:26-39`），屏层仍散落 200/300/2800/500ms（`clipboard_screen.dart:189`、`main.dart:106`、`neu_toast.dart:35`、`windows_title_bar.dart:284`）；`motion_slide.dart:51`、`:255` 引入第 6/7 条曲线。Web 端 `--lv-dur-press` 贯穿但只有 2 处 `transition`。
修复：屏层裸值全部改引用；新增曲线必须先入 `NeuMotion`。

**[T4] 路由退场动画实际失效** · P2 · ×1
证据：`motion_slide.dart:283-295`——`Interval` 末端被连续两次除以 `1e6`，比值趋近 0。
影响：出页淡入看不到效果，动效语言断掉一半。

**[T5] 减弱动效未完全降级** · P2 · ×2
证据：`motion_slide.dart:239-267`（reduced 仍横滑 100ms）；`theme_reveal.dart:80-92`（半径用全局坐标，宿主非全屏时扩散边界错位）。
正面：`theme_reveal.dart:58-63` 与 `neu_theme.dart:50-52` 的 reduced 判定是真实开关，非空实现。

**[T6] 默认皮肤是薄弱环节** · P2 · ×3
证据：`skins.dart:800-815`、`:870-882`（两套手工皮肤用 `copyWith` 覆盖 12 个字段，**不复查对比度**）；而派生皮肤（`monoSkin`/`duoSkin` → `buildDerivedPalette`）对 accent/line/状态色都调 `ensure*` 守护。结果：默认「柔冰蓝」的 `healthy 2.54` 反而比 76 套派生皮肤更差。

**[T7] 死令牌与内联色** · P3 · ×15
证据：41 个 `--lv-*` 中 `--lv-control-outline`、`--lv-status-ok-alt` 零引用；13 处内联 `rgba(...)` 是语义色的透明度变体（`rgba(185,28,28,.4/.45)` 各出现一次），无 alpha 令牌。

**[T8] 微小一致性** · P3 · ×4
证据：`Icons.workspaces_outline`（应为 `_outlined`，`pairing_screen.dart:711`）、两处无变体 `Icons.fingerprint`；`backup_screen.dart:63-64` 连续两次 `super.initState()`；`settings_screen.dart:27` 注释写「左侧 5 项」而实现是 6 项。

### 3.7 文案与术语

**[W1] 术语跨屏不一致** · P2 · ×20+
证据：快照/版本/Rev（`snapshots_screen.dart:681`）、校验/巡检/修复（`:1153`）、备份任务/备份目录/绑定/镜像根/异盘（`backup_screen.dart:720`、`:1617`）、皮肤/外观/主题（`settings_screen.dart:197`）、「双日夜调板」这类生僻自造词（`:197`）。
修复：建一份术语表，UI 只引其值。

**[W2] 后端原始值直出界面** · P2 · ×7
证据：`app.js:937`、`:947`（状态码与 `last_error` 原文）、`timeline.js:115`；`snapshots_screen.dart:1208`、`:1445`、`:1459`（`scrub.status`、`archiveReason`、`snapshotId`）；`settings_screen.dart:1420`（「Debug」中英混排）。
正面：`lanvault_bridge.dart:3365-3420` 有 14 条中文映射，但未命中即原样透出（`:3420`）。
修复：未知值统一映射为「未知状态」，原始串折叠到「详情」。

**[W3] 提示信息藏在不可发现处** · P3 · ×3
证据：`index.html:343`（关键说明只用 `title`）；`app.js:719`（多个「吊销信任」无设备名）；`pairing_screen.dart:1311`（`------` 与真实 PIN 同色，看不出「暂无」）。

**[W4] 空态不可行动** · P2 · 9/13
证据：13 处 `NeuEmptyState` 仅 4 处传 `action`（剪贴板 2/2、存储 2/2），备份/总览/配对/快照共 9 处为纯文本；`pairing_screen.dart:939` 文案写「点击 + 配对新设备」却没有按钮。

### 3.8 架构与工程

**[E1] 单文件承载过多职责** · P1 · ×4
证据：`lanvault_bridge.dart` 4 458 行 / 264 个异步方法 / 仅 19 行文档注释；`app.js` 2 173 行 / 55 函数 / 单例 `state` 31 字段被引用 164 次；`backup_screen.dart` 2 549 行 / 15 个内部类。
修复：桥接按领域拆（保活/存储/配对/组/目录/发件箱 6 个 mixin 或组合类）；`app.js` 按 5 块面板拆文件。

**[E2] 增量补丁痕迹** · P2 · ×5
证据：`lanvault_bridge.dart:1712`、`:2639`、`:3885`（三处「新增 9 个…」）；`:1766` 与 `:2850` 同一分区标题出现两次；`app.js` 的 Block 注释顺序为 2→3→5→4。

**[E3] 数据边界无类型保护** · P2 · ×194
证据：`lanvault_bridge.dart` 194 处 `as` 强转（128 处必填 String），后端加字段即整块渲染失败；Web `app.js:449`、`:620`、`:1264`（WS payload 未校验即 `.slice()`）。
修复：DTO 用 `tryParse` 返回 `null` 而非抛错，UI 侧渲染「数据不完整」。

**[E4] 质量门禁薄弱** · P1 · ×4
证据：`flutter_app/analysis_options.yaml` 仅 `include: package:flutter_lints/flutter.yaml`，63 行全是注释模板，无自定义规则；`test/widget_test.dart` 1 205 行 / 14 个 `testWidgets` / 5 个测试替身，其中 56 处空实现、3 处 `UnimplementedError`（含 `createVault`）；Web 端 0 测试；仓库无 `.github/workflows`。
影响：这解释了「`flutter analyze` 0 issue、`flutter test` 5 passed」与本次 2 个 P0 并存。
修复：启用 `use_build_context_synchronously`/`avoid_dynamic_calls` 等；为 F1/F3 补契约测试；加一条最小 CI 跑 analyze+test。

**[E5] 运维文档小缺口** · P3 · ×3
证据：后端使用 `LANVAULT_CONFIG_PATH`、`LANVAULT_DAEMON_MODE`、`LANVAULT_QUOTA_GB`，`.env.example` 未列（仅 4 个 `LANVAULT_*`）。

---

## 4. 虽然丑但别乱动（豁免区）

以下判断为「高风险 × 中低收益」或「有意的设计取舍」，不建议在本轮改动：

1. **新拟态的双光源阴影系统**（`--lv-shadow-*` / `extrudedShadows`）。它与 1.19–1.40 的边框对比度是同一套取舍：靠光影而非描边区分层级。改写会动摇整个视觉世界。
2. **`minTouchTarget = 42`**。低于 Apple 44，但它与 11px 小字、紧凑卡片共同构成当前密度；单改数值会撑破多处布局。
3. **`NeuToast` 单槽替换**。队列化看似更好，但会引入「提示排队延迟」这一新问题；在 S3 修好就地重试后再考虑。
4. **`buildDerivedPalette` 的固定混色比例**。已由 `ensure*` 兜底，重写收益有限。
5. **`barrierDismissible` 默认 true**。与 C4 同源但涉及 21 处调用，风险集中在「提交中状态」这一小类，按调用点逐个改而非改默认值。
6. **Web 端「零 hover、仅 pointer 事件」**。`app.js:4` 明确声明是有意的触屏优先取舍，不是遗漏。

---

## 5. 正面发现（值得保留与复制）

1. **78 套皮肤 + 亮暗成对 + 派生守护**。`skins.dart` 用 `monoSkin`/`duoSkin` 从 seed 派生，`ensureReadableAccent`/`ensureVisibleLine`/`_ensureInkContrast` 三层守护，57 套单色无重复色值，明度覆盖 `#030100`–`#FFFBF8`。
2. **中文传统色命名体系**（玄墨/绀青/霁蓝/竹影描霜/赤火熔金），命名质量高于同类产品。
3. **15 个可复用组件，48 处跨屏复用**（`NeuProgressBar`/`NeuDropdown`/`NeuIconBadge`/`NeuKeyValueRow`/`NeuEmptyState`）。
4. **真实可用的减弱动效**：`NeuMotion.isReduced` 走 `MediaQuery.maybeDisableAnimationsOf`，`theme_reveal` 入口直接跳过，`SlideSwitchViewport` 对非当前页同时做 `ExcludeSemantics` 与 `TickerMode` 门控。
5. **设置页的失败聚合**（`settings_screen.dart:312-377`）：10 项配置并发独立加载，失败聚合成顶部横幅 + 重试，单项失败不拖垮整页——应作为其余 6 屏的范式。
6. **统一响应式断点**：8 屏 + 外壳 + toast + 设备切换器全部读同一个 `breakpointWide = 840`，且有 `_DesktopNavRail` 替代底部导航。
7. **Web 端 XSS 防护到位**：动态内容几乎全走 `textContent`/`createElement`，`qr.js:219` 的 SVG 插值只含数字坐标。
8. **Web 请求层**：统一 15s 超时 + 外部 `AbortSignal` 转发 + 429 锁定持久化（`api.js:69`、`:81`、`:139`），刷新页面无法绕过登录锁定。
9. **快传的乐观提交**：按 `client_msg_id` 幂等合并、保留已送达目标不回退、失败目标可单独重试（`app.js:1322`、`:1528`）。
10. **组件层键盘路径**：Card/Button/Switch 支持 Enter/Space，分段控件支持方向键与 Home/End 环绕。

---

## 6. 建议处理顺序

**第一批（P0，1–2 天）**：F1 池模式枚举 → F3/S6 两个「假状态」→ A2 焦点装饰统一 → S1 三态拆分。

**第二批（P1，3–5 天）**：C1/C2 确认对话框统一 → C3 恢复流程 → S3 `NeuToast` 动作支持 → A1 `NeuCard` 强制语义 → A3 焦点归还 → R1/R2 竞态与局部更新 → T2 字体子集化 → E4 lint + 契约测试。

**第三批（P2，1 周）**：W1 术语表 → W2 原始值映射 → R3/R4 虚拟化与生命周期 → T1 排版令牌 → T3/T4 动效贯彻 → S2/S4 Web 状态与 live 区 → W4 空态行动。

**第四批（P3）**：T7/T8/E5/W3 等清理项。

修复后建议重跑：`flutter analyze`、`flutter test`，并为本报告每条 P0/P1 补一条对应测试；Web 端先补最小 harness（首屏渲染 + 删除确认 + 失败重试三条用例）再谈覆盖率。

---

## 附录 · 全部实例索引

见同目录 `design-audit-2026-11-26.index.md`（按文件分组，`file:line — 问题`，共 395 条）。

同目录另有 `design-audit-2026-11-26.html`：按 `draft-to-opus` 的渐进披露模板生成的可折叠版（评分环 + 改造清单 + 五个抽屉），与本 Markdown 同源同数据，供快速浏览与转发。
