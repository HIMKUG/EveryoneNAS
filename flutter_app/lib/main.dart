import 'dart:async';
import 'dart:io';
import 'dart:ui';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import 'bridge/lanvault_bridge.dart';
import 'bridge/window_chrome.dart';
import 'features/backup/backup_screen.dart';
import 'features/clipboard/clipboard_screen.dart';
import 'features/dashboard/dashboard_screen.dart';
import 'features/pairing/pairing_screen.dart';
import 'features/settings/settings_screen.dart';
import 'features/vault/vault_screen.dart';
import 'theme/neu_theme.dart';
import 'theme/neu_toast.dart';
import 'theme/appearance_controller.dart';
import 'theme/motion_slide.dart';
import 'theme/theme_reveal.dart';
import 'widgets/device_switcher_pill.dart';
import 'widgets/nav_rail_item.dart';
import 'widgets/windows_title_bar.dart';

bool _isReportingError = false;
bool _bundledFontLoaded = false;

Future<void> _loadBundledAppFont() async {
  if (_bundledFontLoaded) return;
  try {
    final woff2 =
        await rootBundle.load('assets/fonts/SourceHanSansSC-Regular.woff2');
    final sfnt = await LanVaultBridge.decodeWoff2ToSfnt(
      woff2.buffer.asUint8List(woff2.offsetInBytes, woff2.lengthInBytes),
    );
    if (sfnt.isEmpty) return;
    final loader = FontLoader(NeuTokens.appFontFamily);
    loader.addFont(Future<ByteData>.value(ByteData.sublistView(sfnt)));
    await loader.load();
    _bundledFontLoaded = true;
  } catch (error) {
    debugPrint('SourceHanSansSC WOFF2 font load skipped: $error');
  }
}

void _safeReportUnhandledError(
  LanVaultBridge? bridge, {
  required String level,
  required String source,
  required String message,
  String? detail,
}) {
  if (bridge == null || _isReportingError) return;
  _isReportingError = true;
  unawaited(
    bridge
        .reportClientLog(
          level: level,
          source: source,
          message: message,
          detail: detail,
        )
        .catchError((Object _) {})
        .whenComplete(() {
      _isReportingError = false;
    }),
  );
}

void main() {
  LanVaultBridge? appBridge;

  runZonedGuarded<void>(
    () async {
      WidgetsFlutterBinding.ensureInitialized();
      final bridge = LanVaultBridge();
      appBridge = bridge;

      FlutterError.onError = (FlutterErrorDetails details) {
        FlutterError.presentError(details);
        _safeReportUnhandledError(
          bridge,
          level: 'ERROR',
          source: 'flutter.framework',
          message: details.exceptionAsString(),
          detail: details.stack?.toString(),
        );
      };

      PlatformDispatcher.instance.onError = (Object error, StackTrace stack) {
        _safeReportUnhandledError(
          bridge,
          level: 'FATAL',
          source: 'flutter.platform',
          message: error.toString(),
          detail: stack.toString(),
        );
        return true;
      };

      await _loadBundledAppFont().timeout(
        const Duration(seconds: 3),
        onTimeout: () {},
      );
      final appearance = await AppearanceController.preload().timeout(
        const Duration(milliseconds: 300),
        onTimeout: () => AppearanceController.memory(observeSystem: false),
      );
      appearance.resumeSystemObservation();
      runApp(LanVaultApp(bridge: bridge, appearance: appearance));
    },
    (Object error, StackTrace stack) {
      debugPrint('EveryoneNAS ZoneError: $error\n$stack');
      _safeReportUnhandledError(
        appBridge,
        level: 'FATAL',
        source: 'flutter.zone',
        message: error.toString(),
        detail: stack.toString(),
      );
    },
  );
}

class LanVaultApp extends StatefulWidget {
  const LanVaultApp({
    super.key,
    required this.bridge,
    this.appearance,
  });

  final LanVaultBridge bridge;
  final AppearanceController? appearance;

  @override
  State<LanVaultApp> createState() => _LanVaultAppState();
}

class _LanVaultAppState extends State<LanVaultApp> {
  late final AppearanceController _appearance =
      widget.appearance ?? AppearanceController();

  @override
  void dispose() {
    if (widget.appearance == null) {
      _appearance.dispose();
    }
    widget.bridge.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _appearance,
      builder: (context, _) {
        final theme = _appearance.themeData;
        unawaited(_applySystemChrome(theme));
        return ThemeRevealHost(
          child: AppearanceScope(
            notifier: _appearance,
            child: MaterialApp(
              title: 'EveryoneNAS',
              debugShowCheckedModeBanner: false,
              theme: theme,
              themeMode: ThemeMode.light,
              home: LanVaultShell(bridge: widget.bridge),
            ),
          ),
        );
      },
    );
  }

  Future<void> _applySystemChrome(ThemeData theme) async {
    final neu = theme.extension<NeuTheme>();
    if (neu == null) {
      return;
    }
    final dark = neu.isDark;
    if (Platform.isAndroid) {
      SystemChrome.setSystemUIOverlayStyle(
        SystemUiOverlayStyle(
          statusBarIconBrightness: dark ? Brightness.light : Brightness.dark,
          statusBarColor: Colors.transparent,
          systemNavigationBarColor: neu.surface,
          systemNavigationBarIconBrightness:
              dark ? Brightness.light : Brightness.dark,
        ),
      );
    }
    if (Platform.isWindows) {
      unawaited(WindowChrome.setDarkMode(dark));
    }
  }
}

/// 收敛后的 5 个顶级目的地：
/// - 桌面侧栏展示全部 5 项（总览 / 备份 / 存储 / 传输 / 设置）；
/// - 移动端底栏严控 4 项（总览 / 备份 / 存储 / 传输），右上角齿轮进入设置；
/// - 设备切换升格为顶栏全局胶囊，精细设备管理与配对从胶囊底一键进入；
/// - 历史版本与完整性校验收口进“备份”页内二级视角。
enum _ShellTab {
  dashboard,
  backup,
  storage,
  transfer,
  settings,
}

class _NavItem {
  const _NavItem({
    required this.tab,
    required this.label,
    required this.icon,
    required this.selectedIcon,
  });

  final _ShellTab tab;
  final String label;
  final IconData icon;
  final IconData selectedIcon;
}

class LanVaultShell extends StatefulWidget {
  const LanVaultShell({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<LanVaultShell> createState() => _LanVaultShellState();
}

class _LanVaultShellState extends State<LanVaultShell> {
  static const List<_NavItem> _desktopDestinations = <_NavItem>[
    _NavItem(
      tab: _ShellTab.dashboard,
      label: '总览',
      icon: Icons.home_outlined,
      selectedIcon: Icons.home_rounded,
    ),
    _NavItem(
      tab: _ShellTab.backup,
      label: '备份',
      icon: Icons.cloud_sync_outlined,
      selectedIcon: Icons.cloud_sync_rounded,
    ),
    _NavItem(
      tab: _ShellTab.storage,
      label: '存储',
      icon: Icons.layers_outlined,
      selectedIcon: Icons.layers_rounded,
    ),
    _NavItem(
      tab: _ShellTab.transfer,
      label: '传输',
      icon: Icons.chat_bubble_outline_rounded,
      selectedIcon: Icons.chat_bubble_rounded,
    ),
    _NavItem(
      tab: _ShellTab.settings,
      label: '设置',
      icon: Icons.settings_outlined,
      selectedIcon: Icons.settings_rounded,
    ),
  ];

  static final List<_NavItem> _mobileDestinations = _desktopDestinations
      .where((item) => item.tab != _ShellTab.settings)
      .toList(growable: false);

  final GlobalKey _pagesStackKey = GlobalKey();
  final ValueNotifier<int> _backupSubTabNotifier = ValueNotifier<int>(0);
  late List<Widget> _pages;
  _ShellTab _selectedTab = _ShellTab.dashboard;
  String? _activePairDialogRequestId;
  final Set<String> _handledPairRequestIds = <String>{};

  void _openDeviceManager() {
    Navigator.of(context).push(
      NeuSlidePageRoute<void>(
        builder: (routeCtx) => Scaffold(
          backgroundColor: NeuTheme.of(context).canvas,
          appBar: WindowChrome.isSupported
              ? null
              : AppBar(
                  backgroundColor: NeuTheme.of(context).canvas,
                  surfaceTintColor: Colors.transparent,
                  elevation: 0,
                  title: const Text('设备管理'),
                  leading: IconButton(
                    tooltip: '返回',
                    icon: const Icon(Icons.arrow_back_rounded),
                    onPressed: () => Navigator.of(routeCtx).pop(),
                  ),
                ),
          body: WindowChrome.isSupported
              ? Column(
                  children: <Widget>[
                    const WindowsTitleBar(),
                    Padding(
                      padding: const EdgeInsets.fromLTRB(16, 10, 16, 0),
                      child: Row(
                        children: <Widget>[
                          NeuButton(
                            label: '返回',
                            icon: Icons.arrow_back_rounded,
                            onPressed: () => Navigator.of(routeCtx).pop(),
                          ),
                          const SizedBox(width: 12),
                          Text(
                            '设备管理',
                            style: Theme.of(routeCtx).textTheme.titleLarge,
                          ),
                        ],
                      ),
                    ),
                    Expanded(child: PairingScreen(bridge: widget.bridge)),
                  ],
                )
              : PairingScreen(bridge: widget.bridge),
        ),
      ),
    );
  }

  List<Widget> _buildPages(LanVaultBridge bridge) {
    return <Widget>[
      DashboardScreen(
        bridge: bridge,
        onOpenSnapshots: () {
          _backupSubTabNotifier.value = 1;
          setState(() => _selectedTab = _ShellTab.backup);
        },
        onOpenVault: () => setState(() => _selectedTab = _ShellTab.storage),
        onOpenPairing: _openDeviceManager,
        onOpenSettings: () => setState(() => _selectedTab = _ShellTab.settings),
      ),
      BackupScreen(
        bridge: bridge,
        subTabNotifier: _backupSubTabNotifier,
      ),
      VaultScreen(
        bridge: bridge,
        onOpenSettings: () => setState(() => _selectedTab = _ShellTab.settings),
      ),
      ClipboardScreen(
        bridge: bridge,
        onOpenDeviceManager: _openDeviceManager,
      ),
      SettingsScreen(bridge: bridge),
    ];
  }

  @override
  void initState() {
    super.initState();
    _pages = _buildPages(widget.bridge);
    widget.bridge.pendingPairRequestsNotifier
        .addListener(_onPendingPairRequests);
    unawaited(_hydratePersistedSettings());
  }

  @override
  void didUpdateWidget(covariant LanVaultShell oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.bridge != widget.bridge) {
      oldWidget.bridge.pendingPairRequestsNotifier
          .removeListener(_onPendingPairRequests);
      widget.bridge.pendingPairRequestsNotifier
          .addListener(_onPendingPairRequests);
      _pages = _buildPages(widget.bridge);
    }
  }

  @override
  void dispose() {
    widget.bridge.pendingPairRequestsNotifier
        .removeListener(_onPendingPairRequests);
    _backupSubTabNotifier.dispose();
    super.dispose();
  }

  Future<void> _hydratePersistedSettings() async {
    try {
      final role = await widget.bridge.getRoleMode();
      await widget.bridge.getAutoReconnect();
      await widget.bridge.refreshPairedPeers();
      if (role.hasServer && !widget.bridge.serverRunningNotifier.value) {
        final autostart = await widget.bridge.getAutostartServer();
        if (autostart) {
          await widget.bridge.autostartLocalServer();
        }
      }
    } catch (_) {
      // 持久化读取失败时保留默认内存状态。
    }
  }

  void _onPendingPairRequests() {
    if (!mounted) return;
    if (_activePairDialogRequestId != null) return;
    final pending = widget.bridge.pendingPairRequestsNotifier.value;
    PendingPairRequestModel? nextRequest;
    for (final item in pending) {
      if (!_handledPairRequestIds.contains(item.requestId)) {
        nextRequest = item;
        break;
      }
    }
    if (nextRequest == null) return;

    final chosen = nextRequest;
    _activePairDialogRequestId = chosen.requestId;
    Future<void>.microtask(() {
      if (!mounted) return;
      unawaited(_showIncomingPairDialog(chosen));
    });
  }

  Future<void> _showIncomingPairDialog(PendingPairRequestModel request) async {
    var submitting = false;
    var resolved = false;
    var hasError = false;
    try {
      await showNeuDialog<void>(
        context: context,
        barrierDismissible: false,
        builder: (dialogContext) {
          return StatefulBuilder(
            builder: (context, setDialogState) {
              return NeuDialog(
                title: '收到设备配对请求',
                subtitle: '确认后对方设备将自动完成安全配对，无需手动输入配对码',
                actions: <Widget>[
                  if (hasError)
                    NeuButton(
                      label: '关闭',
                      tone: NeuButtonTone.neutral,
                      onPressed: submitting
                          ? null
                          : () {
                              if (dialogContext.mounted) {
                                Navigator.of(dialogContext).pop();
                              }
                            },
                    ),
                  NeuButton(
                    label: '拒绝',
                    tone: NeuButtonTone.neutral,
                    onPressed: submitting
                        ? null
                        : () async {
                            setDialogState(() => submitting = true);
                            try {
                              await widget.bridge.respondPairRequest(
                                requestId: request.requestId,
                                accept: false,
                              );
                              resolved = true;
                              _handledPairRequestIds.add(request.requestId);
                              if (dialogContext.mounted) {
                                Navigator.of(dialogContext).pop();
                              }
                              if (mounted) {
                                NeuToast.info(
                                  this.context,
                                  '已拒绝 ${request.deviceName} 的配对请求',
                                );
                              }
                            } catch (error) {
                              if (dialogContext.mounted) {
                                setDialogState(() {
                                  submitting = false;
                                  hasError = true;
                                });
                              }
                              if (mounted) {
                                NeuToast.error(
                                  this.context,
                                  LanVaultBridge.formatUserError(error),
                                );
                              }
                            }
                          },
                  ),
                  NeuButton(
                    label: '同意配对',
                    icon: Icons.verified_user_rounded,
                    tone: NeuButtonTone.primary,
                    loading: submitting,
                    onPressed: submitting
                        ? null
                        : () async {
                            setDialogState(() => submitting = true);
                            try {
                              await widget.bridge.respondPairRequest(
                                requestId: request.requestId,
                                accept: true,
                              );
                              resolved = true;
                              _handledPairRequestIds.add(request.requestId);
                              if (dialogContext.mounted) {
                                Navigator.of(dialogContext).pop();
                              }
                              if (mounted) {
                                NeuToast.success(
                                  this.context,
                                  '已同意与 ${request.deviceName} 配对',
                                );
                              }
                            } catch (error) {
                              if (dialogContext.mounted) {
                                setDialogState(() {
                                  submitting = false;
                                  hasError = true;
                                });
                              }
                              if (mounted) {
                                NeuToast.error(
                                  this.context,
                                  LanVaultBridge.formatUserError(error),
                                );
                              }
                            }
                          },
                  ),
                ],
                child: NeuCard(
                  inset: true,
                  padding: const EdgeInsets.all(14),
                  child: Row(
                    children: <Widget>[
                      Builder(
                        builder: (badgeContext) => NeuIconBadge(
                          icon: request.osType.toLowerCase().contains('android')
                              ? Icons.phone_android_rounded
                              : Icons.computer_rounded,
                          color: NeuTheme.of(badgeContext).accent,
                          backgroundColor: NeuTheme.of(badgeContext).accentSoft,
                        ),
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: <Widget>[
                            Text(
                              request.deviceName,
                              style: Theme.of(context).textTheme.titleSmall,
                            ),
                            const SizedBox(height: 2),
                            Text(
                              request.fingerprint.isEmpty
                                  ? '${request.osType} · ${request.remoteIp}'
                                  : '${request.osType} · ${request.remoteIp} · 指纹 ${request.fingerprint}',
                              style: Theme.of(context).textTheme.bodySmall,
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              );
            },
          );
        },
      );
    } finally {
      if (!resolved) {
        _handledPairRequestIds.add(request.requestId);
      }
      _activePairDialogRequestId = null;
      _onPendingPairRequests();
    }
  }

  String _titleForTab(_ShellTab tab) {
    return _desktopDestinations.firstWhere((item) => item.tab == tab).label;
  }

  Widget _buildAnimatedPageContent() {
    return SlideSwitchViewport(
      key: _pagesStackKey,
      index: _selectedTab.index,
      axis: MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide
          ? Axis.vertical
          : Axis.horizontal,
      children: _pages,
    );
  }

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final isWide = MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;
    final hasCustomChrome = WindowChrome.isSupported;

    final Widget content = isWide
        ? Row(
            children: <Widget>[
              _DesktopNavRail(
                bridge: widget.bridge,
                destinations: _desktopDestinations,
                selectedTab: _selectedTab,
                showBrandHeader: !hasCustomChrome,
                onSelected: (tab) => setState(() => _selectedTab = tab),
                onOpenDeviceManager: _openDeviceManager,
              ),
              Expanded(child: _buildAnimatedPageContent()),
            ],
          )
        : Column(
            children: <Widget>[
              _MobileTopBar(
                title: _titleForTab(_selectedTab),
                inSettings: _selectedTab == _ShellTab.settings,
                devicePill: _selectedTab == _ShellTab.settings
                    ? null
                    : DeviceSwitcherPill(
                        bridge: widget.bridge,
                        compact: true,
                        onOpenDeviceManager: _openDeviceManager,
                      ),
                onToggleSettings: () {
                  setState(() {
                    _selectedTab = _selectedTab == _ShellTab.settings
                        ? _ShellTab.dashboard
                        : _ShellTab.settings;
                  });
                },
              ),
              Expanded(child: _buildAnimatedPageContent()),
            ],
          );

    return Scaffold(
      backgroundColor: neu.canvas,
      body: hasCustomChrome
          ? Column(
              children: <Widget>[
                WindowsTitleBar(
                  centerWidget: DeviceSwitcherPill(
                    bridge: widget.bridge,
                    onOpenDeviceManager: _openDeviceManager,
                  ),
                ),
                Expanded(
                  child: SafeArea(top: false, child: content),
                ),
              ],
            )
          : SafeArea(child: content),
      bottomNavigationBar: isWide
          ? null
          : _MobileNavBar(
              destinations: _mobileDestinations,
              selectedTab: _selectedTab,
              onSelected: (tab) => setState(() => _selectedTab = tab),
            ),
    );
  }
}

class _MobileTopBar extends StatelessWidget {
  const _MobileTopBar({
    required this.title,
    required this.inSettings,
    required this.onToggleSettings,
    this.devicePill,
  });

  final String title;
  final bool inSettings;
  final VoidCallback onToggleSettings;
  final Widget? devicePill;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 10, 16, 6),
      color: neu.canvas,
      child: Row(
        children: <Widget>[
          Text(
            title,
            style: Theme.of(context).textTheme.headlineSmall?.copyWith(
                  fontSize: 23,
                  fontWeight: FontWeight.w800,
                ),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: devicePill != null
                ? Center(child: devicePill!)
                : const SizedBox.shrink(),
          ),
          const SizedBox(width: 10),
          Semantics(
            button: true,
            label: neu.isDark ? '切换日间模式' : '切换夜间模式',
            child: InkWell(
              onTap: () => ThemeRevealHost.of(context).reveal(
                context,
                () => AppearanceScope.of(context).toggleLightDark(),
              ),
              borderRadius:
                  const BorderRadius.all(Radius.circular(NeuRadius.pill)),
              child: Container(
                width: NeuTokens.minMobileTouchTarget,
                height: NeuTokens.minMobileTouchTarget,
                decoration: BoxDecoration(
                  color: neu.surface,
                  shape: BoxShape.circle,
                  border: Border.all(color: neu.hairlineBorder),
                  boxShadow: neu.controlShadows,
                ),
                child: Icon(
                  neu.isDark
                      ? Icons.light_mode_outlined
                      : Icons.dark_mode_outlined,
                  size: 20,
                  color: neu.textPrimary,
                ),
              ),
            ),
          ),
          const SizedBox(width: 8),
          Semantics(
            button: true,
            label: inSettings ? '返回总览' : '打开设置',
            child: InkWell(
              onTap: onToggleSettings,
              borderRadius:
                  const BorderRadius.all(Radius.circular(NeuRadius.pill)),
              child: Container(
                width: NeuTokens.minMobileTouchTarget,
                height: NeuTokens.minMobileTouchTarget,
                decoration: BoxDecoration(
                  color: inSettings ? neu.accentSoft : neu.surface,
                  shape: BoxShape.circle,
                  border: Border.all(
                    color: inSettings
                        ? neu.accent.withValues(alpha: 0.35)
                        : neu.hairlineBorder,
                  ),
                  boxShadow: neu.controlShadows,
                ),
                child: Icon(
                  inSettings ? Icons.close_rounded : Icons.settings_outlined,
                  size: 20,
                  color: inSettings ? neu.accent : neu.textPrimary,
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _DesktopNavRail extends StatelessWidget {
  const _DesktopNavRail({
    required this.bridge,
    required this.destinations,
    required this.selectedTab,
    required this.onSelected,
    required this.onOpenDeviceManager,
    this.showBrandHeader = true,
  });

  final LanVaultBridge bridge;
  final List<_NavItem> destinations;
  final _ShellTab selectedTab;
  final ValueChanged<_ShellTab> onSelected;
  final VoidCallback onOpenDeviceManager;
  final bool showBrandHeader;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    return Container(
      width: 212,
      padding: const EdgeInsets.fromLTRB(14, 18, 14, 16),
      decoration: BoxDecoration(
        color: neu.chromeSurface,
        border: Border(
          right: BorderSide(color: neu.hairlineBorder, width: 1),
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          if (showBrandHeader) ...<Widget>[
            const Padding(
              padding: EdgeInsets.symmetric(horizontal: 6, vertical: 6),
              child: Align(
                alignment: Alignment.centerLeft,
                child: BrandLogoFull(height: 36),
              ),
            ),
            const SizedBox(height: 12),
            DeviceSwitcherPill(
              bridge: bridge,
              onOpenDeviceManager: onOpenDeviceManager,
            ),
            const SizedBox(height: 18),
          ],
          Builder(
            builder: (context) {
              const itemHeight = 44.0;
              const gap = 6.0;
              final selectedIdx = destinations
                  .indexWhere((d) => d.tab == selectedTab)
                  .clamp(0, destinations.length - 1);
              return Stack(
                children: <Widget>[
                  AnimatedPositioned(
                    duration: NeuMotion.resolveState(context),
                    curve: NeuMotion.resolveStateCurve(context),
                    top: selectedIdx * (itemHeight + gap),
                    left: 0,
                    right: 0,
                    height: itemHeight,
                    child: DecoratedBox(
                      decoration: BoxDecoration(
                        gradient: neu.primaryGradient,
                        borderRadius:
                            const BorderRadius.all(Radius.circular(12)),
                        boxShadow: neu.primaryButtonShadows,
                      ),
                    ),
                  ),
                  Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: <Widget>[
                      for (var i = 0; i < destinations.length; i++) ...<Widget>[
                        NavRailItem(
                          label: destinations[i].label,
                          icon: destinations[i].icon,
                          selectedIcon: destinations[i].selectedIcon,
                          selected: destinations[i].tab == selectedTab,
                          onTap: () => onSelected(destinations[i].tab),
                          paintSelectedSurface: false,
                        ),
                        if (i < destinations.length - 1)
                          const SizedBox(height: gap),
                      ],
                    ],
                  ),
                ],
              );
            },
          ),
          const Spacer(),
          ValueListenableBuilder<bool>(
            valueListenable: bridge.serverRunningNotifier,
            builder: (context, running, _) {
              return InkWell(
                onTap: () => onSelected(_ShellTab.settings),
                borderRadius: const BorderRadius.all(Radius.circular(12)),
                child: Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
                  decoration: BoxDecoration(
                    color: neu.insetSurface,
                    borderRadius: const BorderRadius.all(Radius.circular(12)),
                    border: Border.all(color: neu.hairlineBorder),
                  ),
                  child: Row(
                    children: <Widget>[
                      Container(
                        width: 8,
                        height: 8,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: running
                              ? neu.healthy
                              : neu.textSecondary.withValues(alpha: 0.4),
                        ),
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        child: Text(
                          running
                              ? '本地服务运行中 :${LanVaultBridge.defaultServerPort}'
                              : '本地服务未启动',
                          overflow: TextOverflow.ellipsis,
                          style:
                              Theme.of(context).textTheme.labelMedium?.copyWith(
                                    color: neu.textSecondary,
                                    fontSize: 11.5,
                                  ),
                        ),
                      ),
                    ],
                  ),
                ),
              );
            },
          ),
        ],
      ),
    );
  }
}

class _MobileNavBar extends StatelessWidget {
  const _MobileNavBar({
    required this.destinations,
    required this.selectedTab,
    required this.onSelected,
  });

  final List<_NavItem> destinations;
  final _ShellTab selectedTab;
  final ValueChanged<_ShellTab> onSelected;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    return Container(
      decoration: BoxDecoration(
        color: neu.surface,
        border: Border(
          top: BorderSide(color: neu.hairlineBorder, width: 1),
        ),
        boxShadow: neu.extrudedShadows,
      ),
      child: SafeArea(
        top: false,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
          child: Row(
            children: <Widget>[
              for (final item in destinations)
                Expanded(
                  child: RepaintBoundary(
                    child: Semantics(
                      button: true,
                      selected: item.tab == selectedTab,
                      label: item.label,
                      child: GestureDetector(
                        behavior: HitTestBehavior.opaque,
                        onTap: () => onSelected(item.tab),
                        child: Padding(
                          padding: const EdgeInsets.symmetric(vertical: 4),
                          child: Column(
                            mainAxisSize: MainAxisSize.min,
                            mainAxisAlignment: MainAxisAlignment.center,
                            children: <Widget>[
                              AnimatedContainer(
                                duration: NeuMotion.resolveState(context),
                                curve: NeuMotion.resolveStateCurve(context),
                                padding: const EdgeInsets.symmetric(
                                  horizontal: 16,
                                  vertical: 5,
                                ),
                                decoration: BoxDecoration(
                                  color: item.tab == selectedTab
                                      ? neu.accentSoft
                                      : Colors.transparent,
                                  borderRadius: const BorderRadius.all(
                                    Radius.circular(NeuRadius.pill),
                                  ),
                                ),
                                child: Icon(
                                  item.tab == selectedTab
                                      ? item.selectedIcon
                                      : item.icon,
                                  size: 22,
                                  color: item.tab == selectedTab
                                      ? neu.accent
                                      : neu.textSecondary,
                                ),
                              ),
                              const SizedBox(height: 2),
                              Text(
                                item.label,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                  fontSize: 11,
                                  fontWeight: item.tab == selectedTab
                                      ? FontWeight.w700
                                      : FontWeight.w500,
                                  color: item.tab == selectedTab
                                      ? neu.accent
                                      : neu.textSecondary,
                                ),
                              ),
                            ],
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}
