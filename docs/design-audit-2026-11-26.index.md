# 实例索引 · EveryoneNAS 前端设计审计（2026-11-26）

`file:line — 问题`。按文件分组，同文件内按行号升序。条目前缀为主题编号（见 `DESIGN_AUDIT_2026-11-26.md` 第 3 节）。

## web-ui/js/app.js（Web 控制台主控 2 173 行）

- `app.js:4` — 声明「仅 pointer 事件、零 hover」属有意取舍，非缺陷（豁免 E-6）
- `app.js:265` — `setFeedback` 覆写 `className` 会丢元素原有修饰类（W2）
- `app.js:294` — 复制兜底 textarea 移到屏外仍可短暂获焦（A3）
- `app.js:325` — 全局 pointer 反馈依赖 `pointerout`，子元素间移动频繁触发（T3）
- `app.js:342` — `bindPressAction` 26 处复用，只判 `disabled` 不承载 pending（S5）
- `app.js:350` — 锁屏未清理剪贴板等敏感 DOM（安全，已由既有审计覆盖）
- `app.js:358` — 全站仅两处 `.focus()` 之一：登录框（A3）
- `app.js:362` — 重登不重置上一会话数据，闪现旧内容（S2）
- `app.js:377` — 锁定进度条未用 `aria-valuetext`（A6）
- `app.js:404` — 200ms 定时器写样式与 aria，频率过高（R3）
- `app.js:424` — 连接状态文本无 live 播报（A5）
- `app.js:449` — `status.os.toUpperCase` 未校验存在性（E3）
- `app.js:457` — 重连成功不清重启 banner（S2）
- `app.js:464` — 重启 banner 文案每秒重写（R3）
- `app.js:471` — 重连延时 1500ms 硬编码（T3）
- `app.js:479` — `renderVaults` 全量重建（R3）
- `app.js:483` — 空态与加载失败视觉同构（S2）
- `app.js:507` — 池模式回退显示原始枚举（W2）
- `app.js:517` — 存储条仅 danger/ok 两档，与顶部胶囊阈值不一致（W1）
- `app.js:544` — 每次刷新重建快照 select，可能重置选择（R6）
- `app.js:558` — 无空间时静默清空时间线（S2）
- `app.js:589` — 附加卷「移除」无行语境可访问名（W3）
- `app.js:599` — 巡检日志无上限增长（R3）
- `app.js:610` — 时间 locale 硬编码 `zh-CN`（W1）
- `app.js:620` — `chunk_hash` 未校验即 `.slice()`（E3）
- `app.js:622` — danger banner 不自动降级（S2）
- `app.js:626` — 回滚下载无结果校验（C3）
- `app.js:634` — 下载未完成即提示「已触发回滚下载」（C3）
- `app.js:672` — 每次刷新重算并注入 QR SVG（R3）
- `app.js:674` — QR 失败降级为文本（正面）
- `app.js:710` — 设备名+OS 无长度截断（W3）
- `app.js:719` — 多个「吊销信任」按钮无可区分名称（W3）
- `app.js:723` — 吊销信任无确认（C2）
- `app.js:728` — 吊销成功后刷新失败会被误报为吊销失败（S2）
- `app.js:749` — 能力未知仅小徽标，无原因说明（S2）
- `app.js:764` — 组加载失败有独立错误节点（正面）
- `app.js:788` — `button` 上叠加 `role=listitem` 破坏按钮语义（A1）
- `app.js:789` — 单选语义误用 `aria-pressed`（A5）
- `app.js:790` — 组按钮 aria-label 含对象名（正面）
- `app.js:792` — 选中组后全量重建并丢焦点（A3）
- `app.js:816` — 重渲染覆盖正在输入的重命名内容（S2）
- `app.js:846` — 直接展示完整 device_id（W3）
- `app.js:853` — 移除成员按钮带对象名（正面）
- `app.js:855` — 移除成员无确认（C2）
- `app.js:860` — 移除后重建导致焦点丢失（A3）
- `app.js:896` — 同步模式回退原始枚举（W2）
- `app.js:937` — 绑定状态回退显示原始状态码（W2）
- `app.js:947` — `last_error` 原样展示（W2）
- `app.js:952` — 浏览按钮用 `aria-pressed` 表选中（A5）
- `app.js:958` — 打开目录面板无焦点移动（A3）
- `app.js:973` — 关闭目录面板不归还焦点（A3）
- `app.js:1012` — 为单个 `aria-pressed` 重建整棵规则树（R3）
- `app.js:1019` — catalog 请求序号守卫（正面）
- `app.js:1024` — 分页大小 100 硬编码（与 :1124 的 50、api.js:304 的 200 不一致）
- `app.js:1045` — 错误原样显示（W2）
- `app.js:1064` — 目录全量重建（R3）
- `app.js:1098` — 查看版本重建列表并丢焦点（A3）
- `app.js:1114` — versions 序号守卫（正面）
- `app.js:1124` — 分页大小 50 硬编码（W1）
- `app.js:1174` — `archive_reason` 原样拼接（W2）
- `app.js:1208` — `Promise.all` 单点失败整体失败（S2）
- `app.js:1241` — `refreshGroups` 无请求序号（R1）
- `app.js:1264` — `rule_id.slice` 无校验（E3）
- `app.js:1276` — 后台刷新异常空 `catch`（S3）
- `app.js:1322` — 保留已送达状态避免回退（正面）
- `app.js:1352` — sending 期间跳过历史合并（R2）
- `app.js:1418` — `clipboardMessages` Map 无上限（R3）
- `app.js:1441` — sending 期间丢弃 WS 终态，可能永久卡「发送中」（R2）
- `app.js:1528` — 失败目标可单独重试（正面）
- `app.js:1552` — 快传列表全量重建（R2）
- `app.js:1564` — 每次渲染全量排序（R2）
- `app.js:1607` — 加密内容禁用复制并改文案（正面）
- `app.js:1644` — 目标错误详情原样拼接（W2）
- `app.js:1690` — `refreshVaults` 无序号守卫（R1）
- `app.js:1696` — `refreshPeers` 无序号守卫（R1）
- `app.js:1706` — 批量加载用 `allSettled`（正面）
- `app.js:1720` — 状态失败提示缺重试（S3）
- `app.js:1731` — 存储失败提示缺重试（S3）
- `app.js:1737` — 设备失败提示缺重试（S3）
- `app.js:1755` — 组详情失败提示（S3）
- `app.js:1777` — 提交前复查持久化锁定（正面）
- `app.js:1795` — 429 不自行恢复按钮（S5）
- `app.js:1804` — 刷新状态无 pending（S5）
- `app.js:1813` — 重启无二次确认（C2）
- `app.js:1822` — 固定 3s 后恢复按钮，与实际无关（S5）
- `app.js:1828` — 刷新空间无 pending（S5）
- `app.js:1841` — 巡检结果按损坏数着色（正面）
- `app.js:1866` — 建库提交无锁（S5）
- `app.js:1896` — E2EE 复选框值原样提交（F3）
- `app.js:1913` — 切空间无 loading，旧快照滞留（S2）
- `app.js:1922` — 刷新设备无 pending（S5）
- `app.js:1952` — 刷新设备组无 pending（S5）
- `app.js:1961` — 建组提交无锁（S5）
- `app.js:1990` — 删除设备组无确认（C2）
- `app.js:2006` — 添加成员提交无锁（S5）
- `app.js:2032` — 收起目录不归还焦点（A3）
- `app.js:2037` — 加载更多无锁（R1）
- `app.js:2055` — 登出无确认（C2）
- `app.js:2062` — 同时写反馈行与 banner（W1）
- `app.js:2077` — 空内容静默 return 无提示（S2）
- `app.js:2118` — 异步派发不阻塞（正面）
- `app.js:2147` — 非鉴权网络故障也切登录页（S2）

## web-ui/js/api.js

- `api.js:11` — 仅保存锁定截止时间，非令牌（安全，既有审计已判误判）
- `api.js:54` — 边界解析统一（正面）
- `api.js:69` — 15s 统一超时（正面）
- `api.js:81` — 外部 abort 转发能力存在但调用方从不使用（R1）
- `api.js:118` — 超时文案可行动（正面）
- `api.js:123` — 网络错误文案可行动（正面）
- `api.js:139` — 429 写入 localStorage 持久化（正面）
- `api.js:146` — 401 触发全局回调但无提示文案（S2）
- `api.js:159` — 通用非 2xx 文案含状态码（W2）
- `api.js:242` — 调试日志 `limit` 未钳制（R3）
- `api.js:304` — 目录分页默认 200，与调用方 100 不一致（W1）
- `api.js:319` — 版本分页默认 200，与调用方 50 不一致（W1）
- `api.js:357` — 全量暴露内部 API 到 `window`（安全，P3）

## web-ui/js/timeline.js

- `timeline.js:23` — `formatBytes` 无 undefined/NaN 防护（E3）
- `timeline.js:35` — `formatTimestamp` 无非法日期防护（E3）
- `timeline.js:61` — 排序前复制，不原地改（正面）
- `timeline.js:65` — 层级筛选更新 `aria-pressed`（正面）
- `timeline.js:81` — 空态区分「无快照/筛选无结果」（正面）
- `timeline.js:99` — 事件委托处理筛选（正面）
- `timeline.js:115` — 层级标签回退原始枚举（W2）
- `timeline.js:128` — `snapshot_id.slice` 无校验（E3）
- `timeline.js:148` — 打开后聚焦输入框（正面）
- `timeline.js:153` — 全站仅两处 `.focus()` 之二（A3）
- `timeline.js:158` — 每行挂 5 个 pointer 监听（R3）
- `timeline.js:167` — 只监听 `pointerup`，无 `click`（A10）
- `timeline.js:175` — 手写 `keydown` 处理 Enter/Space（A10）

## web-ui/js/ws.js

- `ws.js:25` — 退避序列硬编码（T3）
- `ws.js:36` — connect 清理旧计时器（正面）
- `ws.js:63` — 防重复打开（正面）
- `ws.js:69` — connecting/reconnecting 状态（正面）
- `ws.js:97` — 4401/1008 视为鉴权失效（正面）
- `ws.js:104` — 1006 先探测鉴权（正面）
- `ws.js:115` — 探测超时 5s 硬编码（T3）
- `ws.js:131` — 探测失败空 `catch`（S3）
- `ws.js:141` — 退避调度（正面）
- `ws.js:155` — 畸形帧仅 `console`（S3）
- `ws.js:159` — 信封字段校验（正面）
- `ws.js:160` — 未知帧仅 `console.warn`（S3）

## web-ui/js/qr.js

- `qr.js:70` — 空 URI 抛错（边界，正面）
- `qr.js:83` — 超容量错误文案给出字节数（正面）
- `qr.js:219` — rect 拼接仅含数字坐标（安全，正面）
- `qr.js:223` — SVG 带 `role`/`aria-label`（正面）

## web-ui/index.html

- `index.html:16` — `aria-modal` 但无焦点陷阱/inert（A3）
- `index.html:18` — 标题「管理台登录」与按钮「解锁进入」措辞不一（W1）
- `index.html:34` — `role=alert` 与 `aria-live=polite` 语义冲突（A5）
- `index.html:38` — 进度条无 `aria-valuetext`（A6）
- `index.html:54` — 连接状态无 live（A5）
- `index.html:84` — 紧急告警用 `role=status`（A5）
- `index.html:86` — 「关闭」未指明对象（W3）
- `index.html:152` — QR 容器 `aria-label` 无 `role`（A5）
- `index.html:164` — 列表整体 `aria-live`（S4）
- `index.html:178` — 列表整体 `aria-live`（S4）
- `index.html:220` — E2EE 复选框可取消，服务端强制 true（F3）
- `index.html:257` — 时间线整体 `aria-live`（S4）
- `index.html:296` — `role=list` + `aria-live`（S4）
- `index.html:307` — 删除按钮嵌在重命名表单内（W1）
- `index.html:343` — 关键说明只用 `title`（W3）
- `index.html:368` — 目录列表整体 `aria-live`（S4）
- `index.html:389` — 登出按钮 danger 无确认（C2）
- `index.html:428` — 快传列表整体 `aria-live`（S4）

## web-ui/css/neumorphism-tokens.css

- `neumorphism-tokens.css:9` — 头部声称与 Flutter「1:1 令牌对齐」，但底色 `#E6E9EF` vs Flutter `#F3F6FC`、主文 `#1E293B` vs `#0F172A`（T7，声明与实现不符）
- `neumorphism-tokens.css:44` — `--lv-control-outline` 零引用（T7）
- `neumorphism-tokens.css:49` — `--lv-status-ok-alt` 零引用且 4.12:1（T7/A6）
- `neumorphism-tokens.css:50` — `--lv-status-warn #B45309` 实测 4.13:1（A6）
- `neumorphism-tokens.css:107` — 全局 `:focus-visible` 焦点环（正面）
- `neumorphism-tokens.css:165` — 禁用态 `opacity:0.52`，等效 2.48:1（A7）
- `neumorphism-tokens.css:199` — `--compact` 按钮字号 14px，密度与 44px 基线冲突（A8）
- `neumorphism-tokens.css:319` — reduced-motion 分支保留 120ms 淡入（正面，非空实现）
- `neumorphism-tokens.css:227` — placeholder 用次级墨色 6.23:1（正面）

## web-ui/css/dashboard.css

- `dashboard.css:6` — 注释声明零 hover 依赖（豁免 E-6）
- `dashboard.css:255` — `cursor:pointer` 仅 2 处，`.rule-card`/`.vault-item` 等可点卡片缺失（W3）
- `dashboard.css:265` — 全文件仅 2 处 `transition`（T3）
- `dashboard.css:517` — QR 底色写死 `#FFFFFF`（T7）
- `dashboard.css:920` — 单按钮重复定义 `focus-visible`（T8）
- `dashboard.css:991` — 仅 900/600 两个断点（响应式）
- `dashboard.css:1030` — reduced 仅覆盖 chevron（T5）
- `dashboard.css:49` — 字号 `18px` 等 8 档裸写（T1）

## flutter_app/lib/theme/neu_theme.dart

- `neu_theme.dart:26-39` — 5 条曲线 + 6 档时长令牌（正面）
- `neu_theme.dart:50-52` — reduced 走 `maybeDisableAnimationsOf`，真实开关（正面）
- `neu_theme.dart:169-170` — `minTouchTarget=42` / `minMobileTouchTarget=46`（A8）
- `neu_theme.dart:384-421` — `TextTheme` 仅 7 档，缺 10/11/15/18/20（T1）
- `neu_theme.dart:517` — donut 字号 15/22 硬编码（T1）
- `neu_theme.dart:660` — 徽章字号 12 硬编码（T1）
- `neu_theme.dart:701-771` — `NeuCard` 有焦点描边（正面，唯一完整实现）
- `neu_theme.dart:818-824` — `semanticLabel == null` 时直接返回裸容器（A1）
- `neu_theme.dart:843-858` — 折叠后内容仍在语义树，未 `ExcludeSemantics`（A5）
- `neu_theme.dart:916-948` — `showNeuDialog` 默认 `barrierDismissible=true`（C4）
- `neu_theme.dart:989` — `disabledLook` 与可点击矛盾，语义 `enabled` 取反（A7）
- `neu_theme.dart:1036-1061` — 仅中性按钮有 hover 反馈（W1）
- `neu_theme.dart:1156` — Button 的 `Focus` 无装饰（A2）
- `neu_theme.dart:1318` — Switch 的 `Focus` 无装饰（A2）
- `neu_theme.dart:1340` — 紧凑开关高约 36（A8）
- `neu_theme.dart:1488` — SegmentedControl 的 `Focus` 无装饰（A2）
- `neu_theme.dart:1505` — 分段控件高 38（A8）
- `neu_theme.dart:1565` — 字号 13.5 双写（T1）
- `neu_theme.dart:1595-1615` — `NeuInsetField` 无 `onChanged` 参数（W1，导致搜索需回车）
- `neu_theme.dart:1657` — 输入框焦点描边（正面）
- `neu_theme.dart:1726-1741` — 字号 15/14.5 硬编码（T1）
- `neu_theme.dart:2032-2122` — `NeuDialog` 无路由语义（`scopesRoute`/`namesRoute`）（A5）
- `neu_theme.dart:2051-2058` — 弹窗 88% 视口高 + 内部滚动（正面）

## flutter_app/lib/theme/motion_slide.dart

- `motion_slide.dart:51` — 引入第 6 条曲线 `easeOutCubic` 旁路 `NeuMotion`（T3）
- `motion_slide.dart:155-162` — 只动入页，出页零位移（T5）
- `motion_slide.dart:194-195` — 默认值即 reduced 时长，首次 push 只用 100ms（T4）
- `motion_slide.dart:239-267` — reduced 模式仍横滑 100ms（T5）
- `motion_slide.dart:248` — `Colors.black` 绕过调色板（T7）
- `motion_slide.dart:255` — `Curves.linear` 旁路（T3）
- `motion_slide.dart:283-295` — `Interval` 末端连除两次 `1e6`，退场动画失效（T4）

## flutter_app/lib/theme/theme_reveal.dart

- `theme_reveal.dart:58-63` — reduced 直接跳过动画（正面）
- `theme_reveal.dart:80-92` — 半径用全局坐标，宿主非全屏时错位（T5）
- `theme_reveal.dart:94-103` — 16ms 忙等轮询（R3）
- `theme_reveal.dart:131-139` — 时长曲线走令牌（正面）

## flutter_app/lib/theme/skins.dart

- `skins.dart:22` — `defaultSkinId` 常量（被 4 处字面量绕过，F5）
- `skins.dart:60-112` — `contrastRatio`/`pickContrastInk`/`ensureVisibleLine`/`ensureReadableAccent` 守护（正面）
- `skins.dart:102-112` — `_ensureInkContrast` 最多混 80%，末步直接返回（T6，尽力而为）
- `skins.dart:470-507` — `buildDerivedPalette` 对 accent/line/状态色过守护（正面）
- `skins.dart:481` — `textSecondary` 固定 0.52 混色，未过守护（A6，实测 4.40）
- `skins.dart:573-604` — `monoSkin`/`duoSkin` 派生对称（正面）
- `skins.dart:800-815` — 赤陶纸亮色 `copyWith` 覆盖 12 字段，不复查对比度（T6）
- `skins.dart:870-882` — 赤陶纸暗色同上（T6）
- `skins.dart:996-1097` — 57 套单色 + 20 套双色，无重复色值（正面）
- `skins.dart:1072-1081` — 四字意境命名（正面）
- `skins.dart:940-951` — 两套手工皮肤为 `SkinFamily.defaultSkin`（T6）

## flutter_app/lib/theme/appearance_controller.dart

- `appearance_controller.dart:44` — 皮肤 id 字面量 `'skin_default'`（F5）
- `appearance_controller.dart:101` — `kIsWeb` 早返回，但 `load()` 仍空转到超时（R4）
- `appearance_controller.dart:111` — Android 通道调用无超时（R4）
- `appearance_controller.dart:189-209` — `load()` 回写无条件，覆盖并发选择（S2）
- `appearance_controller.dart:211-247` — 切皮肤与切主题非原子（T3）
- `appearance_controller.dart:258-264` — 直接 `writeAsString`，非原子写（S2）
- `appearance_controller.dart:296-303` — 手动模式仍响应系统亮度并通知（R4）
- `appearance_controller.dart:33-49` — 非法 JSON 容错（正面）

## flutter_app/lib/theme/neu_toast.dart

- `neu_toast.dart:15-31` — 四类静态方法，无动作参数（S3）
- `neu_toast.dart:12-13` — 全局静态单槽，跨测试串扰（R3）
- `neu_toast.dart:35` — 固定 2800ms，无悬停暂停（T3）
- `neu_toast.dart:40` — 新提示先杀旧提示（R3）
- `neu_toast.dart:64-71` — 到点直接 `remove()`，无退场动画（T5）
- `neu_toast.dart:143-147` — warning 用 `folderGoldSoft` 底色，色义存疑（W1）
- `neu_toast.dart:163-166` — `Semantics(label:)` 与子 `Text` 可能重复播报（A5）
- `neu_toast.dart:167` — 仅手势点击可关闭，无键盘/语义点击（A10）

## flutter_app/lib/bridge/lanvault_bridge.dart

- `lanvault_bridge.dart:430` — `poolMode` 裸 String（F4）
- `lanvault_bridge.dart:1712` — 「新增 9 个…」补丁注释（E2）
- `lanvault_bridge.dart:1766` — 分区标题与 `:2850` 重复（E2）
- `lanvault_bridge.dart:2235` — 2s 轮询无生命周期门控（R4）
- `lanvault_bridge.dart:2395` — 1s 轮询（R4）
- `lanvault_bridge.dart:2639` — 第二处「新增 9 个…」（E2）
- `lanvault_bridge.dart:3177` — 15s 轮询（R4）
- `lanvault_bridge.dart:3181` — 2s 配对轮询（R4）
- `lanvault_bridge.dart:3365-3420` — 14 条中文错误映射（正面）
- `lanvault_bridge.dart:3420` — 未命中即原样透出后端英文（W2）
- `lanvault_bridge.dart:3885` — 第三处「新增 9 个…」（E2）
- `lanvault_bridge.dart:3908` — `createVault` 仅 `trim()` 直传 `poolMode`（F1）
- `lanvault_bridge.dart:194`（约）— 全文件 194 处 `as` 强转（E3）

## flutter_app/lib/main.dart

- `main.dart:32-42` — `FontLoader` 解码 11.4 MB WOFF2，失败静默跳过（T2）
- `main.dart:106` — 300ms 裸时长（T3）
- `main.dart:428` — 配对弹窗 `barrierDismissible:false`（正面，唯一）
- `main.dart:597-611` — `isWide` 分支 + `_DesktopNavRail`（正面）
- `main.dart:802` — `BrandLogoFull(height:36)`（正面）
- `main.dart:946` — 底部导航语义完整（正面）

## flutter_app/lib/features/dashboard/dashboard_screen.dart

- `dashboard_screen.dart:42` — `_loading` 初值 true 但无骨架（S1）
- `dashboard_screen.dart:55` — 6s 轮询不感知生命周期（R4）
- `dashboard_screen.dart:89-94` — 失败只 toast，无内联态（S1）
- `dashboard_screen.dart:177-217` — 宽屏缺「数据健康」卡（S1）
- `dashboard_screen.dart:248` — 「去开启」按钮 < 46dp（A8）
- `dashboard_screen.dart:252` — 关闭按钮 `compact` 约 40dp（A8）
- `dashboard_screen.dart:275` — 硬编码 IP 前缀过滤（W2）
- `dashboard_screen.dart:288` — 卡片 `onTap` 无语义（A1）
- `dashboard_screen.dart:295` — 状态图标用 `healthy`（2.54:1）（A6）
- `dashboard_screen.dart:322` — 加载中即显示「点击进入设置启动」（S1）
- `dashboard_screen.dart:331`/`:368`/`:427`/`:469`/`:506`/`:605` — 6 处 `onTap` 无语义（A1）
- `dashboard_screen.dart:347` — 大数字无缩放兜底（A9）
- `dashboard_screen.dart:384` — 「已保护」误标为字节数（W1）
- `dashboard_screen.dart:545` — 同步横幅缺操作与 ETA（S3）
- `dashboard_screen.dart:631`/`:759` — 空态无 `action`（W4）
- `dashboard_screen.dart:693` — 仅健康卡有 chevron（W1）

## flutter_app/lib/features/vault/vault_screen.dart

- `vault_screen.dart:115` — `poolMode` 初值 `'single'`（F4）
- `vault_screen.dart:156` — 配额仅 `<=0` 校验，无上限（F4）
- `vault_screen.dart:160`/`:184`/`:262`/`:542` — `'raid1'` 字面量（F1）
- `vault_screen.dart:182` — `e2eeEnabled` 写死 true（F3）
- `vault_screen.dart:259` — 段值 `'raid1'`，缺 `striped`（F1/F2）
- `vault_screen.dart:350` — 「点击启用」徽标裸 `GestureDetector`（A1）
- `vault_screen.dart:355` — 操作指令写进 label（A1）
- `vault_screen.dart:402` — 仅空列表才显加载（S5）
- `vault_screen.dart:411` — 原始错误进空态副标题（W2）
- `vault_screen.dart:465`/`:479` — 全量构建卡片（R3）
- `vault_screen.dart:514` — `volumes.isEmpty` 误判「全部在线」（S1）
- `vault_screen.dart:543` — 徽标文案说模式、颜色说在线（W1）
- `vault_screen.dart:569` — 挂载路径截断不可复制（W3）

## flutter_app/lib/features/pairing/pairing_screen.dart

- `pairing_screen.dart:51` — 5s 轮询无失败可见性（R4）
- `pairing_screen.dart:136` — 超时文案称「已完成」（W1）
- `pairing_screen.dart:152` — 扫描错误后回落空态（S1）
- `pairing_screen.dart:171` — `_revokePeer` 无确认（C1）
- `pairing_screen.dart:176` — 解绑成功不刷新列表（S2）
- `pairing_screen.dart:390` — 添加成员默认预选首个（W1）
- `pairing_screen.dart:461` — 移除成员无确认（C1）
- `pairing_screen.dart:711` — `Icons.workspaces_outline` 拼写变体（T8）
- `pairing_screen.dart:750` — 组语义未含在线/离线（A5）
- `pairing_screen.dart:939` — 空态写「点击 + 配对新设备」但无按钮（W4）
- `pairing_screen.dart:954` — 整行点击改目标，无 button/selected 语义（A1）
- `pairing_screen.dart:1013` — 菜单单击即解绑（C1）
- `pairing_screen.dart:1101` — 32 位指纹全量外露（W3）
- `pairing_screen.dart:1113` — 配对未预填指纹（W1）
- `pairing_screen.dart:1203` — 等待对端确认无超时/无取消（S3）
- `pairing_screen.dart:1295` — `info == null` 一律「服务未启动」（S1）
- `pairing_screen.dart:1311` — `------` 与真实 PIN 同色（W3）

## flutter_app/lib/features/snapshots/snapshots_screen.dart

- `snapshots_screen.dart:92` — 加载无超时（S3）
- `snapshots_screen.dart:156` — 错误仅 toast（S3）
- `snapshots_screen.dart:317` — 恢复中静默丢弃点击（C3）
- `snapshots_screen.dart:319-328` — 恢复无确认直接写盘（C1/C3）
- `snapshots_screen.dart:324` — 仅一条会消失的 toast（C3）
- `snapshots_screen.dart:619` — 加载失败显示为空态（S1）
- `snapshots_screen.dart:681` — 「Rev」中英混排（W1）
- `snapshots_screen.dart:820` — 列表行无选中语义（A1）
- `snapshots_screen.dart:859` — 桌面「恢复」无确认（C1）
- `snapshots_screen.dart:942` — 横向节点 16px 触控目标（A8）
- `snapshots_screen.dart:1040` — 纵向节点 12px（A8）
- `snapshots_screen.dart:1100` — 「预览并恢复」名不副实（W1）
- `snapshots_screen.dart:1153` — 校验/巡检/修复混用（W1）
- `snapshots_screen.dart:1208` — `scrub.status` 原值直出（W2）
- `snapshots_screen.dart:1226` — 空态承诺恢复历史不持久（W4）
- `snapshots_screen.dart:1257` — `snapshotId` 作主标题（W2）
- `snapshots_screen.dart:1389` — 版本弹窗内容无滚动（已核实外层有滚动，降为 P3 观察）
- `snapshots_screen.dart:1445` — `archiveReason` 原值（W2）

## flutter_app/lib/features/settings/settings_screen.dart

- `settings_screen.dart:27` — 注释「左侧 5 项」与 6 项实现不符（T8）
- `settings_screen.dart:87` — 恢复默认皮肤无确认（C1）
- `settings_screen.dart:107` — 搜索仅 `onSubmitted` 生效（W1）
- `settings_screen.dart:197` — 「双日夜调板」自造词（W1）
- `settings_screen.dart:267` — 默认值先于真实值渲染（S1）
- `settings_screen.dart:296` — 仅 `resumed` 刷新权限（R4）
- `settings_screen.dart:307` — 空 `catch (_)` 吞权限读取错误（S3）
- `settings_screen.dart:312-377` — 10 项并发加载 + 失败聚合成横幅（正面）
- `settings_screen.dart:410` — 裸 `ListTile` 脱离拟态体系（W1）
- `settings_screen.dart:450` — 非数字输入静默归零（S2）
- `settings_screen.dart:477` — 保存失败仍弹成功（S2）
- `settings_screen.dart:589` — 关闭开关未撤销服务端密码（S6）
- `settings_screen.dart:645` — 无服务时假启用密码（S6）
- `settings_screen.dart:700` — 主密钥明文写入剪贴板（安全，P2）
- `settings_screen.dart:737` — 重启服务无确认（C1）
- `settings_screen.dart:787` — 清空网络限制立即持久化、无确认（C1）
- `settings_screen.dart:801` — 「完成」保存结果被丢弃（S2）
- `settings_screen.dart:926` — 清空日志零确认（C1）
- `settings_screen.dart:985` — 权限组仅窄屏渲染（S1）
- `settings_screen.dart:1079` — 内容 720 上限左对齐（响应式）
- `settings_screen.dart:1137` — 节标题字号硬编码 13（T1）
- `settings_screen.dart:1302` — 密码开关本地伪状态（S6）
- `settings_screen.dart:1527-1578` — `_SettingsSwitchRow` 裸 `Switch`，未合并语义（A4）
- `settings_screen.dart:1601` — `InkWell` 无显式按钮语义（A1）
- `settings_screen.dart:1621` — 副标题单行省略丢路径（W3）
- `settings_screen.dart:1683` — 选中态仅靠边框与勾选色（A6）

## flutter_app/lib/features/backup/backup_screen.dart

- `backup_screen.dart:63-64` — `super.initState()` 连写两次（T8）
- `backup_screen.dart:117` — 7 个 `Future.wait` 任一失败全灭（S2）
- `backup_screen.dart:149` — 失败后直接露空态（S1）
- `backup_screen.dart:306` — 撤销镜像根无确认（C1）
- `backup_screen.dart:467` — 删除组规则直执行（C1）
- `backup_screen.dart:505` — 未知状态归为「离线待同步」（W2）
- `backup_screen.dart:612` — 删除备份任务无确认（C1）
- `backup_screen.dart:720` — 后端枚举直出（W2）
- `backup_screen.dart:812` — `ListView(children:[...])` 全量构建（R3）
- `backup_screen.dart:828` — 页内重复「备份」大标题（W1）
- `backup_screen.dart:857`/`:866` — 两张不可点卡占首屏（W1）
- `backup_screen.dart:1038`/`:2133` — 卡片三层嵌套（T8）
- `backup_screen.dart:1102` — `NeuSwitch(title:'', semanticLabel:)` 正确范式（正面）
- `backup_screen.dart:1237` — 全局暂停裸图标、范围不明（W3）
- `backup_screen.dart:1272` — 开关轨道仅 28 高（A8）
- `backup_screen.dart:1367` — 任务行无状态与上次备份时间（S2）
- `backup_screen.dart:1403`/`:1424` — 窄屏丢调度徽章与内联扫描（响应式）
- `backup_screen.dart:1467` — 菜单删除项直调（C1）
- `backup_screen.dart:1617` — 「主/备异盘」术语（W1）
- `backup_screen.dart:2410` — `ruleId` 用毫秒取模生成（E3）
- `backup_screen.dart:2515` — FAB 裸 `GestureDetector`，键盘不可达（A1）

## flutter_app/lib/features/clipboard/clipboard_screen.dart

- `clipboard_screen.dart:44` — 2s 轮询，静默失败（R4）
- `clipboard_screen.dart:63` — 历史硬上限 100，无分页提示（W1）
- `clipboard_screen.dart:68` — `_outbox` 只增不删（R3）
- `clipboard_screen.dart:178` — 粘贴覆盖草稿无确认/撤销（C1）
- `clipboard_screen.dart:189` — 200ms 裸时长（T3）
- `clipboard_screen.dart:198` — 断点用整窗宽而非面板宽（响应式）
- `clipboard_screen.dart:207-218` — 归属靠 `peerName` 字符串启发式（E3）
- `clipboard_screen.dart:231-261` — 历史与发件箱双源无去重键（R2）
- `clipboard_screen.dart:271` — 窄屏无设备行（S1）
- `clipboard_screen.dart:346` — 设备行裸 `InkWell`，无选中语义（A1）
- `clipboard_screen.dart:431` — 设备头仅宽屏渲染（S1）
- `clipboard_screen.dart:640-650` — 失败态与成功同权重（W1）
- `clipboard_screen.dart:656` — 文件气泡无任何操作（S3）
- `clipboard_screen.dart:712-718` — 气泡点击复制无可发现线索（W3）
- `clipboard_screen.dart:729` — 文本无 `maxLines`/不可选中（W3）
- `clipboard_screen.dart:759` — 仅 Tooltip 提供名称（W3）

## flutter_app/test/widget_test.dart

- `widget_test.dart:171` — `createVault` 桩直接 `UnimplementedError`（E4，F1 得以存活的原因）
- `widget_test.dart` — 56 处空实现替换真实桥接（E4）

## crates（仅引用，用于证实跨端契约）

- `crates/lanvault-core/src/db.rs:20-45` — `PoolMode` 三值定义与 `parse` 白名单（F1）
- `crates/lanvault-ffi/src/api.rs:4308` — `createVault` 用 `PoolMode::parse`（F1）
- `crates/lanvault-ffi/src/api.rs:4318` — `Striped → VolumeRole::PoolExt`（F2）
- `crates/lanvault-server/src/web/mod.rs:1282` — 强制 `e2ee_enabled == true`（F3）
- `crates/lanvault-server/src/vault.rs:356`/`:436` — 条带读写两路实现（F2）
- `.env.example` — 缺 `LANVAULT_CONFIG_PATH`/`DAEMON_MODE`/`QUOTA_GB`（E5）
- `flutter_app/analysis_options.yaml:11-33` — 仅默认 `flutter_lints`，无自定义规则（E4）
