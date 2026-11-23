// 参考依据:
// - Apple Human Interface Guidelines (Motion, ConcentricRectangle, Buttons 44pt/48dp)
// - W3C WCAG 2.2 AA (正文 >= 4.5:1, 非文本控件边界与状态 >= 3:1)
// - ARCHITECTURE_CONTRACTS.md 第 6 节 (改良新拟态 Token 与 120Hz 高刷合规范式)

import 'dart:math' as math;
import 'dart:ui' show FontFeature;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

/// Apple HIG 统一动效 Token：全项目禁止散落不一致的毫秒数或曲线。
abstract final class NeuMotion {
  /// 按压、聚焦与轻触物理反馈时长。
  static const Duration pressDuration = Duration(milliseconds: 220);

  /// 按压与聚焦 Apple 减速曲线。
  static const Curve pressCurve = Curves.linearToEaseOut;

  /// 分段滑块、开关拨动、进度条变化与导航选中切换时长。
  static const Duration stateDuration = Duration(milliseconds: 280);

  /// 状态切换 Apple 减速曲线。
  static const Curve stateCurve = Curves.linearToEaseOut;

  /// 系统开启“减少动态效果”时的降级短淡入时长（绝不采用 0ms 生硬跳变）。
  static const Duration reducedDuration = Duration(milliseconds: 120);

  /// 降动效模式的平滑曲线。
  static const Curve reducedCurve = Curves.easeOut;

  /// 物理按压微缩放比例。
  static const double pressedScale = 0.97;

  static bool isReduced(BuildContext context) {
    return MediaQuery.maybeDisableAnimationsOf(context) ?? false;
  }

  static Duration resolvePress(BuildContext context) {
    return isReduced(context) ? reducedDuration : pressDuration;
  }

  static Duration resolveState(BuildContext context) {
    return isReduced(context) ? reducedDuration : stateDuration;
  }

  static Curve resolvePressCurve(BuildContext context) {
    return isReduced(context) ? reducedCurve : pressCurve;
  }

  static Curve resolveStateCurve(BuildContext context) {
    return isReduced(context) ? reducedCurve : stateCurve;
  }
}

/// Apple 同心圆角 Token 与嵌套计算公式。
abstract final class NeuRadius {
  static const double card = 20.0;
  static const double segmentedTrack = 14.0;
  static const double control = 12.0;
  static const double pill = 999.0;

  /// Apple ConcentricRectangle 同心圆角公式：内圆角 = max(min, 外圆角 - 内边距)。
  static double concentric(double outer, double padding, {double min = 8.0}) {
    return math.max(min, outer - padding);
  }
}

/// 4pt / 8pt 间距栅格 Token。
abstract final class NeuSpace {
  static const double xs = 4.0;
  static const double sm = 8.0;
  static const double md = 16.0;
  static const double lg = 24.0;
  static const double xl = 32.0;
}

/// EveryoneNAS 改良新拟态（Accessible Neumorphism）设计 Token。
abstract final class NeuTokens {
  /// 主画布与浮层表面基底色。
  static const Color surface = Color(0xFFE6E9EF);

  /// 凹陷槽位底色（比主表面略深，确保与凸起组件可区分）。
  static const Color insetSurface = Color(0xFFDCE1EA);

  /// 悬停微提亮表面色（仅用于桌面指针辅助反馈，不承载唯一交互信息）。
  static const Color hoverSurface = Color(0xFFECEFF4);

  /// 135° 光源亮阴影（左上）。
  static const Color lightShadow = Color(0xFFFFFFFF);

  /// 135° 光源暗阴影（右下）。
  static const Color darkShadow = Color(0xFFC5CCD8);

  /// 细描边，用于在浅色表面提供清晰物理边界。
  static const Color hairlineBorder = Color(0xFFD1D7E2);

  /// 高对比非文本控件边界色（与 #E6E9EF 对比度约 3.98:1，满足 WCAG 2.2 AA >= 3:1）。
  static const Color controlOutline = Color(0xFF64748B);

  /// 正文主色（与 #E6E9EF 对比度 > 10:1）。
  static const Color textPrimary = Color(0xFF1E293B);

  /// 次级文字色（与 #E6E9EF 对比度 > 5.7:1）。
  static const Color textSecondary = Color(0xFF475569);

  /// 语义状态色（均满足 >= 3:1 非文本与 >= 4.5:1 文本对比度）。
  static const Color healthy = Color(0xFF0F766E);
  static const Color healthyAlt = Color(0xFF15803D);
  static const Color warning = Color(0xFFB45309);
  static const Color danger = Color(0xFFB91C1C);
  static const Color accent = Color(0xFF1D4ED8);
  static const Color accentPressed = Color(0xFF1E40AF);

  /// 最小触控热区（桌面 >= 44pt，移动端 >= 48dp）。
  static const double minTouchTarget = 44.0;
  static const double minMobileTouchTarget = 48.0;

  /// 兼容别名：指向 [NeuSpace] 4pt / 8pt 间距栅格。
  static const double spaceXs = NeuSpace.xs;
  static const double spaceSm = NeuSpace.sm;
  static const double spaceMd = NeuSpace.md;
  static const double spaceLg = NeuSpace.lg;

  static const List<BoxShadow> extrudedShadows = <BoxShadow>[
    BoxShadow(
      color: lightShadow,
      offset: Offset(-6, -6),
      blurRadius: 12,
    ),
    BoxShadow(
      color: darkShadow,
      offset: Offset(6, 6),
      blurRadius: 12,
    ),
  ];

  static const List<BoxShadow> controlShadows = <BoxShadow>[
    BoxShadow(
      color: lightShadow,
      offset: Offset(-4, -4),
      blurRadius: 9,
    ),
    BoxShadow(
      color: darkShadow,
      offset: Offset(4, 4),
      blurRadius: 9,
    ),
  ];

  static const List<BoxShadow> pressedShadows = <BoxShadow>[
    BoxShadow(
      color: lightShadow,
      offset: Offset(-2, -2),
      blurRadius: 4,
    ),
    BoxShadow(
      color: darkShadow,
      offset: Offset(2, 2),
      blurRadius: 4,
    ),
  ];

  static double resolveMinTouchTarget(BuildContext context) {
    final platform = Theme.of(context).platform;
    final isMobile =
        platform == TargetPlatform.android || platform == TargetPlatform.iOS;
    return isMobile ? minMobileTouchTarget : minTouchTarget;
  }
}

@immutable
class NeuTheme extends ThemeExtension<NeuTheme> {
  const NeuTheme({
    this.surfaceColor = NeuTokens.surface,
    this.insetColor = NeuTokens.insetSurface,
    this.extrudedShadows = NeuTokens.extrudedShadows,
    this.pressedShadows = NeuTokens.pressedShadows,
    this.borderRadius = NeuRadius.card,
  });

  final Color surfaceColor;
  final Color insetColor;
  final List<BoxShadow> extrudedShadows;
  final List<BoxShadow> pressedShadows;
  final double borderRadius;

  static NeuTheme of(BuildContext context) {
    final ext = Theme.of(context).extension<NeuTheme>();
    assert(ext != null, 'NeuTheme extension must be registered on ThemeData.');
    return ext!;
  }

  @override
  NeuTheme copyWith({
    Color? surfaceColor,
    Color? insetColor,
    List<BoxShadow>? extrudedShadows,
    List<BoxShadow>? pressedShadows,
    double? borderRadius,
  }) {
    return NeuTheme(
      surfaceColor: surfaceColor ?? this.surfaceColor,
      insetColor: insetColor ?? this.insetColor,
      extrudedShadows: extrudedShadows ?? this.extrudedShadows,
      pressedShadows: pressedShadows ?? this.pressedShadows,
      borderRadius: borderRadius ?? this.borderRadius,
    );
  }

  @override
  NeuTheme lerp(ThemeExtension<NeuTheme>? other, double t) {
    if (other is! NeuTheme) return this;
    return NeuTheme(
      surfaceColor: Color.lerp(surfaceColor, other.surfaceColor, t)!,
      insetColor: Color.lerp(insetColor, other.insetColor, t)!,
      extrudedShadows:
          BoxShadow.lerpList(extrudedShadows, other.extrudedShadows, t)!,
      pressedShadows:
          BoxShadow.lerpList(pressedShadows, other.pressedShadows, t)!,
      borderRadius: borderRadius + (other.borderRadius - borderRadius) * t,
    );
  }

  @override
  bool operator ==(Object other) {
    if (identical(this, other)) return true;
    return other is NeuTheme &&
        other.surfaceColor == surfaceColor &&
        other.insetColor == insetColor &&
        listEquals(other.extrudedShadows, extrudedShadows) &&
        listEquals(other.pressedShadows, pressedShadows) &&
        other.borderRadius == borderRadius;
  }

  @override
  int get hashCode => Object.hash(
        surfaceColor,
        insetColor,
        Object.hashAll(extrudedShadows),
        Object.hashAll(pressedShadows),
        borderRadius,
      );
}

ThemeData buildLanVaultTheme() {
  const colorScheme = ColorScheme.light(
    surface: NeuTokens.surface,
    primary: NeuTokens.accent,
    onPrimary: Colors.white,
    secondary: NeuTokens.healthy,
    onSecondary: Colors.white,
    error: NeuTokens.danger,
    onError: Colors.white,
    onSurface: NeuTokens.textPrimary,
    onSurfaceVariant: NeuTokens.textSecondary,
    outline: NeuTokens.hairlineBorder,
  );

  return ThemeData(
    useMaterial3: true,
    colorScheme: colorScheme,
    scaffoldBackgroundColor: NeuTokens.surface,
    canvasColor: NeuTokens.surface,
    dividerColor: NeuTokens.hairlineBorder,
    textTheme: const TextTheme(
      headlineSmall: TextStyle(
        color: NeuTokens.textPrimary,
        fontSize: 22,
        fontWeight: FontWeight.w700,
        letterSpacing: -0.3,
      ),
      titleMedium: TextStyle(
        color: NeuTokens.textPrimary,
        fontSize: 16,
        fontWeight: FontWeight.w600,
      ),
      titleSmall: TextStyle(
        color: NeuTokens.textPrimary,
        fontSize: 14,
        fontWeight: FontWeight.w600,
      ),
      bodyMedium: TextStyle(
        color: NeuTokens.textPrimary,
        fontSize: 14,
        height: 1.45,
      ),
      bodySmall: TextStyle(
        color: NeuTokens.textSecondary,
        fontSize: 13,
        height: 1.4,
      ),
      labelLarge: TextStyle(
        color: NeuTokens.textPrimary,
        fontSize: 14,
        fontWeight: FontWeight.w600,
      ),
      labelMedium: TextStyle(
        color: NeuTokens.textSecondary,
        fontSize: 12,
        fontWeight: FontWeight.w600,
      ),
    ),
    extensions: const <ThemeExtension<dynamic>>[
      NeuTheme(),
    ],
  );
}

/// 120Hz 高刷优化新拟态卡片：支持凸起/凹陷与 [RepaintBoundary] 隔离光栅化，
/// 仅使用 [BoxDecoration] 圆角，绝不使用 ClipPath / BackdropFilter。
class NeuCard extends StatelessWidget {
  const NeuCard({
    super.key,
    required this.child,
    this.padding = const EdgeInsets.all(NeuSpace.md),
    this.radius,
    this.inset = false,
    this.semanticLabel,
  });

  final Widget child;
  final EdgeInsetsGeometry padding;
  final double? radius;
  final bool inset;
  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final resolvedRadius = radius ?? neu.borderRadius;
    final box = RepaintBoundary(
      child: Container(
        padding: padding,
        decoration: BoxDecoration(
          color: inset ? neu.insetColor : neu.surfaceColor,
          borderRadius: BorderRadius.all(Radius.circular(resolvedRadius)),
          border: Border.all(
            color: inset
                ? NeuTokens.darkShadow.withValues(alpha: 0.85)
                : NeuTokens.lightShadow.withValues(alpha: 0.6),
            width: 1,
          ),
          boxShadow: inset ? null : neu.extrudedShadows,
        ),
        child: child,
      ),
    );
    if (semanticLabel == null) return box;
    return Semantics(container: true, label: semanticLabel, child: box);
  }
}

enum NeuButtonTone { neutral, primary, danger, healthy }

/// 第一性原理新拟态按钮：
/// 完整状态机（rest / hover / pressed / focused / disabled / loading）、
/// 键盘 Enter/Space 激活、WCAG >= 3:1 焦点环、Transform.scale(0.97) + 220ms 物理按压、
/// >= 44x44px（移动端 >= 48px）触控热区与 Semantics(button: true)。
class NeuButton extends StatefulWidget {
  const NeuButton({
    super.key,
    required this.label,
    required this.onPressed,
    this.icon,
    this.tone = NeuButtonTone.neutral,
    this.loading = false,
    this.expanded = false,
    this.radius = NeuRadius.control,
    this.semanticLabel,
  });

  final String label;
  final VoidCallback? onPressed;
  final IconData? icon;
  final NeuButtonTone tone;
  final bool loading;
  final bool expanded;
  final double radius;
  final String? semanticLabel;

  @override
  State<NeuButton> createState() => _NeuButtonState();
}

class _NeuButtonState extends State<NeuButton> {
  final FocusNode _focusNode = FocusNode(debugLabel: 'NeuButton');
  bool _hovered = false;
  bool _pressed = false;
  bool _focused = false;

  bool get _enabled => widget.onPressed != null && !widget.loading;

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

  void _setPressed(bool value) {
    if (_pressed != value) {
      setState(() => _pressed = value);
    }
  }

  void _setHovered(bool value) {
    if (_hovered != value) {
      setState(() => _hovered = value);
    }
  }

  KeyEventResult _handleKey(FocusNode node, KeyEvent event) {
    if (!_enabled || event is! KeyDownEvent) return KeyEventResult.ignored;
    if (event.logicalKey == LogicalKeyboardKey.enter ||
        event.logicalKey == LogicalKeyboardKey.numpadEnter ||
        event.logicalKey == LogicalKeyboardKey.space) {
      widget.onPressed!();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  Color _resolveForeground() {
    if (!_enabled) return NeuTokens.textSecondary;
    return switch (widget.tone) {
      NeuButtonTone.neutral => NeuTokens.textPrimary,
      NeuButtonTone.primary => Colors.white,
      NeuButtonTone.danger => Colors.white,
      NeuButtonTone.healthy => Colors.white,
    };
  }

  Color _resolveBackground(NeuTheme neu) {
    if (!_enabled) return neu.insetColor;
    return switch (widget.tone) {
      NeuButtonTone.neutral =>
        _hovered && !_pressed ? NeuTokens.hoverSurface : neu.surfaceColor,
      NeuButtonTone.primary =>
        _pressed ? NeuTokens.accentPressed : NeuTokens.accent,
      NeuButtonTone.danger => NeuTokens.danger,
      NeuButtonTone.healthy => NeuTokens.healthy,
    };
  }

  List<BoxShadow>? _resolveShadows(NeuTheme neu) {
    final base = !_enabled
        ? const <BoxShadow>[]
        : (_pressed ? neu.pressedShadows : NeuTokens.controlShadows);
    if (!_focused) {
      return base.isEmpty ? null : base;
    }
    final ringColor = widget.tone == NeuButtonTone.primary
        ? NeuTokens.textPrimary
        : NeuTokens.accent;
    return <BoxShadow>[
      ...base,
      BoxShadow(
        color: ringColor,
        offset: Offset.zero,
        blurRadius: 0,
        spreadRadius: 2.5,
      ),
    ];
  }

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final reduced = NeuMotion.isReduced(context);
    final duration = NeuMotion.resolvePress(context);
    final curve = NeuMotion.resolvePressCurve(context);
    final minTouch = NeuTokens.resolveMinTouchTarget(context);
    final fg = _resolveForeground();
    final bg = _resolveBackground(neu);
    final targetScale =
        (_pressed && _enabled && !reduced) ? NeuMotion.pressedScale : 1.0;

    final content = Row(
      mainAxisSize: widget.expanded ? MainAxisSize.max : MainAxisSize.min,
      mainAxisAlignment: MainAxisAlignment.center,
      children: <Widget>[
        if (widget.loading) ...<Widget>[
          SizedBox(
            width: 16,
            height: 16,
            child: CircularProgressIndicator(
              strokeWidth: 2,
              valueColor: AlwaysStoppedAnimation<Color>(fg),
            ),
          ),
          const SizedBox(width: NeuSpace.sm),
        ] else if (widget.icon != null) ...<Widget>[
          Icon(widget.icon, size: 18, color: fg),
          const SizedBox(width: NeuSpace.sm),
        ],
        Flexible(
          child: Text(
            widget.label,
            overflow: TextOverflow.ellipsis,
            style: Theme.of(context).textTheme.labelLarge?.copyWith(color: fg),
          ),
        ),
      ],
    );

    final decoratedButton = AnimatedContainer(
      duration: duration,
      curve: curve,
      constraints: BoxConstraints(
        minWidth: minTouch,
        minHeight: minTouch,
      ),
      padding: const EdgeInsets.symmetric(
        horizontal: NeuSpace.md,
        vertical: 12,
      ),
      decoration: BoxDecoration(
        color: bg,
        borderRadius: BorderRadius.all(Radius.circular(widget.radius)),
        border: Border.all(
          color: _focused
              ? (widget.tone == NeuButtonTone.primary
                  ? Colors.white
                  : NeuTokens.accent)
              : (widget.tone == NeuButtonTone.neutral
                  ? (_enabled
                      ? NeuTokens.controlOutline.withValues(alpha: 0.35)
                      : NeuTokens.hairlineBorder)
                  : bg),
          width: _focused ? 1.5 : 1.0,
        ),
        boxShadow: _resolveShadows(neu),
      ),
      child: content,
    );

    return RepaintBoundary(
      child: Semantics(
        button: true,
        enabled: _enabled,
        label: widget.semanticLabel ?? widget.label,
        child: Focus(
          focusNode: _focusNode,
          canRequestFocus: _enabled,
          onKeyEvent: _handleKey,
          child: MouseRegion(
            cursor: _enabled
                ? SystemMouseCursors.click
                : SystemMouseCursors.forbidden,
            onEnter: (_) => _setHovered(true),
            onExit: (_) {
              _setHovered(false);
              _setPressed(false);
            },
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTapDown: _enabled
                  ? (_) {
                      _focusNode.requestFocus();
                      _setPressed(true);
                    }
                  : null,
              onTapUp: _enabled ? (_) => _setPressed(false) : null,
              onTapCancel: _enabled ? () => _setPressed(false) : null,
              onTap: _enabled ? widget.onPressed : null,
              child: TweenAnimationBuilder<double>(
                tween: Tween<double>(end: targetScale),
                duration: duration,
                curve: curve,
                child: decoratedButton,
                builder: (context, scale, child) {
                  return Transform.scale(
                    scale: scale,
                    alignment: Alignment.center,
                    child: child,
                  );
                },
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// 第一性原理新拟态开关（替代原生 Material SwitchListTile）：
/// 凹陷轨道 + 凸起圆滑块、280ms Apple 曲线平滑位移、开启态 >= 3:1 对比度、
/// 键盘/焦点环、Semantics(toggled: value) 与整行 >= 48px 触控区。
class NeuSwitch extends StatefulWidget {
  const NeuSwitch({
    super.key,
    required this.value,
    required this.onChanged,
    required this.title,
    this.subtitle,
    this.semanticLabel,
    this.padding = const EdgeInsets.symmetric(vertical: NeuSpace.xs),
  });

  final bool value;
  final ValueChanged<bool>? onChanged;
  final String title;
  final String? subtitle;
  final String? semanticLabel;
  final EdgeInsetsGeometry padding;

  @override
  State<NeuSwitch> createState() => _NeuSwitchState();
}

class _NeuSwitchState extends State<NeuSwitch> {
  final FocusNode _focusNode = FocusNode(debugLabel: 'NeuSwitch');
  bool _focused = false;
  bool _pressed = false;

  bool get _enabled => widget.onChanged != null;

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

  void _toggle() {
    if (_enabled) {
      widget.onChanged!(!widget.value);
    }
  }

  KeyEventResult _handleKey(FocusNode node, KeyEvent event) {
    if (!_enabled || event is! KeyDownEvent) return KeyEventResult.ignored;
    if (event.logicalKey == LogicalKeyboardKey.enter ||
        event.logicalKey == LogicalKeyboardKey.numpadEnter ||
        event.logicalKey == LogicalKeyboardKey.space) {
      _toggle();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final textTheme = Theme.of(context).textTheme;
    final stateDuration = NeuMotion.resolveState(context);
    final stateCurve = NeuMotion.resolveStateCurve(context);
    final pressDuration = NeuMotion.resolvePress(context);
    final pressCurve = NeuMotion.resolvePressCurve(context);
    final reduced = NeuMotion.isReduced(context);

    final trackColor = !_enabled
        ? neu.insetColor
        : (widget.value ? NeuTokens.accent : neu.insetColor);
    final trackBorderColor = widget.value
        ? NeuTokens.accent
        : NeuTokens.controlOutline;

    final thumbScale =
        (_pressed && _enabled && !reduced) ? NeuMotion.pressedScale : 1.0;

    return RepaintBoundary(
      child: Semantics(
        toggled: widget.value,
        enabled: _enabled,
        label: widget.semanticLabel ?? widget.title,
        hint: widget.subtitle,
        child: Focus(
          focusNode: _focusNode,
          canRequestFocus: _enabled,
          onKeyEvent: _handleKey,
          child: MouseRegion(
            cursor: _enabled
                ? SystemMouseCursors.click
                : SystemMouseCursors.forbidden,
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTapDown: _enabled
                  ? (_) {
                      _focusNode.requestFocus();
                      setState(() => _pressed = true);
                    }
                  : null,
              onTapUp: _enabled ? (_) => setState(() => _pressed = false) : null,
              onTapCancel:
                  _enabled ? () => setState(() => _pressed = false) : null,
              onTap: _enabled ? _toggle : null,
              child: ConstrainedBox(
                constraints: const BoxConstraints(
                  minHeight: NeuTokens.minMobileTouchTarget,
                ),
                child: Padding(
                  padding: widget.padding,
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.center,
                    children: <Widget>[
                      Expanded(
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: <Widget>[
                            Text(
                              widget.title,
                              style: textTheme.titleSmall?.copyWith(
                                color: _enabled
                                    ? NeuTokens.textPrimary
                                    : NeuTokens.textSecondary,
                              ),
                            ),
                            if (widget.subtitle != null) ...<Widget>[
                              const SizedBox(height: 2),
                              Text(
                                widget.subtitle!,
                                style: textTheme.bodySmall,
                              ),
                            ],
                          ],
                        ),
                      ),
                      const SizedBox(width: NeuSpace.md),
                      AnimatedContainer(
                        duration: stateDuration,
                        curve: stateCurve,
                        width: 52,
                        height: 30,
                        padding: const EdgeInsets.all(3),
                        decoration: BoxDecoration(
                          color: trackColor,
                          borderRadius: const BorderRadius.all(
                            Radius.circular(NeuRadius.pill),
                          ),
                          border: Border.all(
                            color: _focused
                                ? (widget.value
                                    ? NeuTokens.textPrimary
                                    : NeuTokens.accent)
                                : trackBorderColor,
                            width: _focused ? 2.0 : 1.5,
                          ),
                          boxShadow: _focused
                              ? <BoxShadow>[
                                  BoxShadow(
                                    color: widget.value
                                        ? NeuTokens.textPrimary
                                        : NeuTokens.accent,
                                    offset: Offset.zero,
                                    blurRadius: 0,
                                    spreadRadius: 2.0,
                                  ),
                                ]
                              : null,
                        ),
                        child: AnimatedAlign(
                          alignment: widget.value
                              ? Alignment.centerRight
                              : Alignment.centerLeft,
                          duration: stateDuration,
                          curve: stateCurve,
                          child: TweenAnimationBuilder<double>(
                            tween: Tween<double>(end: thumbScale),
                            duration: pressDuration,
                            curve: pressCurve,
                            builder: (context, scale, child) {
                              return Transform.scale(
                                scale: scale,
                                child: child,
                              );
                            },
                            child: Container(
                              width: 22,
                              height: 22,
                              decoration: BoxDecoration(
                                color: Colors.white,
                                shape: BoxShape.circle,
                                border: Border.all(
                                  color: widget.value
                                      ? Colors.white
                                      : NeuTokens.hairlineBorder,
                                  width: 1,
                                ),
                                boxShadow: const <BoxShadow>[
                                  BoxShadow(
                                    color: Color(0x331E293B),
                                    offset: Offset(0, 2),
                                    blurRadius: 4,
                                  ),
                                ],
                              ),
                            ),
                          ),
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

/// 分段控件选项定义。
@immutable
class NeuSegment<T> {
  const NeuSegment({
    required this.value,
    required this.label,
    this.semanticLabel,
  });

  final T value;
  final String label;
  final String? semanticLabel;
}

/// 第一性原理新拟态分段切换器：
/// 同心圆角（外 14.0、内边距 4.0、滑块 concentric(14, 4) = 10.0）、
/// 280ms Apple 统一曲线、键盘左右方向键导航、WCAG >= 3:1 焦点环与 Semantics(selected)。
class NeuSegmentedControl<T> extends StatefulWidget {
  const NeuSegmentedControl({
    super.key,
    required this.segments,
    required this.value,
    required this.onChanged,
    this.enabled = true,
    this.semanticLabel,
  }) : assert(segments.length >= 2, 'NeuSegmentedControl requires at least 2 segments.');

  final List<NeuSegment<T>> segments;
  final T value;
  final ValueChanged<T> onChanged;
  final bool enabled;
  final String? semanticLabel;

  @override
  State<NeuSegmentedControl<T>> createState() => _NeuSegmentedControlState<T>();
}

class _NeuSegmentedControlState<T> extends State<NeuSegmentedControl<T>> {
  final FocusNode _focusNode = FocusNode(debugLabel: 'NeuSegmentedControl');
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

  int get _selectedIndex {
    final idx = widget.segments.indexWhere((s) => s.value == widget.value);
    return idx >= 0 ? idx : 0;
  }

  Alignment _thumbAlignment() {
    final count = widget.segments.length;
    if (count <= 1) return Alignment.center;
    final alignX = -1.0 + (2.0 * _selectedIndex / (count - 1));
    return Alignment(alignX, 0.0);
  }

  KeyEventResult _handleKey(FocusNode node, KeyEvent event) {
    if (!widget.enabled || event is! KeyDownEvent) {
      return KeyEventResult.ignored;
    }
    final count = widget.segments.length;
    final current = _selectedIndex;
    if (event.logicalKey == LogicalKeyboardKey.arrowRight ||
        event.logicalKey == LogicalKeyboardKey.arrowDown) {
      final next = (current + 1) % count;
      widget.onChanged(widget.segments[next].value);
      return KeyEventResult.handled;
    }
    if (event.logicalKey == LogicalKeyboardKey.arrowLeft ||
        event.logicalKey == LogicalKeyboardKey.arrowUp) {
      final prev = (current - 1 + count) % count;
      widget.onChanged(widget.segments[prev].value);
      return KeyEventResult.handled;
    }
    if (event.logicalKey == LogicalKeyboardKey.home) {
      widget.onChanged(widget.segments.first.value);
      return KeyEventResult.handled;
    }
    if (event.logicalKey == LogicalKeyboardKey.end) {
      widget.onChanged(widget.segments.last.value);
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final duration = NeuMotion.resolveState(context);
    final curve = NeuMotion.resolveStateCurve(context);
    final minTouch = NeuTokens.resolveMinTouchTarget(context);

    const double outerRadius = NeuRadius.segmentedTrack;
    const double trackPadding = NeuSpace.xs;
    final double thumbRadius = NeuRadius.concentric(outerRadius, trackPadding);

    return RepaintBoundary(
      child: Semantics(
        container: true,
        label: widget.semanticLabel,
        child: Focus(
          focusNode: _focusNode,
          canRequestFocus: widget.enabled,
          onKeyEvent: _handleKey,
          child: AnimatedContainer(
            duration: NeuMotion.resolvePress(context),
            curve: NeuMotion.resolvePressCurve(context),
            padding: const EdgeInsets.all(trackPadding),
            decoration: BoxDecoration(
              color: neu.insetColor,
              borderRadius: const BorderRadius.all(
                Radius.circular(outerRadius),
              ),
              border: Border.all(
                color: _focused
                    ? NeuTokens.accent
                    : NeuTokens.controlOutline.withValues(alpha: 0.45),
                width: _focused ? 2.0 : 1.0,
              ),
              boxShadow: _focused
                  ? const <BoxShadow>[
                      BoxShadow(
                        color: NeuTokens.accent,
                        offset: Offset.zero,
                        blurRadius: 0,
                        spreadRadius: 1.5,
                      ),
                    ]
                  : null,
            ),
            child: SizedBox(
              height: minTouch,
              child: Stack(
                children: <Widget>[
                  Positioned.fill(
                    child: AnimatedAlign(
                      alignment: _thumbAlignment(),
                      duration: duration,
                      curve: curve,
                      child: FractionallySizedBox(
                        widthFactor: 1 / widget.segments.length,
                        heightFactor: 1.0,
                        child: DecoratedBox(
                          decoration: BoxDecoration(
                            color: neu.surfaceColor,
                            borderRadius: BorderRadius.all(
                              Radius.circular(thumbRadius),
                            ),
                            border: Border.all(
                              color: NeuTokens.lightShadow.withValues(alpha: 0.85),
                              width: 1,
                            ),
                            boxShadow: NeuTokens.controlShadows,
                          ),
                        ),
                      ),
                    ),
                  ),
                  Row(
                    children: <Widget>[
                      for (final segment in widget.segments)
                        Expanded(
                          child: Semantics(
                            button: true,
                            selected: segment.value == widget.value,
                            enabled: widget.enabled,
                            label: segment.semanticLabel ?? segment.label,
                            child: MouseRegion(
                              cursor: widget.enabled
                                  ? SystemMouseCursors.click
                                  : SystemMouseCursors.forbidden,
                              child: GestureDetector(
                                behavior: HitTestBehavior.opaque,
                                onTap: widget.enabled
                                    ? () {
                                        _focusNode.requestFocus();
                                        widget.onChanged(segment.value);
                                      }
                                    : null,
                                child: ConstrainedBox(
                                  constraints: BoxConstraints(
                                    minWidth: minTouch,
                                    minHeight: minTouch,
                                  ),
                                  child: Center(
                                    child: Text(
                                      segment.label,
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: Theme.of(context)
                                          .textTheme
                                          .labelLarge
                                          ?.copyWith(
                                            color: segment.value == widget.value
                                                ? NeuTokens.accent
                                                : NeuTokens.textSecondary,
                                          ),
                                    ),
                                  ),
                                ),
                              ),
                            ),
                          ),
                        ),
                    ],
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// 凹陷输入框（诺曼物理隐喻：凹陷 = 可输入槽位）：
/// >= 16px 字号防移动端缩放、220ms Apple 曲线焦点环（>= 3:1）、纯 BoxDecoration。
class NeuInsetField extends StatefulWidget {
  const NeuInsetField({
    super.key,
    required this.controller,
    this.label,
    this.hint,
    this.errorText,
    this.obscureText = false,
    this.maxLines = 1,
    this.keyboardType,
    this.prefixIcon,
    this.suffixWidget,
    this.enabled = true,
    this.onSubmitted,
  });

  final TextEditingController controller;
  final String? label;
  final String? hint;
  final String? errorText;
  final bool obscureText;
  final int maxLines;
  final TextInputType? keyboardType;
  final IconData? prefixIcon;
  final Widget? suffixWidget;
  final bool enabled;
  final ValueChanged<String>? onSubmitted;

  @override
  State<NeuInsetField> createState() => _NeuInsetFieldState();
}

class _NeuInsetFieldState extends State<NeuInsetField> {
  final FocusNode _focusNode = FocusNode(debugLabel: 'NeuInsetField');
  bool _focused = false;

  @override
  void initState() {
    super.initState();
    _focusNode.addListener(_onFocusChanged);
  }

  @override
  void dispose() {
    _focusNode.removeListener(_onFocusChanged);
    _focusNode.dispose();
    super.dispose();
  }

  void _onFocusChanged() {
    if (_focusNode.hasFocus != _focused) {
      setState(() => _focused = _focusNode.hasFocus);
    }
  }

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final duration = NeuMotion.resolvePress(context);
    final curve = NeuMotion.resolvePressCurve(context);
    final minTouch = NeuTokens.resolveMinTouchTarget(context);
    final hasError = widget.errorText != null;
    final borderColor = hasError
        ? NeuTokens.danger
        : (_focused
            ? NeuTokens.accent
            : NeuTokens.controlOutline.withValues(alpha: 0.55));

    return RepaintBoundary(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (widget.label != null) ...<Widget>[
            Text(
              widget.label!,
              style: Theme.of(context).textTheme.labelMedium,
            ),
            const SizedBox(height: NeuSpace.sm),
          ],
          AnimatedContainer(
            duration: duration,
            curve: curve,
            constraints: BoxConstraints(minHeight: minTouch),
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 2),
            decoration: BoxDecoration(
              color: neu.insetColor,
              borderRadius: const BorderRadius.all(
                Radius.circular(NeuRadius.control),
              ),
              border: Border.all(
                color: borderColor,
                width: _focused || hasError ? 2.0 : 1.0,
              ),
              boxShadow: <BoxShadow>[
                const BoxShadow(
                  color: NeuTokens.lightShadow,
                  offset: Offset(1, 1),
                  blurRadius: 0,
                ),
                if (_focused)
                  BoxShadow(
                    color: (hasError ? NeuTokens.danger : NeuTokens.accent)
                        .withValues(alpha: 0.22),
                    offset: Offset.zero,
                    blurRadius: 0,
                    spreadRadius: 2.0,
                  ),
              ],
            ),
            child: Row(
              crossAxisAlignment: widget.maxLines == 1
                  ? CrossAxisAlignment.center
                  : CrossAxisAlignment.start,
              children: <Widget>[
                if (widget.prefixIcon != null) ...<Widget>[
                  Padding(
                    padding: EdgeInsets.only(top: widget.maxLines == 1 ? 0 : 12),
                    child: Icon(
                      widget.prefixIcon,
                      size: 18,
                      color: NeuTokens.textSecondary,
                    ),
                  ),
                  const SizedBox(width: NeuSpace.sm),
                ],
                Expanded(
                  child: TextField(
                    controller: widget.controller,
                    focusNode: _focusNode,
                    enabled: widget.enabled,
                    obscureText: widget.obscureText,
                    maxLines: widget.maxLines,
                    keyboardType: widget.keyboardType,
                    onSubmitted: widget.onSubmitted,
                    cursorColor: NeuTokens.accent,
                    style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                          fontSize: 16,
                          height: 1.4,
                          color: widget.enabled
                              ? NeuTokens.textPrimary
                              : NeuTokens.textSecondary,
                        ),
                    decoration: InputDecoration(
                      isDense: true,
                      border: InputBorder.none,
                      hintText: widget.hint,
                      hintStyle: Theme.of(context).textTheme.bodySmall?.copyWith(
                            fontSize: 16,
                            color: NeuTokens.textSecondary,
                          ),
                      contentPadding: const EdgeInsets.symmetric(vertical: 12),
                    ),
                  ),
                ),
                if (widget.suffixWidget != null) ...<Widget>[
                  const SizedBox(width: NeuSpace.sm),
                  widget.suffixWidget!,
                ],
              ],
            ),
          ),
          if (hasError) ...<Widget>[
            const SizedBox(height: NeuSpace.xs),
            Text(
              widget.errorText!,
              style: Theme.of(context)
                  .textTheme
                  .bodySmall
                  ?.copyWith(color: NeuTokens.danger),
            ),
          ],
        ],
      ),
    );
  }
}

/// 凹陷槽容量/进度条：280ms Apple 曲线平滑过渡 + 等宽数字百分比 + Semantics 播报，
/// 无 ClipPath / BackdropFilter。
class NeuProgressBar extends StatelessWidget {
  const NeuProgressBar({
    super.key,
    required this.value,
    this.height = 12,
    this.fillColor = NeuTokens.accent,
    this.showPercentage = true,
    this.semanticLabel,
  }) : assert(value >= 0.0 && value <= 1.0, 'value must be in [0.0, 1.0].');

  final double value;
  final double height;
  final Color fillColor;
  final bool showPercentage;
  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final duration = NeuMotion.resolveState(context);
    final curve = NeuMotion.resolveStateCurve(context);
    final radius = const BorderRadius.all(Radius.circular(NeuRadius.pill));
    final percent = (value * 100).round();

    final track = Container(
      height: height,
      decoration: BoxDecoration(
        color: neu.insetColor,
        borderRadius: radius,
        border: Border.all(
          color: NeuTokens.controlOutline.withValues(alpha: 0.45),
          width: 1,
        ),
      ),
      child: Align(
        alignment: Alignment.centerLeft,
        child: AnimatedFractionallySizedBox(
          duration: duration,
          curve: curve,
          widthFactor: value,
          heightFactor: 1.0,
          child: DecoratedBox(
            decoration: BoxDecoration(
              color: fillColor,
              borderRadius: radius,
            ),
          ),
        ),
      ),
    );

    return RepaintBoundary(
      child: Semantics(
        label: semanticLabel,
        value: '$percent%',
        child: showPercentage
            ? Row(
                children: <Widget>[
                  Expanded(child: track),
                  const SizedBox(width: NeuSpace.sm),
                  SizedBox(
                    width: 40,
                    child: Text(
                      '$percent%',
                      textAlign: TextAlign.right,
                      style: Theme.of(context).textTheme.labelMedium?.copyWith(
                            color: NeuTokens.textPrimary,
                            fontFeatures: const <FontFeature>[
                              FontFeature.tabularFigures(),
                            ],
                          ),
                    ),
                  ),
                ],
              )
            : track,
      ),
    );
  }
}
