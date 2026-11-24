import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../bridge/window_chrome.dart';
import '../theme/neu_theme.dart';

/// Windows 桌面端的自绘标题栏。
///
/// 支持居中嵌入可交互控件（如全局设备切换胶囊），并将其与右侧窗口按钮的矩形
/// 一并回传给 Win32 原生侧作为客户区排除项，保证其余空白区域仍是原生拖动区。
class WindowsTitleBar extends StatefulWidget {
  const WindowsTitleBar({super.key, this.centerWidget});

  /// 标题栏高度（逻辑像素），同时决定原生侧拖动条带的高度。
  static const double height = 46.0;

  /// 居中显示的交互控件（例如全局设备切换胶囊）。
  final Widget? centerWidget;

  @override
  State<WindowsTitleBar> createState() => _WindowsTitleBarState();
}

class _WindowsTitleBarState extends State<WindowsTitleBar> {
  final GlobalKey _controlsKey = GlobalKey(debugLabel: 'CaptionControls');
  final GlobalKey _centerKey = GlobalKey(debugLabel: 'CaptionCenter');
  bool _maximized = false;
  double? _reportedHeight;
  List<Rect> _reportedExclusions = const <Rect>[];

  @override
  void initState() {
    super.initState();
    WindowChrome.attach(onMaximizedChanged: _onMaximizedChanged);
    unawaited(
      WindowChrome.isMaximized().then((value) {
        if (!mounted || value == _maximized) return;
        setState(() => _maximized = value);
      }),
    );
  }

  @override
  void dispose() {
    WindowChrome.detach();
    super.dispose();
  }

  void _onMaximizedChanged(bool value) {
    if (!mounted || value == _maximized) return;
    setState(() => _maximized = value);
  }

  void _syncCaptionMetrics() {
    final RenderBox? root = context.findRenderObject() as RenderBox?;
    if (root == null || !root.hasSize) return;

    final List<Rect> exclusions = <Rect>[];
    for (final key in <GlobalKey>[_controlsKey, _centerKey]) {
      final RenderObject? box = key.currentContext?.findRenderObject();
      if (box is RenderBox && box.hasSize) {
        final Offset origin =
            root.globalToLocal(box.localToGlobal(Offset.zero));
        exclusions.add(origin & box.size);
      }
    }

    final double height = root.size.height;
    if (height == _reportedHeight &&
        listEquals<Rect>(exclusions, _reportedExclusions)) {
      return;
    }
    _reportedHeight = height;
    _reportedExclusions = exclusions;
    unawaited(
      WindowChrome.setCaptionMetrics(
        captionHeight: height,
        exclusions: exclusions,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _syncCaptionMetrics();
    });

    return MouseRegion(
      cursor: SystemMouseCursors.basic,
      child: Container(
        width: MediaQuery.sizeOf(context).width,
        height: WindowsTitleBar.height,
        padding: const EdgeInsets.only(left: 14, right: 8),
        decoration: const BoxDecoration(
          color: Color(0xFFFAFBFD),
          border: Border(
            bottom: BorderSide(color: NeuTokens.hairlineBorder, width: 1),
          ),
        ),
        child: Stack(
          alignment: Alignment.center,
          children: <Widget>[
            Row(
              children: <Widget>[
                const Expanded(
                  child: Align(
                    alignment: Alignment.centerLeft,
                    child: BrandLogoFull(height: 32),
                  ),
                ),
                Padding(
                  key: _controlsKey,
                  padding:
                      const EdgeInsets.symmetric(horizontal: 4, vertical: 4),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: <Widget>[
                      _CaptionButton(
                        icon: Icons.remove_rounded,
                        label: '最小化',
                        onPressed: () => unawaited(WindowChrome.minimize()),
                      ),
                      _CaptionButton(
                        icon: _maximized
                            ? Icons.filter_none_rounded
                            : Icons.crop_square_rounded,
                        label: _maximized ? '向下还原' : '最大化',
                        iconSize: 14,
                        onPressed: () =>
                            unawaited(WindowChrome.toggleMaximize()),
                      ),
                      _CaptionButton(
                        icon: Icons.close_rounded,
                        label: '关闭',
                        destructive: true,
                        onPressed: () => unawaited(WindowChrome.close()),
                      ),
                    ],
                  ),
                ),
              ],
            ),
            if (widget.centerWidget != null)
              KeyedSubtree(
                key: _centerKey,
                child: widget.centerWidget!,
              ),
          ],
        ),
      ),
    );
  }
}

class _CaptionButton extends StatefulWidget {
  const _CaptionButton({
    required this.icon,
    required this.label,
    required this.onPressed,
    this.iconSize = 16,
    this.destructive = false,
  });

  final IconData icon;
  final String label;
  final VoidCallback onPressed;
  final double iconSize;
  final bool destructive;

  @override
  State<_CaptionButton> createState() => _CaptionButtonState();
}

class _CaptionButtonState extends State<_CaptionButton> {
  static const double _buttonWidth = 42;
  static const double _buttonHeight = 32;

  final FocusNode _focusNode = FocusNode(debugLabel: 'CaptionButton');
  bool _hovered = false;
  bool _pressed = false;
  bool _focused = false;

  @override
  void initState() {
    super.initState();
    _focusNode.addListener(_onFocusChanged);
  }

  @override
  void dispose() {
    _focusNode
      ..removeListener(_onFocusChanged)
      ..dispose();
    super.dispose();
  }

  void _onFocusChanged() {
    if (_focusNode.hasFocus == _focused) return;
    setState(() => _focused = _focusNode.hasFocus);
  }

  KeyEventResult _handleKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) return KeyEventResult.ignored;
    final LogicalKeyboardKey key = event.logicalKey;
    if (key == LogicalKeyboardKey.enter ||
        key == LogicalKeyboardKey.numpadEnter ||
        key == LogicalKeyboardKey.space) {
      widget.onPressed();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  Color get _background {
    if (widget.destructive) {
      if (_pressed) return const Color(0xFFC42B1C);
      if (_hovered) return NeuTokens.danger;
      return Colors.transparent;
    }
    if (_pressed) return const Color(0xFFDCE6F5);
    if (_hovered) return NeuTokens.insetSurface;
    return Colors.transparent;
  }

  Color get _foreground {
    if (widget.destructive && (_hovered || _pressed)) return Colors.white;
    return NeuTokens.textSecondary;
  }

  @override
  Widget build(BuildContext context) {
    final Duration duration = NeuMotion.resolveState(context);

    return Semantics(
      button: true,
      label: widget.label,
      child: Tooltip(
        message: widget.label,
        waitDuration: const Duration(milliseconds: 500),
        child: Focus(
          focusNode: _focusNode,
          onKeyEvent: _handleKey,
          child: MouseRegion(
            cursor: SystemMouseCursors.basic,
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
              onTap: widget.onPressed,
              child: AnimatedContainer(
                duration: duration,
                curve: NeuMotion.resolveStateCurve(context),
                width: _buttonWidth,
                height: _buttonHeight,
                decoration: BoxDecoration(
                  color: _background,
                  borderRadius: const BorderRadius.all(Radius.circular(8)),
                  border: Border.all(
                    color: _focused ? NeuTokens.accent : Colors.transparent,
                    width: 1.5,
                  ),
                ),
                child: Center(
                  child: Icon(
                    widget.icon,
                    size: widget.iconSize,
                    color: _foreground,
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
