# EveryoneNAS 日夜模式、皮肤切换与滑动动画移植施工指南（对标 ToList）

> 目标仓库：`D:\code\nas\EveryoneNAS-Merge`（分支 `integration/best-of-both`）  
> 参考源仓库：`D:\code\tolist\ToList`（核心逻辑）与 `D:\code\tolist\ToListAndroid`（移动端手势/动效补充）  
> 适用端：Flutter Windows 桌面端 + Android 移动端（共用 `flutter_app/` 源码与原生桥接层）  
> 本文档供施工 AI 按步骤直接执行，所有文件路径、符号与行号均已在目标/参考仓库核对。

---

## 0. 需求与架构总览

### 0.1 核心需求
1. **日夜模式系统**：支持「跟随系统 / 日间 / 夜间」三态；Windows 顶栏与 Android 顶栏均提供一键日夜切换入口，并支持类似 ToList 的**圆形扩散揭示转场动画**。
2. **多皮肤切换系统**：移植 ToList 的双调板（Light/Dark 成对定义）与对比度守护算法，保留 EveryoneNAS 默认「柔冰蓝」主皮肤，引入「赤陶纸」及精选单色/双色皮肤市场（支持搜索、分类筛选、实时预览与一键还原）。
3. **方向感滑动动画系统**：移植 ToList `viewSlide.ts` + `mobileMotion.ts` + `motionTokens.ts` 的统一运动语言，为一级页面切换、二级分段切换（备份/历史版本、已配对/发现设备、设置分类）与二级路由推入/弹出提供**方向一致、可即时中断、支持三档速度与系统减弱动画降级**的滑动动效。
4. **验证与 Release 发布**：补齐测试，跑通 `flutter analyze` 与 `flutter test`，完成 Windows Release 与 Android Release 构建及发布打包。

### 0.2 参考实现映射（ToList -> EveryoneNAS-Merge）

| 能力模块 | ToList 参考文件与行号 | EveryoneNAS Flutter 对应目标 |
| --- | --- | --- |
| 日夜选择与主题解析 | `D:\code\tolist\ToList\apps\web\src\app\themes.ts` L210-282, L306-380 | `flutter_app/lib/theme/neu_theme.dart` + `appearance_controller.dart` |
| 皮肤定义、混色与 WCAG 对比度 | `D:\code\tolist\ToList\apps\web\src\app\skins.ts` L15-265, L294-557 | 新增 `flutter_app/lib/theme/skins.dart` |
| 控件级语义回退令牌 | `D:\code\tolist\ToList\apps\web\src\app\skinTokens.ts` L54-143 | `NeuTheme` (`ThemeExtension`) 全字段化 |
| 圆形扩散揭示转场 | `D:\code\tolist\ToList\apps\web\src\app\themeReveal.ts` L22-130 | 新增 `flutter_app/lib/theme/theme_reveal.dart` |
| 外观偏好与系统主题监听 | `D:\code\tolist\ToList\apps\web\src\app\appearancePrefs.ts` L18-226；`useSystemTheme.ts` L12-27 | 新增 `flutter_app/lib/theme/appearance_controller.dart` |
| 运动曲线与三档速度令牌 | `D:\code\tolist\ToList\apps\web\src\app\motionTokens.ts` L8-153 | 升级 `flutter_app/lib/theme/neu_theme.dart` L13-45 (`NeuMotion`) |
| 可中断方向滑动切换 | `D:\code\tolist\ToList\apps\web\src\app\viewSlide.ts` L16-253；`D:\code\tolist\ToListAndroid\apps\web\src\app\mobileMotion.ts` L6-160 | 新增 `flutter_app/lib/theme/motion_slide.dart` |
| 顶栏日夜按钮 | `D:\code\tolist\ToList\apps\web\src\components\TitleBar.tsx` L108-118 | `windows_title_bar.dart` + `main.dart` (`_MobileTopBar`) |
| 外观设置与皮肤市场 UI | `D:\code\tolist\ToList\apps\web\src\pages\AppearancePage.tsx` L92-334, L671-900 | `flutter_app/lib/features/settings/settings_screen.dart` |

---

## 1. 施工前准备与现状基线

1. **备份核心文件**（执行前先将待改核心文件复制到 `D:\code\nas\EveryoneNAS-Merge\backups\`）：
   - `flutter_app/lib/theme/neu_theme.dart`
   - `flutter_app/lib/main.dart`
   - `flutter_app/lib/features/settings/settings_screen.dart`
   - `flutter_app/windows/runner/flutter_window.cpp`
   - `flutter_app/windows/runner/win32_window.cpp`
   - `flutter_app/android/app/src/main/res/values-night/styles.xml`
2. **基线检查**：在 `D:\code\nas\EveryoneNAS-Merge\flutter_app` 运行 `flutter test`（当前共 12 个用例全绿）确认初始可用。
3. **注意依赖约束**：`flutter_app/pubspec.yaml` (L10-19) 仅依赖 `flutter`、`flutter_rust_bridge`、`ffi`、`material_ui`，**不要擅自新增 pub 第三方包**；所有颜色计算、圆形揭示、滑动动画、JSON 持久化均使用 Flutter SDK 与 `dart:io`/`dart:ui`/`dart:math` 原生实现。

---

## 2. 详细施工步骤

### Step 1：移植皮肤目录与色板算法（新增 `flutter_app/lib/theme/skins.dart`）

参考 `D:\code\tolist\ToList\apps\web\src\app\skins.ts` L15-557 与 `themes.ts` L334-380，新建 `flutter_app/lib/theme/skins.dart`：

1. **定义枚举与数据结构**：
   - `enum SkinFamily { defaultSkin, mono, duo }`
   - `enum SkinGroupId { base, inkGray, blue, green, warmWhite, pinkViolet, redOrange, yellow, duoContrast, duoPremium, duoComfort }`
   - `class SkinPalette`（对应 `skins.ts` L53-75 + `NeuTokens` 全部语义角色）：
     - 基础面：`canvas`（对应 `paper`）、`surface`、`insetSurface`（对应 `wash`）、`hoverSurface`、`chromeSurface`（侧栏/标题栏底色，替代旧硬编码 `0xFFFAFBFD`）、`surfacePressed`（对应 `surface3`）
     - 线条与轮廓：`hairlineBorder`（对应 `line`）、`controlOutline`、`dividerSubtle`
     - 文字与对比：`textPrimary`（`ink`）、`textSecondary`（`muted`）、`slateMuted`
     - 强调色：`accent`、`accentLight`、`accentPressed`（`accentStrong`）、`accentSoft`、`onAccent`
     - 状态与语义：`healthy`、`healthyAlt`、`healthyButton`、`healthySoft`、`folderGold`、`folderGoldSoft`、`warning`、`danger`、`dangerSoft`
     - 禁用态深色按钮：`disabledDarkSurface`、`disabledDarkBorder`、`disabledDarkForeground`
     - 图表与时间线：`chartSky`、`chartIndigo`、`chartIndigoSoft`、`timelineInactive`、`timelineHalo`、`timelineLine`
     - 浮层与开关：`dialogBarrier`、`switchTrackDisabled`、`switchTrackOff`
     - 阴影组：`extrudedShadows`、`controlShadows`、`primaryButtonShadows`、`pressedShadows`
2. **移植颜色数学与 WCAG 对比度函数**（对应 `skins.ts` L126-265 与 `themes.ts` L334-344）：
   - `mixColor(Color a, Color b, double amount)`
   - `relativeLuminance(Color c)` & `contrastRatio(Color a, Color b)`
   - `isDarkColor(Color c)`（阈值 `relativeLuminance < 0.32`，对应 `skins.ts` L164）
   - `pickContrastInk(Color background)`（保证白字或深墨字对比度 `>= 4.58:1`，对应 `themes.ts` L334-344）
   - `ensureVisibleLine(Color canvas, Color surface, Color line)`（保证卡片描边对比度 `>= 1.6`，对应 `skins.ts` L168-189）
   - `ensureReadableAccent(Color source, Color canvas, bool dark)`（浅色 `>= 3.8`、暗色 `>= 3.0`，对应 `skins.ts` L191-202）
   - `buildDerivedPalette({required Color canvas, required Color accentSource, required Brightness brightness})`（对应 `skins.ts` L209-244）
   - `monoSkin(...)` 与 `duoSkin(...)`（对应 `skins.ts` L246-285）
3. **固化内置皮肤清单**：
   - **默认皮肤 `skin_default`（「柔冰蓝」）**：
     - `light`：直接固化当前 `neu_theme.dart` L72-189 的全部柔冰蓝 / 纯白轻拟态色值。
     - `dark`：手工校准的深海夜蓝低眩光调板（例：`canvas: #0F141E`、`chromeSurface: #131926`、`surface: #182030`、`insetSurface: #202A3D`、`hoverSurface: #232F44`、`hairlineBorder: #2A364D`、`textPrimary: #ECF2FA`、`textSecondary: #9AA8BF`、`accent: #4C8BF5`、`accentLight: #60A5FA`、`accentPressed: #3B82F6`、`accentSoft: #1E2E4A`，以及高可读的暗色语义底色与阴影）。
   - **「赤陶纸」皮肤 `skin_terracotta`**：完整移植 `skins.ts` L294-320 的 `light`（`#F7F7F5 / #FFFFFF / #262624`）与 `dark`（`#1C1A18 / #26231F / #E0A184`）精确令牌。
   - **单色与双色皮肤市场**：从 `skins.ts` L326-467 移植 `monoSkinGroups`（墨灰/蓝色/绿色/暖白/粉紫/红橙/黄色 7 组）与 `duoSkinGroups`（撞色/高级/舒适 3 组），所有皮肤均同时具备 `light` 与 `dark` 两套 `SkinPalette`。

---

### Step 2：改造 `NeuTheme`、`NeuMotion` 与全套组件库（`flutter_app/lib/theme/neu_theme.dart` & `neu_toast.dart`）

#### 2.1 修改 `flutter_app/lib/theme/neu_theme.dart`
1. **升级 `NeuMotion`（L13-45）**，对齐 `D:\code\tolist\ToList\apps\web\src\app\motionTokens.ts` L8-153：
   - 定义 `enum MotionSpeed { fast, balanced, elegant }`，缩放倍率 `fast: 1.0`, `balanced: 1.46`, `elegant: 1.7`（对应 `motionTokens.ts` L90-96）。
   - 新增静态/可变速度倍率入口（或从 `AppearanceScope` 读取），以及统一曲线常量：
     - `enterCurve = Cubic(0.16, 1.0, 0.3, 1.0)`（对应 `MOTION_CURVE`）
     - `exitCurve = Cubic(0.4, 0.0, 1.0, 1.0)`（对应 `MOTION_EXIT_CURVE`）
     - `appleSheetCurve = Cubic(0.32, 0.72, 0.0, 1.0)`（对应 `APPLE_SHEET_CURVE`）
     - `revealCurve = Cubic(0.22, 1.0, 0.36, 1.0)`（对应 `themeReveal.ts` L119）
   - 基准时长：`pressDuration = 180ms`、`pageEnterDuration = 220ms`、`pageExitDuration = 140ms`、`modalDuration = 240ms`、`revealDuration = 280ms`、`sheetDuration = 420ms`（受 `MotionSpeed` 缩放；`isReduced(context)` 为 true 时折叠为 `reducedDuration = 100ms`）。
2. **拆分 `NeuTokens`（L70-197）**：
   - 保留布局/平台非颜色常量：`appFontFamily` (L139)、`breakpointWide` (L140)、`minTouchTarget` (L142)、`minMobileTouchTarget` (L143)、`spaceXs~Lg` (L145-148)、`resolveMinTouchTarget` (L191-196)。
   - 将 L72-137 的颜色/渐变与 L151-189 的阴影全部迁移为 `NeuTheme` 的实例属性（可保留 `NeuTokens.defaultLight` 供极少数无 context 静态常量引用迁移期间过渡，最终所有 UI 均通过 `NeuTheme.of(context)` 获取）。
3. **扩充 `NeuTheme extends ThemeExtension<NeuTheme>`（L200-271）**：
   - 持有完整 `SkinPalette`（或直接展平所有颜色/渐变/阴影字段 + `Brightness brightness` + `String skinId`）。
   - 提供便捷属性：`LinearGradient get primaryGradient`、`LinearGradient get pressedGradient`、`bool get isDark`。
   - 完整实现 `copyWith`、`lerp`（用 `Color.lerp` 与 `BoxShadow.lerpList` 平滑插值，使主题渐变动画天然成立）、`==` 与 `hashCode`。
4. **改造 `buildLanVaultTheme`（L273-337）**：
   - 签名改为 `ThemeData buildLanVaultTheme({SkinPalette? palette, Brightness brightness = Brightness.light, String skinId = 'skin_default'})`（保留默认无参调用以兼容既有测试 `widget_test.dart` L937/958/978）。
   - 按 `brightness` 构造 `ColorScheme.light(...)` 或 `ColorScheme.dark(...)`，移除 `textTheme` 的 `const`（L294）并绑定 `palette.textPrimary` / `palette.textSecondary`，注入 `NeuTheme` extension。
5. **改造 `neu_theme.dart` 内部全部内置组件读取 `NeuTheme.of(context)`**：
   - `StorageDonutRing` & `_StorageDonutPainter`（L383-513）：把 `palette` 传给 `_StorageDonutPainter`，替换 L426、L439、L472 (`Color(0xFFE6EEF9)`)、L484-489；并在 `shouldRepaint` (L509) 对比颜色字段。
   - `NeuStatusBadge`（L518-575）：`_resolveColors(NeuTheme neu)` 替换 L531-537。
   - `NeuCard`（L578-732）：替换 L641 (`hoverSurface`)、L644 (`Color(0xFFE0E8F4)` 改 `neu.hairlineBorder`)、L646-647。
   - `showNeuDialog`（L819-852）：L829 `barrierColor` 改为 `NeuTheme.of(context).dialogBarrier`。
   - `NeuButton`（L857-1100）：L923-967 替换前景色（`primary` / `danger` / `healthy` 用 `neu.onAccent` / `pickContrastInk` 保证浅色高明度皮肤按钮文字清晰！）、背景色、渐变与阴影；L1038-1044 替换边框色。
   - `NeuSwitch`（L1103-1285）：L1173-1208 替换 `Color(0xFFE2E8F0)`、`Color(0xFFD6DFEE)`、`primaryGradient` 及阴影 `Color(0x260F172A)`；L1259-1260 替换文字色。
   - `NeuSegmentedControl`（L1302-1493）：L1401、L1419、L1425 (`Color(0x282563EB)`)、L1468 (`neu.onAccent`)、L1469 全部改用 `neu`；并给滑块增加轻微的过渡弹性/缩放跟随（对齐 `UI_MOTION_UPGRADE.md` #010）。
   - `NeuInsetField`（L1496-1667）：L1559-1641 替换 `surface` / `accent` / `danger` / `hairlineBorder` / `Color(0x1F2563EB)`。
   - `NeuProgressBar`（L1670-1747）：注意 L1675 构造参数默认 `this.fillColor` 改为 `Color? fillColor`，在 `build` 内 `final effectiveFill = fillColor ?? neu.accent;`；移除 L1693 对静态常量的相等判断，改用 `fillColor == null` 判断是否启用 `neu.primaryGradient`。
   - `NeuDropdown`（L1764-1895）：L1802-1870 替换 `dropdownColor` (`neu.surface`)、边框与文字色。
   - `NeuIconBadge`（L1898-1928）：L1902-1903 默认参数改为可空 `this.color` / `this.backgroundColor`，在 `build` 内回退 `neu.accent` / `neu.accentSoft`。
   - `NeuDialog`（L1931-2020）：L1995 替换关闭按钮图标色为 `neu.textSecondary`。
   - `NeuKeyValueRow`（L2023-2093）：L2029 `this.leadingColor` 改为可空并在 `build` 回退 `neu.healthy`；L2067 替换 `textSecondary`。

#### 2.2 修改 `flutter_app/lib/theme/neu_toast.dart`
- L131-154 (`_resolveVisual`)：接收 `NeuTheme neu`，不再使用静态 `NeuTokens`。
- L175-194：将 `NeuTokens.surface`、`NeuTokens.hairlineBorder`、阴影 `Color(0x1A0F172A)` / `Color(0x122563EB)` 替换为 `neu.surface`、`neu.hairlineBorder`、`neu.extrudedShadows`。
- L213：替换为 `neu.textPrimary`。

---

### Step 3：外观状态控制器与持久化（新增 `flutter_app/lib/theme/appearance_controller.dart`）

参考 `D:\code\tolist\ToList\apps\web\src\app\appearancePrefs.ts` L18-226 与 `useSystemTheme.ts` L12-27：

1. **状态模型**：
   - `enum ThemeModePreference { system, light, dark }`
   - `AppearanceController extends ChangeNotifier`：
     - `ThemeModePreference themeMode`（默认 `system`）
     - `String skinId`（默认 `'skin_default'`）
     - `MotionSpeed motionSpeed`（默认 `MotionSpeed.balanced`）
     - `Brightness platformBrightness`（由 `WidgetsBindingObserver.didChangePlatformBrightness` 实时同步）
     - 计算属性 `Brightness get effectiveBrightness`：
       - `light` -> `Brightness.light`
       - `dark` -> `Brightness.dark`
       - `system` -> `platformBrightness`
     - 计算属性 `SkinDefinition get activeSkin` & `SkinPalette get activePalette` & `ThemeData get themeData`
2. **持久化策略（推荐纯 Dart 本地 JSON 文件，无需改 Rust ABI；也可后续同步进 `db.rs` `app_settings` L1953-1980）**：
   - 存放路径：
     - Windows：`%LOCALAPPDATA%\EveryoneNAS\appearance_prefs.json`（通过 `Platform.environment['LOCALAPPDATA']` 获取，测试环境回退仅内存）
     - Android：通过已有 `MethodChannel('com.lanvault.app/keep_alive')` 新增一个轻方法 `"getFilesDir"` 返回 `filesDir.absolutePath`（`MainActivity.kt` L181 `when (call.method)` 添加一行即可），持久化到 `<filesDir>/appearance_prefs.json`。
   - 启动加载：在 `main.dart` L97 `runApp(...)` 之前以短超时（如 300ms）预加载 `appearance_prefs.json`，确保首帧即以用户上次的皮肤与日夜模式渲染，避免冷启动白闪/蓝闪。
3. **提供 `AppearanceScope extends InheritedNotifier<AppearanceController>`**：
   - 包裹在 `LanVaultApp` 外层，供设置页与顶栏日夜按钮随时读取与切换。

---

### Step 4：圆形扩散揭示动效（新增 `flutter_app/lib/theme/theme_reveal.dart`）

1:1 移植 `D:\code\tolist\ToList\apps\web\src\app\themeReveal.ts` L22-130：

1. **核心原理（Widget + `ui.Image` 快照遮罩）**：
   - 用 `RepaintBoundary(key: _boundaryKey)` 包裹整个 `MaterialApp`（或 `LanVaultShell` 根视觉树）。
   - 触发 `switchWithReveal(BuildContext triggerContext, VoidCallback applyChange, {Offset? globalOrigin})` 时：
     1. 若 `NeuMotion.isReduced(context)` 为 true，直接执行 `applyChange()`（跳过截图与动画，对应 `themeReveal.ts` L60-76）。
     2. 若当前正在执行上一轮揭示动画，立即终止上一轮并释放旧 `ui.Image`（对应 `themeReveal.ts` L48-55）。
     3. 解析起点坐标 `origin`：若未显式传入 `globalOrigin`，则通过 `triggerContext.findRenderObject()` 计算控件中心全局坐标，兜底屏幕中心（对应 `themeReveal.ts` L22-31）。
     4. 通过 `_boundaryKey.currentContext?.findRenderObject()` 调用 `boundary.toImage(pixelRatio: dpr)` 截取切换前的画面 `oldImage`（注意：截取旧帧 -> 立即执行 `applyChange()` 切换到新主题 -> 顶层覆盖一个展示 `oldImage` 的**反向圆形镂空**或**新帧在底层、旧帧用 `Path.combine(PathOperation.difference, fullRect, expandingCircle)` 裁剪**！使用「顶层绘制旧帧截图 + 中心圆形空洞从 0 扩至 `maxRadius`」方案**完全不需要阻塞新 UI 交互**，且能立即响应下一次点击，完美对齐 `themeReveal.ts` L88-105「pointer-events: none + 任意新输入可跳过」）。
     5. 计算最大半径：`radius = hypot(max(x, width - x), max(y, height - y))`（对应 `themeReveal.ts` L79-82）。
     6. 使用 `AnimationController(duration: 280ms)` + `CurvedAnimation(curve: Cubic(0.22, 1.0, 0.36, 1.0))` 驱动空洞半径 `0 -> radius`，动画完成或被新输入中断时 `oldImage.dispose()` 并移除叠层。

---

### Step 5：方向感滑动动画移植（新增 `flutter_app/lib/theme/motion_slide.dart`）

参考 `D:\code\tolist\ToList\apps\web\src\app\viewSlide.ts` L16-253 与 `D:\code\tolist\ToListAndroid\apps\web\src\app\mobileMotion.ts` L6-160：

1. **封装 `SlideSwitchViewport`（对应 `useSlideSwitch`）**：
   - 输入：`required int index`、`required List<Widget> children`、`Axis axis = Axis.horizontal`、`bool keepAlive = true`。
   - 方向解析：比较 `oldIndex` 与 `widget.index`：
     - `newIndex > oldIndex` -> `direction = 1`（旧页向左/上漂移 `12px` 并淡出 `140ms`，新页从右侧/下方 `24px` 滑入并淡入 `220ms`，对应 `viewSlide.ts` L22-23 与 `motionTokens.ts` L102-103）。
     - `newIndex < oldIndex` -> `direction = -1`（镜像方向）。
   - **可中断与状态保活**：
     - 外层保持 `IndexedStack`（或 `TickerMode` + `Offstage`）保活各页滚动位置与网络状态（避免切回总览/备份页重拉接口），内部用一个轻量 `AnimationController` 对当前可见层执行进场位移+透明度过渡；若切换过快在途再次切换，使用单一递增 `_switchToken`（对应 `viewSlide.ts` L116 `switchTokenRef`）立即取消旧动画并从当前 offset 收敛至目标，绝不出现排队延迟或透明锁死。
     - `NeuMotion.isReduced(context)` 为 true 时，直接瞬切或仅做 `100ms` 微弱淡入（位移为 0）。
   - **移动端手势联动（可选增强）**：在移动端底部 4 个主 Tab 区域支持水平轻扫手势（或保留点击触发，避免与 `ClipboardScreen` / `RefreshIndicator` / 水平滚动卡片冲突；若加轻扫需用 `PageView` 仅在明确无水平滚动子控件时或仅通过 `SlideSwitchViewport` 点击方向驱动，推荐**先以点击方向驱动的 `SlideSwitchViewport` 为主**，确保不误触聊天气泡和水平图表）。
2. **自定义路由滑动转场 `NeuSlidePageRoute<T>`（对应 `mobileMotion.ts` push/pop 与 `APPLE_SHEET_CURVE`）**：
   - 用于 `main.dart` L224 `_openDeviceManager()` 等二级全屏页面推入：从右侧 100% 滑入 + 微弱背景遮罩变暗，时长 `280ms`，曲线 `Cubic(0.32, 0.72, 0.0, 1.0)`，Android 上自然支持返回滑出。
3. **接入点清单**：
   - `flutter_app/lib/main.dart` L526-550 (`_buildAnimatedPageContent`)：将当前仅纵向 `5px` 的 `TweenAnimationBuilder` 替换为 `SlideSwitchViewport`（桌面端侧栏纵向顺序用 `Axis.vertical`；移动端底栏横向顺序用 `Axis.horizontal`，设置页与总览之间用 `Axis.horizontal` 推拉）。
   - `flutter_app/lib/main.dart` L225 (`_openDeviceManager`)：将 `MaterialPageRoute` 替换为 `NeuSlidePageRoute`。
   - `flutter_app/lib/features/backup/backup_screen.dart` L791-795 (`_subTab` 0/1)：包裹为 `SlideSwitchViewport(index: _subTab, axis: Axis.horizontal, children: [...])`。
   - `flutter_app/lib/features/pairing/pairing_screen.dart` L618-621 (`_activeTab`)：包裹为 `SlideSwitchViewport(index: _activeTab.index, axis: Axis.horizontal, children: [...])`。
   - `flutter_app/lib/features/settings/settings_screen.dart` L861 (`_buildDesktopSectionBody`)：使用 `SlideSwitchViewport` 或方向性 `AnimatedSwitcher` 切换右侧 5/6 个设置分组。

---

### Step 6：Windows & Android 原生系统适配

#### 6.1 Windows 原生层 + 自绘标题栏
1. **`flutter_app/windows/runner/win32_window.h` & `win32_window.cpp`**：
   - 参考 `win32_window.cpp` L509-522 (`UpdateTheme`)：新增 `void Win32Window::ApplyDarkMode(bool dark)`，直接调用 `DwmSetWindowAttribute(window_handle_, DWMWA_USE_IMMERSIVE_DARK_MODE, &enable_dark_mode, sizeof(enable_dark_mode))`（L519），使 Windows 11 原生窗口边框/系统菜单/最大化阴影同步匹配当前 App 日夜模式，不再只读注册表。
   - 若用户选择 `ThemeModePreference.system`，在 `flutter_window.cpp` L350 (`MessageHandler`) 监听 `WM_SETTINGCHANGE`（如 `"ImmersiveColorSet"`）时刷新原生标题栏主题。
2. **`flutter_app/windows/runner/flutter_window.cpp`**：
   - 在 `window_channel_` 处理链（L228-266）增加 `"setDarkMode"` 方法调用，解析 `bool` 参数并调用 `ApplyDarkMode(dark)`。
3. **`flutter_app/lib/bridge/window_chrome.dart`**：
   - 在 L85 `_invoke` 后新增 `static Future<void> setDarkMode(bool dark)`，通过 `_channel.invokeMethod<void>('setDarkMode', dark)` 下发给 Win32。
4. **`flutter_app/lib/widgets/windows_title_bar.dart`**：
   - **Token 迁移**：
     - L99 `Color(0xFFFAFBFD)` -> `neu.chromeSurface`
     - L101 `NeuTokens.hairlineBorder` -> `neu.hairlineBorder`
     - L220 `Color(0xFFC42B1C)` -> `neu.isDark ? const Color(0xFFA82317) : const Color(0xFFC42B1C)`
     - L221 `NeuTokens.danger` -> `neu.danger`
     - L224 `Color(0xFFDCE6F5)` -> `neu.surfacePressed`
     - L225 `NeuTokens.insetSurface` -> `neu.insetSurface`
     - L230-231 -> `neu.textSecondary`
     - L269 `NeuTokens.accent` -> `neu.accent`
   - **增加日夜切换按钮**（对应 ToList `TitleBar.tsx` L108-118）：
     - 在 `WindowsTitleBar` 增加可选回调 `VoidCallback?` 或内部直接通过 `AppearanceScope.of(context)` 切换（日 <-> 夜快速切换，或长按/右键切回跟随系统）；
     - 放在右侧 `_controlsKey` 所在的 `Row` 内（L119 `Row` 最左侧，最小化按钮之前，加一条 `1px` 竖线分隔，使其矩形自动包含在 `_controlsKey` 上报给 Win32 的 `exclusions` 中，保证可点击且不破坏标题栏其余区域拖拽！）；
     - 点击时取按钮 `BuildContext` 坐标调用 `ThemeRevealHost.of(context).reveal(...)`；
     - 同步在 `_CaptionButton` (L274) 为窗口关闭按钮 hover 增加 `1px` 轻微下移微交互（对齐 `UI_MOTION_UPGRADE.md` #015）。

#### 6.2 Android 原生层 + 移动顶栏
1. **`flutter_app/android/app/src/main/res/values-night/styles.xml`**：
   - 将 L4 `LaunchTheme` 与 L12 `NormalTheme` 的 `parent` 从 `@android:style/Theme.Light.NoTitleBar` 改为 `@android:style/Theme.Black.NoTitleBar`（或配暗色 `@drawable/launch_background`），消除系统深色模式下启动闪白问题。
2. **`flutter_app/android/app/src/main/kotlin/com/lanvault/app/MainActivity.kt`**：
   - 在 `handleKeepAliveCall`（L181 `when (call.method)`）增加 `"getFilesDir" -> result.success(filesDir.absolutePath)`，供纯 Dart `AppearanceController` 读写配置。
3. **`flutter_app/lib/main.dart`（系统栏与移动顶栏）**：
   - 每次有效亮度/皮肤变化时调用 `SystemChrome.setSystemUIOverlayStyle` 同步 Android 状态栏图标亮暗（`statusBarIconBrightness`）与导航栏底色（`systemNavigationBarColor: neu.surface`）。
   - 在 `_MobileTopBar`（L623-689）右上角设置按钮（L657）左侧增加一个 `46x46` 圆形日/夜切换图标按钮（使用 `Icons.dark_mode_outlined` / `Icons.light_mode_outlined`），点击时以该按钮中心触发 `ThemeReveal` 切换日夜模式。

---

### Step 7：全量业务页面与组件颜色迁移 + 外观设置页（精确行号清单）

> 核心原则：全项目 **0 处残留硬编码日间底色 `Color(0xFFFAFBFD)` / `Color(0x0A0F172A)` / `Color(0x59000000)`**，也 **0 处调用静态颜色 `NeuTokens.<color>`**。除品牌白字在已保证对比度前提下使用 `neu.onAccent` 外，其余全部从 `final neu = NeuTheme.of(context);` 读取。

#### 7.1 外壳与公共组件迁移清单
1. **`flutter_app/lib/main.dart`**：
   - L125-141 (`_LanVaultAppState`)：监听 `AppearanceController`，包裹 `ThemeRevealHost` + `AppearanceScope`；L137 `theme: controller.themeData`；L134 `MaterialApp` 同步监听系统亮度变化。
   - L228, L232：`_openDeviceManager` 的 `Scaffold` / `AppBar` 背景改 `NeuTheme.of(routeCtx).canvas`。
   - L479-500：配对弹窗 `NeuIconBadge` 改用 `neu.accent` / `neu.accentSoft`。
   - L596 (`Scaffold.backgroundColor`)、L640 (`_MobileTopBar` 背景)、L668-681（设置圆形按钮底色/边框/阴影/图标色）改用 `neu`。
   - L714 (`_DesktopNavRail` 背景 `Color(0xFFFAFBFD)`) -> `neu.chromeSurface`；L716 -> `neu.hairlineBorder`；L755-758 -> `neu.primaryGradient` / `neu.primaryButtonShadows`；L794-820 -> `neu.insetSurface` / `neu.hairlineBorder` / `neu.healthy` / `neu.textSecondary`。
   - L851-922 (`_MobileNavBar`)：`Color(0x0A0F172A)` -> `neu.extrudedShadows`；L851, L853, L894, L906-907, L921-922 -> `neu`。
2. **`flutter_app/lib/widgets/nav_rail_item.dart`**：
   - L54-56：选中态前景色用 `neu.onAccent`（而非写死 `Colors.white`，适配暖白/鹅黄等浅强调色皮肤），未选中态用 `neu.textPrimary` / `neu.textSecondary`。
   - L101-111：`neu.insetSurface` / `neu.primaryGradient` / `neu.accent` / `neu.primaryButtonShadows`。
3. **`flutter_app/lib/widgets/device_switcher_pill.dart`**：
   - L55-100（顶栏胶囊）、L158 & L176（移动端 BottomSheet 背景与拖拽条）、L272-288 & L329（设备列表项与勾选图标）全部改用 `neu`。

#### 7.2 六大业务页迁移清单（按实测命中行号逐一替换）
1. **`flutter_app/lib/features/dashboard/dashboard_screen.dart`**（39 处 `NeuTokens.*`）：
   - L229-243、L282-296、L334-372、L420-434、L471-509、L555-574、L611-643、L694-796：全部改用 `final neu = NeuTheme.of(context);` 对应字段。
2. **`flutter_app/lib/features/backup/backup_screen.dart`**（34 处）：
   - L975-977、L1044-1047、L1202-1208、L1233-1246、L1288-1291、L1363-1364、L1426-1530、L1593-1653、L2313-2323、L2521-2529：全部改用 `neu`（L2529 `Colors.white` 改为 `neu.onAccent`）。
3. **`flutter_app/lib/features/clipboard/clipboard_screen.dart`**（35 处）：
   - L293 `Color(0xFFFAFBFD)` -> `neu.chromeSurface`；
   - L295, L356-403, L433-435, L519-546, L625-782 全部改用 `neu`（L726 发送气泡文字、L779 发送按钮图标改为 `neu.onAccent`）。
4. **`flutter_app/lib/features/pairing/pairing_screen.dart`**（15 处）：
   - L500-514, L762-763, L860-861, L1002-1023, L1308 全部改用 `neu`。
5. **`flutter_app/lib/features/snapshots/snapshots_screen.dart`**（37 处）：
   - L601-651, L829-842, L905-1075（含时间轴节点 `timelineInactive` / `timelineHalo` / `timelineLine` 与 L963 `neu.onAccent`）, L1121-1244, L1514 全部改用 `neu`。
6. **`flutter_app/lib/features/vault/vault_screen.dart`**（10 处）：
   - L342, L378-384, L437-443, L523-572 全部改用 `neu`。

#### 7.3 在 `flutter_app/lib/features/settings/settings_screen.dart` 迁移颜色并新增「外观与动效」专区
1. **颜色清理（13 处）**：
   - L191 (`NeuTokens.accent`)、L796-801（桌面设置左侧容器 `Color(0x0A0F172A)` 改 `neu.extrudedShadows`）、L928、L941-947、L1367（`Color(0x59000000)` 改 `neu.hairlineBorder`）、L1412, L1471, L1486, L1493 全部改为 `neu`。
2. **新增 `_SettingsSection.appearance`（「外观与动效」）**，对标 `D:\code\tolist\ToList\apps\web\src\pages\AppearancePage.tsx`：
   - 桌面端左侧分类（L810）在「常规与节点」之后增加 `(_SettingsSection.appearance, '外观与动效', Icons.palette_outlined)`，移动端滚动列表（L756 之后）增加 `_buildSectionHeader(context, '外观与动效')` + `_buildAppearanceGroup(context)`。
   - **外观与动效分组包含 4 项核心控件**：
     1. **日夜模式**：`NeuSegmentedControl<ThemeModePreference>`（跟随系统 / 日间 / 夜间），切换时通过 `ThemeRevealHost` 从控件中心触发圆形扩散。
     2. **当前皮肤与皮肤市场入口**：展示当前皮肤名称、双色预览圆点与「更换皮肤 / 恢复默认」按钮；点击打开皮肤市场弹窗（`showNeuDialog`，支持搜索框 `NeuInsetField`、全部分类筛选 `全部 / 基础 / 单色 / 双色`、色板卡片网格即时预览 + 圆形揭示切换，对应 `AppearancePage.tsx` L820-900）。
     3. **滑动与交互动画速度**：`NeuSegmentedControl<MotionSpeed>`（极速 / 均衡 / 优雅），实时调整 `NeuMotion` 速度倍率（`1.0 / 1.46 / 1.7`）。
     4. **双端一致性预览**：提示当前 Windows 标题栏/Android 系统栏已自动同步。

---

### Step 8：测试补充与验证

1. **修复并扩展 `flutter_app/test/widget_test.dart`**：
   - 检查 L937, 958, 978 等现有 `buildLanVaultTheme()` 调用保持向后兼容。
   - 新增至少 4 个测试用例：
     1. 日/夜模式切换后 `Scaffold` 背景色、`NeuTheme.surface` 与 `Brightness` 正确切换且不报错；
     2. 切换「赤陶纸」及高亮度单色皮肤（如「鹅黄」「云白」）时，`NeuButton(tone: primary)` 的前景色与背景色满足 WCAG 对比度 `>= 4.5`；
     3. `SlideSwitchViewport` 连续快速切换 Tab 时无异常且最终停留在最新目标页；
     4. `MediaQuery(disableAnimations: true)` 下日夜切换与页面切换正确跳过位移与截图揭示。
2. **在 `D:\code\nas\EveryoneNAS-Merge\flutter_app` 执行验证命令**：
   - `flutter analyze`（必须 0 error）
   - `flutter test`（必须全部通过）

---

## 3. Release 构建与发布流程

> 环境核对结果：`Flutter 3.50.0`、`cargo 1.100.0`、`cargo-ndk 4.1.2` 已就绪。

### 3.1 Windows Release 构建
在仓库根目录 `D:\code\nas\EveryoneNAS-Merge` 执行：
```powershell
powershell -ExecutionPolicy Bypass -File scripts\build_windows_release.ps1 -Clean
```
- 脚本行为（`scripts/build_windows_release.ps1` L58-145）：
  1. `cargo build --release -p lanvault-ffi -p lanvault-win-svc --target x86_64-pc-windows-msvc -j 16`
  2. `flutter build windows --release --obfuscate --split-debug-info=build/symbols/windows/0.2.0`
  3. 归档到 `dist\windows-release\` 并生成 `dist\EveryoneNAS-0.2.0-windows-x64.zip`。
- **实机冒烟验证**：启动 `dist\windows-release\lanvault_app.exe`（或实际可执行文件名），实测：
  - 标题栏日/夜按钮点击触发圆形扩散、Win32 DWM 边框同步变暗/变亮、窗口可正常拖动；
  - 皮肤市场切换「赤陶纸」及多款单/双色皮肤，6 个主页面与弹窗文字均清晰可读；
  - 侧栏与备份/设备二级页签切换呈现平滑滑动动画。

### 3.2 Android Release 构建
> **当前阻塞点提醒**：`scripts/build_android_release.ps1` L58-61 与 `flutter_app/android/app/build.gradle.kts` L10-14 强制要求 `flutter_app/android/key.properties` 存在才能执行正式签名 Release 构建；目前仓库中 `flutter_app\android\key.properties` **不存在**。

1. **前置配置**：先让用户提供或生成正式签名 keystore，并在 `flutter_app/android/key.properties` 写入 `storeFile` / `storePassword` / `keyAlias` / `keyPassword`（切勿提交到 Git）。
2. **执行构建**：
   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\build_android_release.ps1 -Clean
   ```
   - 编译 `arm64-v8a` 的 `liblanvault_ffi.so`（16KB 页对齐），构建混淆剥离符号的 `app-release.apk`，归档至 `dist\android-release\`。
3. **发布 Release**：
   - 使用 `gh release create`（或 GitHub Web）上传 `dist\EveryoneNAS-0.2.0-windows-x64.zip` 与 `dist\android-release\*.apk` 及校验和；
   - 若执行了 `backups\` 备份或产生临时测试文件，在提交前清理任务临时文件（保留用户需要的源码与发布物）。
