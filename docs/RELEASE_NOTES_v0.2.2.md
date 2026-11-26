# EveryoneNAS v0.2.2

本次发布是一次修复型更新：修掉了 2026-11-26 设计审计中确认的全部 P0，以及大部分 P1/P2。审计方法、评分与线索见 `docs/DESIGN_AUDIT_2026-11-26.md`。

## 破坏性变更

- **Web 管理台已归档。** 静态控制台（`web-ui/`）不再编入客户端二进制，源码移到 `archive/web-ui-2026-11-26/`。局域网内不再能用浏览器打开管理页；服务端保留 `/api/*` 与 `/ws`，Flutter 端与第三方客户端不受影响。移除了 `lanvault-server` 的 `embed-web-ui` feature 与 `static-serve` 依赖。

## 修复

### 数据与契约

- **建镜像池必然失败**：App 提交 `'raid1'`，后端只接受 `'mirror_raid1'`。新增 `PoolMode` 枚举承载 wire 值，`createVault` 全链路改为强类型，漂移从此是编译错误。
- **多盘串联模式不可达**：后端 `Striped` 的读写路径早已实现，App 没有入口。补上第三档「多盘串联」，附加卷校验按模式区分（镜像需异盘，串联只要求非空）。
- **Web 密码开关是伪状态**：之前只改本地布尔并提示「已启用」。新增后端命令 `clear_web_password`（停服 + 删除加密留档密码），关闭走真实往返并需二次确认，失败时不改开关。

### 主题与可读性

- **亮色皮肤文字与底色同色**：`mixColor` 把 `amount` 钳在 0..1，而亮色分支用 `-0.88` 求深墨色，结果返回 canvas 本身（对比度 1.00）；同时通道按 0..1 取值却传给收 0..255 的 `Color.fromARGB`，派生色整体退化为近透明。两处一并修正，负值改为显式 `_darken`。
- **次级文字不达 AA**：`textSecondary` 原本是固定 52% 回混、不过任何守护。现在按实际承载面（surface）守护到 4.5:1，`slateMuted` 守护到 3:1。
- **强调色上的文字可能不达标**：中灰 accent（如 `#887B3F`）白/墨两选一都到不了 4.5。新增 `readableAccentPair`，必要时把 accent 本身压深/提亮到成对通过。
- 新增回归测试覆盖 **79 套皮肤 × 亮暗两档** 的正文、次级文字与主按钮对比度。

### 交互与无障碍

- **破坏性操作零确认**：新增 `confirmDestructive()`，覆盖删除组规则、删除备份任务、恢复快照、解绑设备、移除成员、关闭 Web 密码；正文写明后果，危险按钮统一 danger 语义。
- **可点卡片无声语义**：`NeuCard` 原先只在传了 `semanticLabel` 时才包 `Semantics`，9 处可点卡片仅 1 处带标签。现在 `onTap` 非空即强制 `button: true`，标签回退到卡片文本。
- **焦点不可见**：`NeuButton`/`NeuSwitch`/`NeuSegmentedControl` 都没有焦点视觉（只有 `NeuCard` 有）。三者统一补描边，键盘用户能定位焦点。
- **设置页开关读屏不友好**：8 个开关用裸 Material `Switch`，标题与开关是两个节点。改用 `NeuSwitch`，标题/副标题/状态合并为一个可朗读节点。
- **选中态只靠颜色**：剪贴板设备行、快照列表行补 `Semantics(button, selected)`。

### 状态真实性

- **加载失败被当成「服务未启动」**：总览把 `overview == null` 一律渲染成「点击进入设置启动」。现在区分「正在读取 / 读取失败 / 未启动」三态，存储池空态按失败/为空给出不同按钮。
- **空态不可行动**：总览的「暂无最近活动」「暂无存储池」补上行动按钮。

### 性能与生命周期

- **轮询不感知前后台**：总览（6s）、剪贴板（2s）、配对（5s）三个定时器退到后台仍按原频率打 FFI。三者接入 `WidgetsBindingObserver`，后台停止、回前台立即刷新一次。
- **路由退场动画失效**：`Interval` 末端被连续两次除以 `1e6`，比值趋近 0。改为按进出场时长比例计算并钳在 1.0。

### 工程

- 清理 `lanvault-server/src/web/mod.rs` 的 Web UI 嵌入分支与注释。
- 修复设置页 4 处 `use_build_context_synchronously` 与 3 处缺花括号的 lint；`flutter analyze` 归零。

## 验证

- `cargo test --workspace`：49 项通过，0 失败。
- `flutter analyze`：No issues found。
- `flutter test`：17 项通过（新增 1 项皮肤可读性回归）。
- Windows 与 Android Release 均由 `scripts/build_*_release.ps1` 构建，开启混淆与符号剥离。

## 已知限制

- 审计里 395 条 `file:line` 线索中，P3 清理项（死令牌、术语表统一、原始枚举映射、列表虚拟化）尚未处理，见 `docs/design-audit-2026-11-26.index.md`。
- 皮肤对比度按算法与抽样验证，未在真机上逐套目视确认。
- 归档的 `web-ui/` 不随本次二进制发布，需要时请从 `archive/web-ui-2026-11-26/` 自取。
