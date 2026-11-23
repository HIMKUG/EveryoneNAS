import 'dart:async';
import 'dart:ui';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import 'bridge/lanvault_bridge.dart';
import 'features/backup/backup_screen.dart';
import 'features/clipboard/clipboard_screen.dart';
import 'features/dashboard/dashboard_screen.dart';
import 'features/pairing/pairing_screen.dart';
import 'features/settings/settings_screen.dart';
import 'features/snapshots/snapshots_screen.dart';
import 'features/vault/vault_screen.dart';
import 'theme/neu_theme.dart';
import 'theme/neu_toast.dart';

bool _isReportingError = false;
bool _bundledFontLoaded = false;

Future<void> _loadBundledAppFont() async {
  if (_bundledFontLoaded) return;
  try {
    final woff2 = await rootBundle.load('assets/fonts/SourceHanSansSC-Regular.woff2');
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
      runApp(LanVaultApp(bridge: bridge));
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
  const LanVaultApp({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<LanVaultApp> createState() => _LanVaultAppState();
}

class _LanVaultAppState extends State<LanVaultApp> {
  @override
  void dispose() {
    widget.bridge.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'EveryoneNAS',
      debugShowCheckedModeBanner: false,
      theme: buildLanVaultTheme(),
      home: LanVaultShell(bridge: widget.bridge),
    );
  }
}

/// 严格对齐设计稿顺序的 7 个功能页签。
enum _ShellTab {
  dashboard,
  devices,
  backup,
  storage,
  snapshots,
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

/// 跨平台自适应主导航壳层：
/// - 桌面端（>= 840px）：左侧 216px 固定导航栏，7 个导航项依次排布，激活项为实心蓝渐变胶囊。
/// - 移动端（< 840px）：顶部标题栏 + 右上角一键进入“设置”齿轮按钮；底部 6 项紧凑导航栏。
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
      tab: _ShellTab.devices,
      label: '设备',
      icon: Icons.devices_outlined,
      selectedIcon: Icons.devices_rounded,
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
      tab: _ShellTab.snapshots,
      label: '快照',
      icon: Icons.history_rounded,
      selectedIcon: Icons.history_rounded,
    ),
    _NavItem(
      tab: _ShellTab.transfer,
      label: '传输',
      icon: Icons.send_outlined,
      selectedIcon: Icons.send_rounded,
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
  late List<Widget> _pages;
  _ShellTab _selectedTab = _ShellTab.dashboard;
  String? _activePairDialogRequestId;
  final Set<String> _handledPairRequestIds = <String>{};

  List<Widget> _buildPages(LanVaultBridge bridge) {
    return <Widget>[
      DashboardScreen(
        bridge: bridge,
        onOpenSnapshots: () => setState(() => _selectedTab = _ShellTab.snapshots),
        onOpenVault: () => setState(() => _selectedTab = _ShellTab.storage),
        onOpenPairing: () => setState(() => _selectedTab = _ShellTab.devices),
      ),
      PairingScreen(bridge: bridge),
      BackupScreen(bridge: bridge),
      VaultScreen(bridge: bridge),
      SnapshotsScreen(bridge: bridge),
      ClipboardScreen(bridge: bridge),
      SettingsScreen(bridge: bridge),
    ];
  }

  @override
  void initState() {
    super.initState();
    _pages = _buildPages(widget.bridge);
    widget.bridge.pendingPairRequestsNotifier.addListener(_onPendingPairRequests);
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
    super.dispose();
  }

  Future<void> _hydratePersistedSettings() async {
    try {
      await widget.bridge.getRoleMode();
      await widget.bridge.getAutoReconnect();
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
                  NeuButton(
                    label: '拒绝',
                    tone: NeuButtonTone.neutral,
                    onPressed: submitting
                        ? null
                        : () async {
                            setDialogState(() => submitting = true);
                            resolved = true;
                            _handledPairRequestIds.add(request.requestId);
                            try {
                              await widget.bridge.respondPairRequest(
                                requestId: request.requestId,
                                accept: false,
                              );
                            } catch (_) {}
                            if (dialogContext.mounted) {
                              Navigator.of(dialogContext).pop();
                            }
                            if (mounted) {
                              NeuToast.info(
                                this.context,
                                '已拒绝 ${request.deviceName} 的配对请求',
                              );
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
                            resolved = true;
                            _handledPairRequestIds.add(request.requestId);
                            try {
                              await widget.bridge.respondPairRequest(
                                requestId: request.requestId,
                                accept: true,
                              );
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
                                Navigator.of(dialogContext).pop();
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
                      NeuIconBadge(
                        icon: request.osType.toLowerCase().contains('android')
                            ? Icons.phone_android_rounded
                            : Icons.computer_rounded,
                        color: NeuTokens.accent,
                        backgroundColor: NeuTokens.accentSoft,
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
    return _desktopDestinations
        .firstWhere((item) => item.tab == tab)
        .label;
  }

  Widget _buildAnimatedPageContent() {
    return RepaintBoundary(
      child: TweenAnimationBuilder<double>(
        key: ValueKey<_ShellTab>(_selectedTab),
        tween: Tween<double>(begin: 0.0, end: 1.0),
        duration: const Duration(milliseconds: 160),
        curve: Curves.linearToEaseOut,
        builder: (context, t, child) {
          final reduced = NeuMotion.isReduced(context);
          return Opacity(
            opacity: 0.92 + 0.08 * t,
            child: Transform.translate(
              offset: Offset(0, reduced ? 0 : (1 - t) * 5),
              child: child,
            ),
          );
        },
        child: IndexedStack(
          key: _pagesStackKey,
          index: _selectedTab.index,
          children: _pages,
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final isWide =
        MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;

    return Scaffold(
      backgroundColor: NeuTokens.canvas,
      body: SafeArea(
        child: isWide
            ? Row(
                children: <Widget>[
                  _DesktopNavRail(
                    destinations: _desktopDestinations,
                    selectedTab: _selectedTab,
                    onSelected: (tab) => setState(() => _selectedTab = tab),
                  ),
                  Expanded(child: _buildAnimatedPageContent()),
                ],
              )
            : Column(
                children: <Widget>[
                  _MobileTopBar(
                    title: _titleForTab(_selectedTab),
                    inSettings: _selectedTab == _ShellTab.settings,
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
              ),
      ),
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
  });

  final String title;
  final bool inSettings;
  final VoidCallback onToggleSettings;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 12, 16, 6),
      color: NeuTokens.canvas,
      child: Row(
        children: <Widget>[
          Expanded(
            child: Text(
              title,
              style: Theme.of(context).textTheme.headlineSmall?.copyWith(
                    fontSize: 24,
                    fontWeight: FontWeight.w800,
                  ),
            ),
          ),
          Semantics(
            button: true,
            label: inSettings ? '返回总览' : '打开设置',
            child: InkWell(
              onTap: onToggleSettings,
              borderRadius: const BorderRadius.all(Radius.circular(NeuRadius.pill)),
              child: Container(
                width: NeuTokens.minMobileTouchTarget,
                height: NeuTokens.minMobileTouchTarget,
                decoration: BoxDecoration(
                  color: inSettings ? NeuTokens.accentSoft : NeuTokens.surface,
                  shape: BoxShape.circle,
                  border: Border.all(
                    color: inSettings
                        ? NeuTokens.accent.withValues(alpha: 0.35)
                        : NeuTokens.hairlineBorder,
                  ),
                  boxShadow: NeuTokens.controlShadows,
                ),
                child: Icon(
                  inSettings
                      ? Icons.close_rounded
                      : Icons.settings_outlined,
                  size: 20,
                  color: inSettings
                      ? NeuTokens.accent
                      : NeuTokens.textPrimary,
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
    required this.destinations,
    required this.selectedTab,
    required this.onSelected,
  });

  final List<_NavItem> destinations;
  final _ShellTab selectedTab;
  final ValueChanged<_ShellTab> onSelected;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 216,
      padding: const EdgeInsets.fromLTRB(16, 20, 16, 18),
      decoration: const BoxDecoration(
        color: Color(0xFFFAFBFD),
        border: Border(
          right: BorderSide(color: NeuTokens.hairlineBorder, width: 1),
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 6),
            child: Row(
              children: <Widget>[
                const BrandLogoMark(size: 30),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: <Widget>[
                      Text(
                        'EveryoneNAS',
                        overflow: TextOverflow.ellipsis,
                        style: Theme.of(context).textTheme.titleMedium?.copyWith(
                              fontWeight: FontWeight.w800,
                              letterSpacing: -0.3,
                            ),
                      ),
                      Text(
                        'v${LanVaultBridge.appVersion}',
                        style: Theme.of(context).textTheme.labelMedium?.copyWith(
                              fontSize: 11,
                              color: NeuTokens.textSecondary,
                            ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(height: 22),
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
                        gradient: NeuTokens.primaryGradient,
                        borderRadius:
                            const BorderRadius.all(Radius.circular(12)),
                        boxShadow: NeuTokens.primaryButtonShadows,
                      ),
                    ),
                  ),
                  Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: <Widget>[
                      for (var i = 0; i < destinations.length; i++) ...<Widget>[
                        _NavRailButton(
                          item: destinations[i],
                          selected: destinations[i].tab == selectedTab,
                          onTap: () => onSelected(destinations[i].tab),
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
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
            decoration: BoxDecoration(
              color: NeuTokens.insetSurface,
              borderRadius: const BorderRadius.all(Radius.circular(12)),
              border: Border.all(color: NeuTokens.hairlineBorder),
            ),
            child: Row(
              children: <Widget>[
                const Icon(
                  Icons.verified_user_outlined,
                  size: 16,
                  color: NeuTokens.healthy,
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    '局域网零知识加密',
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context).textTheme.labelMedium?.copyWith(
                          color: NeuTokens.textSecondary,
                          fontSize: 11.5,
                        ),
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _NavRailButton extends StatefulWidget {
  const _NavRailButton({
    required this.item,
    required this.selected,
    required this.onTap,
  });

  final _NavItem item;
  final bool selected;
  final VoidCallback onTap;

  @override
  State<_NavRailButton> createState() => _NavRailButtonState();
}

class _NavRailButtonState extends State<_NavRailButton> {
  final FocusNode _focusNode = FocusNode(debugLabel: 'NavRailButton');
  bool _hovered = false;
  bool _pressed = false;

  @override
  void dispose() {
    _focusNode.dispose();
    super.dispose();
  }

  KeyEventResult _handleKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) return KeyEventResult.ignored;
    if (event.logicalKey == LogicalKeyboardKey.enter ||
        event.logicalKey == LogicalKeyboardKey.numpadEnter ||
        event.logicalKey == LogicalKeyboardKey.space) {
      widget.onTap();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final fg = widget.selected
        ? Colors.white
        : (_hovered ? NeuTokens.textPrimary : NeuTokens.textSecondary);
    final stateDuration = NeuMotion.resolveState(context);
    final stateCurve = NeuMotion.resolveStateCurve(context);
    final pressDuration = NeuMotion.resolvePress(context);
    final pressCurve = NeuMotion.resolvePressCurve(context);
    final reduced = NeuMotion.isReduced(context);
    final targetScale = (_pressed && !reduced) ? NeuMotion.pressedScale : 1.0;

    return RepaintBoundary(
      child: Semantics(
        button: true,
        selected: widget.selected,
        label: widget.item.label,
        child: Focus(
          focusNode: _focusNode,
          onKeyEvent: _handleKey,
          child: MouseRegion(
            cursor: SystemMouseCursors.click,
            onEnter: (_) => setState(() => _hovered = true),
            onExit: (_) => setState(() {
              _hovered = false;
              _pressed = false;
            }),
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTapDown: (_) => setState(() => _pressed = true),
              onTapUp: (_) => setState(() => _pressed = false),
              onTapCancel: () => setState(() => _pressed = false),
              onTap: widget.onTap,
              child: TweenAnimationBuilder<double>(
                tween: Tween<double>(end: targetScale),
                duration: pressDuration,
                curve: pressCurve,
                builder: (context, scale, child) {
                  return Transform.scale(
                    scale: scale,
                    alignment: Alignment.center,
                    child: child,
                  );
                },
                child: AnimatedContainer(
                  duration: stateDuration,
                  curve: stateCurve,
                  height: 44,
                  padding: const EdgeInsets.symmetric(horizontal: 14),
                  decoration: BoxDecoration(
                    color: (!widget.selected && _hovered)
                        ? NeuTokens.insetSurface
                        : Colors.transparent,
                    borderRadius: const BorderRadius.all(Radius.circular(12)),
                  ),
                  child: Row(
                    children: <Widget>[
                      Icon(
                        widget.selected
                            ? widget.item.selectedIcon
                            : widget.item.icon,
                        size: 19,
                        color: fg,
                      ),
                      const SizedBox(width: 11),
                      Text(
                        widget.item.label,
                        style: Theme.of(context).textTheme.labelLarge?.copyWith(
                              color: fg,
                              fontWeight: widget.selected
                                  ? FontWeight.w600
                                  : FontWeight.w500,
                            ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
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
    return Container(
      decoration: const BoxDecoration(
        color: NeuTokens.surface,
        border: Border(
          top: BorderSide(color: NeuTokens.hairlineBorder, width: 1),
        ),
        boxShadow: <BoxShadow>[
          BoxShadow(
            color: Color(0x0A0F172A),
            offset: Offset(0, -4),
            blurRadius: 12,
          ),
        ],
      ),
      child: SafeArea(
        top: false,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 6),
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
                              AnimatedScale(
                                scale: item.tab == selectedTab ? 1.12 : 1.0,
                                duration: NeuMotion.resolvePress(context),
                                curve: NeuMotion.resolvePressCurve(context),
                                child: Icon(
                                  item.tab == selectedTab
                                      ? item.selectedIcon
                                      : item.icon,
                                  size: 22,
                                  color: item.tab == selectedTab
                                      ? NeuTokens.accent
                                      : NeuTokens.textSecondary,
                                ),
                              ),
                              const SizedBox(height: 3),
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
                                      ? NeuTokens.accent
                                      : NeuTokens.textSecondary,
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
