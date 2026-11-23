// 参考依据:
// - lanvault_complete_blueprint.md 第 3.5 / 5.1 节 (4+1 导航壳层与 HCI 认知负荷收敛)
// - ARCHITECTURE_CONTRACTS.md 第 6 节 (改良新拟态设计 Token 与响应式触控热区)
// - Apple HIG Motion / ConcentricRectangle / WCAG 2.2 AA

import 'dart:async';
import 'dart:ui';

import 'package:material_ui/material_ui.dart';
import 'package:flutter/services.dart';

import 'bridge/lanvault_bridge.dart';
import 'features/backup/backup_screen.dart';
import 'features/clipboard/clipboard_screen.dart';
import 'features/dashboard/dashboard_screen.dart';
import 'features/pairing/pairing_screen.dart';
import 'features/vault/vault_screen.dart';
import 'theme/neu_theme.dart';

void main() {
  runZonedGuarded<void>(
    () {
      WidgetsFlutterBinding.ensureInitialized();
      final bridge = LanVaultBridge();

      FlutterError.onError = (FlutterErrorDetails details) {
        FlutterError.presentError(details);
        unawaited(
          bridge.reportClientLog(
            level: 'ERROR',
            source: 'flutter.framework',
            message: details.exceptionAsString(),
            detail: details.stack?.toString(),
          ),
        );
      };

      PlatformDispatcher.instance.onError = (Object error, StackTrace stack) {
        unawaited(
          bridge.reportClientLog(
            level: 'FATAL',
            source: 'flutter.platform',
            message: error.toString(),
            detail: stack.toString(),
          ),
        );
        return true;
      };

      runApp(LanVaultApp(bridge: bridge));
    },
    (Object error, StackTrace stack) {
      // Unhandled async zone errors are forwarded if native FFI is initialized.
      debugPrint('LanVault ZoneError: $error\n$stack');
    },
  );
}

class LanVaultApp extends StatelessWidget {
  const LanVaultApp({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'EveryoneNAS',
      debugShowCheckedModeBanner: false,
      theme: buildLanVaultTheme(),
      home: LanVaultShell(bridge: bridge),
    );
  }
}

/// 固定顺序的 5 个功能页签，索引与 [IndexedStack] 子页严格一一对应。
enum _ShellTab {
  dashboard,
  pairing,
  backup,
  vault,
  clipboard,
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
/// 监听 [LanVaultBridge.roleModeNotifier] 动态裁剪桌面侧边栏与移动底栏可见页签，
/// 同时以固定顺序的 [IndexedStack] + [GlobalKey] 保持各页面状态不丢失。
class LanVaultShell extends StatefulWidget {
  const LanVaultShell({super.key, required this.bridge});

  final LanVaultBridge bridge;

  @override
  State<LanVaultShell> createState() => _LanVaultShellState();
}

class _LanVaultShellState extends State<LanVaultShell> {
  static const Map<_ShellTab, _NavItem> _allDestinations =
      <_ShellTab, _NavItem>{
    _ShellTab.dashboard: _NavItem(
      tab: _ShellTab.dashboard,
      label: '概览',
      icon: Icons.monitor_heart_outlined,
      selectedIcon: Icons.monitor_heart,
    ),
    _ShellTab.pairing: _NavItem(
      tab: _ShellTab.pairing,
      label: '配对',
      icon: Icons.devices_outlined,
      selectedIcon: Icons.devices,
    ),
    _ShellTab.backup: _NavItem(
      tab: _ShellTab.backup,
      label: '备份',
      icon: Icons.sync_outlined,
      selectedIcon: Icons.sync,
    ),
    _ShellTab.vault: _NavItem(
      tab: _ShellTab.vault,
      label: '空间',
      icon: Icons.storage_outlined,
      selectedIcon: Icons.storage,
    ),
    _ShellTab.clipboard: _NavItem(
      tab: _ShellTab.clipboard,
      label: '快传',
      icon: Icons.content_paste_outlined,
      selectedIcon: Icons.content_paste,
    ),
  };

  final GlobalKey _pagesStackKey = GlobalKey();
  late final List<Widget> _pages;
  _ShellTab _selectedTab = _ShellTab.dashboard;

  @override
  void initState() {
    super.initState();
    _pages = <Widget>[
      DashboardScreen(bridge: widget.bridge),
      PairingScreen(bridge: widget.bridge),
      BackupScreen(bridge: widget.bridge),
      VaultScreen(bridge: widget.bridge),
      ClipboardScreen(bridge: widget.bridge),
    ];
    widget.bridge.roleModeNotifier.addListener(_handleRoleModeChanged);
  }

  @override
  void didUpdateWidget(covariant LanVaultShell oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.bridge != widget.bridge) {
      oldWidget.bridge.roleModeNotifier.removeListener(_handleRoleModeChanged);
      widget.bridge.roleModeNotifier.addListener(_handleRoleModeChanged);
      _handleRoleModeChanged();
    }
  }

  @override
  void dispose() {
    widget.bridge.roleModeNotifier.removeListener(_handleRoleModeChanged);
    super.dispose();
  }

  static List<_NavItem> _visibleDestinationsFor(RoleMode mode) {
    final tabs = switch (mode) {
      RoleMode.clientOnly => const <_ShellTab>[
          _ShellTab.dashboard,
          _ShellTab.pairing,
          _ShellTab.backup,
          _ShellTab.clipboard,
        ],
      RoleMode.serverOnly => const <_ShellTab>[
          _ShellTab.dashboard,
          _ShellTab.vault,
          _ShellTab.pairing,
          _ShellTab.clipboard,
        ],
      RoleMode.dual => const <_ShellTab>[
          _ShellTab.dashboard,
          _ShellTab.pairing,
          _ShellTab.backup,
          _ShellTab.vault,
          _ShellTab.clipboard,
        ],
    };
    return tabs.map((t) => _allDestinations[t]!).toList(growable: false);
  }

  void _handleRoleModeChanged() {
    final visible = _visibleDestinationsFor(widget.bridge.currentRoleMode);
    final isStillVisible = visible.any((item) => item.tab == _selectedTab);
    setState(() {
      if (!isStillVisible) {
        _selectedTab = _ShellTab.dashboard;
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final isWide = MediaQuery.sizeOf(context).width >= 840;
    final visibleDestinations =
        _visibleDestinationsFor(widget.bridge.currentRoleMode);
    final visibleIndex =
        visibleDestinations.indexWhere((item) => item.tab == _selectedTab);
    final safeVisibleIndex = visibleIndex >= 0 ? visibleIndex : 0;
    final activeTab = visibleDestinations[safeVisibleIndex].tab;

    return Scaffold(
      body: SafeArea(
        child: Row(
          children: <Widget>[
            if (isWide)
              _DesktopNavRail(
                destinations: visibleDestinations,
                selectedIndex: safeVisibleIndex,
                onSelected: (index) =>
                    setState(() => _selectedTab = visibleDestinations[index].tab),
              ),
            Expanded(
              child: IndexedStack(
                key: _pagesStackKey,
                index: activeTab.index,
                children: _pages,
              ),
            ),
          ],
        ),
      ),
      bottomNavigationBar: isWide
          ? null
          : _MobileNavBar(
              destinations: visibleDestinations,
              selectedIndex: safeVisibleIndex,
              onSelected: (index) =>
                  setState(() => _selectedTab = visibleDestinations[index].tab),
            ),
    );
  }
}

class _DesktopNavRail extends StatelessWidget {
  const _DesktopNavRail({
    required this.destinations,
    required this.selectedIndex,
    required this.onSelected,
  });

  final List<_NavItem> destinations;
  final int selectedIndex;
  final ValueChanged<int> onSelected;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 212,
      padding: const EdgeInsets.all(NeuSpace.md),
      decoration: const BoxDecoration(
        color: NeuTokens.surface,
        border: Border(
          right: BorderSide(color: NeuTokens.hairlineBorder, width: 1),
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: <Widget>[
          Padding(
            padding: const EdgeInsets.symmetric(
              horizontal: NeuSpace.sm,
              vertical: NeuSpace.sm,
            ),
            child: Text(
              'EveryoneNAS',
              style: Theme.of(context).textTheme.headlineSmall,
            ),
          ),
          const SizedBox(height: NeuSpace.md),
          for (var i = 0; i < destinations.length; i++) ...<Widget>[
            if (destinations[i].tab == _ShellTab.clipboard) ...<Widget>[
              const Padding(
                padding: EdgeInsets.symmetric(vertical: NeuSpace.sm),
                child: Divider(height: 1),
              ),
            ],
            _NavRailButton(
              item: destinations[i],
              selected: i == selectedIndex,
              onTap: () => onSelected(i),
            ),
            const SizedBox(height: NeuSpace.sm),
          ],
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
  bool _focused = false;

  @override
  void initState() {
    super.initState();
    _focusNode.addListener(_handleFocusChange);
  }

  @override
  void dispose() {
    _focusNode.removeListener(_handleFocusChange);
    _focusNode.dispose();
    super.dispose();
  }

  void _handleFocusChange() {
    if (_focusNode.hasFocus != _focused) {
      setState(() => _focused = _focusNode.hasFocus);
    }
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
    final fg = widget.selected ? NeuTokens.accent : NeuTokens.textPrimary;
    final stateDuration = NeuMotion.resolveState(context);
    final stateCurve = NeuMotion.resolveStateCurve(context);
    final pressDuration = NeuMotion.resolvePress(context);
    final pressCurve = NeuMotion.resolvePressCurve(context);
    final reduced = NeuMotion.isReduced(context);
    final itemRadius = NeuRadius.concentric(NeuRadius.card, NeuSpace.sm);
    final targetScale = (_pressed && !reduced) ? NeuMotion.pressedScale : 1.0;

    final bgColor = widget.selected
        ? NeuTokens.insetSurface
        : (_hovered ? NeuTokens.hoverSurface : NeuTokens.surface);

    final baseShadows =
        widget.selected ? const <BoxShadow>[] : NeuTokens.controlShadows;
    final shadows = _focused
        ? <BoxShadow>[
            ...baseShadows,
            const BoxShadow(
              color: NeuTokens.accent,
              offset: Offset.zero,
              blurRadius: 0,
              spreadRadius: 2.0,
            ),
          ]
        : (baseShadows.isEmpty ? null : baseShadows);

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
              onTapDown: (_) {
                _focusNode.requestFocus();
                setState(() => _pressed = true);
              },
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
                  constraints: const BoxConstraints(
                    minHeight: NeuTokens.minTouchTarget,
                  ),
                  padding: const EdgeInsets.symmetric(
                    horizontal: 14,
                    vertical: 10,
                  ),
                  decoration: BoxDecoration(
                    color: bgColor,
                    borderRadius: BorderRadius.all(Radius.circular(itemRadius)),
                    border: Border.all(
                      color: _focused
                          ? NeuTokens.accent
                          : (widget.selected
                              ? NeuTokens.accent.withValues(alpha: 0.45)
                              : Colors.transparent),
                      width: _focused ? 1.5 : 1.0,
                    ),
                    boxShadow: shadows,
                  ),
                  child: Row(
                    children: <Widget>[
                      Icon(
                        widget.selected
                            ? widget.item.selectedIcon
                            : widget.item.icon,
                        size: 20,
                        color: fg,
                      ),
                      const SizedBox(width: 12),
                      Text(
                        widget.item.label,
                        style: Theme.of(context)
                            .textTheme
                            .labelLarge
                            ?.copyWith(color: fg),
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
    required this.selectedIndex,
    required this.onSelected,
  });

  final List<_NavItem> destinations;
  final int selectedIndex;
  final ValueChanged<int> onSelected;

  @override
  Widget build(BuildContext context) {
    final stateDuration = NeuMotion.resolveState(context);
    final stateCurve = NeuMotion.resolveStateCurve(context);
    final itemRadius = NeuRadius.concentric(NeuRadius.card, NeuSpace.sm);

    return Container(
      decoration: const BoxDecoration(
        color: NeuTokens.surface,
        border: Border(
          top: BorderSide(color: NeuTokens.hairlineBorder, width: 1),
        ),
      ),
      child: SafeArea(
        top: false,
        child: Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: NeuSpace.sm,
            vertical: NeuSpace.xs,
          ),
          child: Row(
            children: <Widget>[
              for (var i = 0; i < destinations.length; i++)
                Expanded(
                  child: RepaintBoundary(
                    child: Semantics(
                      button: true,
                      selected: i == selectedIndex,
                      label: destinations[i].label,
                      child: GestureDetector(
                        behavior: HitTestBehavior.opaque,
                        onTap: () => onSelected(i),
                        child: AnimatedContainer(
                          duration: stateDuration,
                          curve: stateCurve,
                          constraints: const BoxConstraints(
                            minHeight: NeuTokens.minMobileTouchTarget,
                          ),
                          margin: const EdgeInsets.symmetric(
                            horizontal: 2,
                            vertical: 2,
                          ),
                          padding: const EdgeInsets.symmetric(
                            vertical: NeuSpace.xs,
                          ),
                          decoration: BoxDecoration(
                            color: i == selectedIndex
                                ? NeuTokens.insetSurface
                                : Colors.transparent,
                            borderRadius: BorderRadius.all(
                              Radius.circular(itemRadius),
                            ),
                            border: Border.all(
                              color: i == selectedIndex
                                  ? NeuTokens.accent.withValues(alpha: 0.35)
                                  : Colors.transparent,
                            ),
                          ),
                          child: Column(
                            mainAxisSize: MainAxisSize.min,
                            mainAxisAlignment: MainAxisAlignment.center,
                            children: <Widget>[
                              Icon(
                                i == selectedIndex
                                    ? destinations[i].selectedIcon
                                    : destinations[i].icon,
                                size: 22,
                                color: i == selectedIndex
                                    ? NeuTokens.accent
                                    : NeuTokens.textSecondary,
                              ),
                              const SizedBox(height: 2),
                              Text(
                                destinations[i].label,
                                style: Theme.of(context)
                                    .textTheme
                                    .labelMedium
                                    ?.copyWith(
                                      color: i == selectedIndex
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
