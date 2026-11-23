// EveryoneNAS 全新柔冰蓝 / 纯白轻拟态玻璃卡片设计系统
// 完整保留 NeuCard / NeuButton / NeuSwitch / NeuInsetField / NeuSegmentedControl /
// NeuDropdown / NeuProgressBar 公开 API，并新增 NeuStatusBadge / StorageDonutRing /
// BrandLogoMark / BrandLogoFull。

import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

/// 统一动效 Token。
abstract final class NeuMotion {
  static const Duration pressDuration = Duration(milliseconds: 220);
  static const Curve pressCurve = Curves.linearToEaseOut;

  static const Duration stateDuration = Duration(milliseconds: 280);
  static const Curve stateCurve = Curves.linearToEaseOut;

  static const Duration reducedDuration = Duration(milliseconds: 120);
  static const Curve reducedCurve = Curves.easeOut;

  static const double pressedScale = 0.968;
  static const double cardPressedScale = 0.985;

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

/// 同心圆角 Token 与嵌套计算公式。
abstract final class NeuRadius {
  static const double card = 18.0;
  static const double segmentedTrack = 12.0;
  static const double control = 12.0;
  static const double pill = 999.0;

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

/// EveryoneNAS 柔冰蓝 / 纯白轻拟态玻璃卡片设计 Token。
abstract final class NeuTokens {
  /// 全局页面画布底色：清透冰蓝白 `#F3F6FC`。
  static const Color canvas = Color(0xFFF3F6FC);

  /// 卡片与浮层表面：纯白 `#FFFFFF`。
  static const Color surface = Color(0xFFFFFFFF);

  /// 凹陷槽位底色：`#EEF3FA`。
  static const Color insetSurface = Color(0xFFEEF3FA);

  /// 悬停微冷蓝表面。
  static const Color hoverSurface = Color(0xFFF6F9FF);

  /// 亮阴影与冷蓝阴影。
  static const Color lightShadow = Color(0xFFFFFFFF);
  static const Color darkShadow = Color(0x142563EB);

  /// 1px 细腻冷蓝边框：`#E6ECF5`。
  static const Color hairlineBorder = Color(0xFFE6ECF5);

  /// 控件清晰轮廓色。
  static const Color controlOutline = Color(0xFFCBD5E1);

  /// 文字层级：主文字 `#0F172A`，次级文字 `#64748B`。
  static const Color textPrimary = Color(0xFF0F172A);
  static const Color textSecondary = Color(0xFF64748B);

  /// 明快科技蓝主强调色渐变 `#2563EB` -> `#3B82F6`。
  static const Color accent = Color(0xFF2563EB);
  static const Color accentLight = Color(0xFF3B82F6);
  static const Color accentPressed = Color(0xFF1D4ED8);
  static const Color accentSoft = Color(0xFFEFF6FF);

  /// 健康绿 `#10B981` + 浅绿底 `#ECFDF5`。
  static const Color healthy = Color(0xFF10B981);
  static const Color healthyAlt = Color(0xFF059669);
  static const Color healthySoft = Color(0xFFECFDF5);

  /// 文件夹暖金 `#F59E0B` + 浅金底 `#FEF3C7`。
  static const Color folderGold = Color(0xFFF59E0B);
  static const Color folderGoldSoft = Color(0xFFFEF3C7);

  /// 警告与危险语义色。
  static const Color warning = Color(0xFFD97706);
  static const Color danger = Color(0xFFEF4444);
  static const Color dangerSoft = Color(0xFFFEF2F2);

  /// 图表与时间线辅助语义色（全端统一收敛）。
  static const Color chartSky = Color(0xFF38BDF8);
  static const Color chartIndigo = Color(0xFF6366F1);
  static const Color chartIndigoSoft = Color(0xFFEEF2FF);
  static const Color slateMuted = Color(0xFF475569);
  static const Color timelineInactive = Color(0xFFCBD5E1);
  static const Color timelineHalo = Color(0xFFBFDBFE);
  static const Color timelineLine = Color(0xFFD6E2F5);

  /// 标准蓝渐变（导航激活胶囊、主按钮）。
  static const LinearGradient primaryGradient = LinearGradient(
    begin: Alignment.centerLeft,
    end: Alignment.centerRight,
    colors: <Color>[accent, accentLight],
  );

  static const double minTouchTarget = 42.0;
  static const double minMobileTouchTarget = 46.0;

  static const double spaceXs = NeuSpace.xs;
  static const double spaceSm = NeuSpace.sm;
  static const double spaceMd = NeuSpace.md;
  static const double spaceLg = NeuSpace.lg;

  /// 纯白轻拟态卡片柔和冷蓝阴影。
  static const List<BoxShadow> extrudedShadows = <BoxShadow>[
    BoxShadow(
      color: Color(0x0D1E3A8A),
      offset: Offset(0, 6),
      blurRadius: 20,
      spreadRadius: -2,
    ),
    BoxShadow(
      color: Color(0x080F172A),
      offset: Offset(0, 2),
      blurRadius: 6,
    ),
  ];

  static const List<BoxShadow> controlShadows = <BoxShadow>[
    BoxShadow(
      color: Color(0x101E3A8A),
      offset: Offset(0, 3),
      blurRadius: 10,
      spreadRadius: -1,
    ),
  ];

  static const List<BoxShadow> primaryButtonShadows = <BoxShadow>[
    BoxShadow(
      color: Color(0x332563EB),
      offset: Offset(0, 6),
      blurRadius: 14,
      spreadRadius: -2,
    ),
  ];

  static const List<BoxShadow> pressedShadows = <BoxShadow>[
    BoxShadow(
      color: Color(0x0A1E3A8A),
      offset: Offset(0, 1),
      blurRadius: 3,
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
    scaffoldBackgroundColor: NeuTokens.canvas,
    canvasColor: NeuTokens.canvas,
    dividerColor: NeuTokens.hairlineBorder,
    textTheme: const TextTheme(
      headlineSmall: TextStyle(
        color: NeuTokens.textPrimary,
        fontSize: 24,
        fontWeight: FontWeight.w700,
        letterSpacing: -0.4,
      ),
      titleMedium: TextStyle(
        color: NeuTokens.textPrimary,
        fontSize: 16,
        fontWeight: FontWeight.w600,
        letterSpacing: -0.2,
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

/// 品牌图标（加载 `assets/brand/logo_mark.png`）。
class BrandLogoMark extends StatelessWidget {
  const BrandLogoMark({
    super.key,
    this.size = 28,
  });

  final double size;

  @override
  Widget build(BuildContext context) {
    return Image.asset(
      'assets/brand/logo_mark.png',
      width: size,
      height: size,
      fit: BoxFit.contain,
      filterQuality: FilterQuality.medium,
      semanticLabel: 'EveryoneNAS Logo',
    );
  }
}

/// 完整品牌横幅 Logo（加载 `assets/brand/logo_full.png`）。
class BrandLogoFull extends StatelessWidget {
  const BrandLogoFull({
    super.key,
    this.height = 44,
  });

  final double height;

  @override
  Widget build(BuildContext context) {
    return Image.asset(
      'assets/brand/logo_full.png',
      height: height,
      fit: BoxFit.contain,
      filterQuality: FilterQuality.medium,
      semanticLabel: 'EveryoneNAS',
    );
  }
}

/// 环形存储占比图（设计稿总览页同款：蓝紫/天蓝渐变圆环 + 居中百分比）。
class StorageDonutRing extends StatelessWidget {
  const StorageDonutRing({
    super.key,
    required this.ratio,
    this.size = 112,
    this.strokeWidth = 11,
    this.centerLabel = '已使用',
  });

  final double ratio;
  final double size;
  final double strokeWidth;
  final String centerLabel;

  @override
  Widget build(BuildContext context) {
    final clamped = ratio.clamp(0.0, 1.0);
    final percent = (clamped * 100).round();
    final isCompact = size < 84;

    return RepaintBoundary(
      child: Semantics(
        label: '$centerLabel $percent%',
        child: SizedBox(
          width: size,
          height: size,
          child: TweenAnimationBuilder<double>(
            tween: Tween<double>(end: clamped),
            duration: NeuMotion.resolveState(context),
            curve: NeuMotion.resolveStateCurve(context),
            builder: (context, animatedValue, _) {
              return CustomPaint(
                painter: _StorageDonutPainter(
                  progress: animatedValue,
                  strokeWidth: strokeWidth,
                ),
                child: Center(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: <Widget>[
                      Text(
                        '${(animatedValue * 100).round()}%',
                        style: TextStyle(
                          color: NeuTokens.textPrimary,
                          fontSize: isCompact ? 15 : 22,
                          fontWeight: FontWeight.w700,
                          letterSpacing: -0.5,
                          fontFeatures: const <FontFeature>[
                            FontFeature.tabularFigures(),
                          ],
                        ),
                      ),
                      if (centerLabel.isNotEmpty)
                        Text(
                          centerLabel,
                          style: TextStyle(
                            color: NeuTokens.textSecondary,
                            fontSize: isCompact ? 10 : 11.5,
                            fontWeight: FontWeight.w500,
                          ),
                        ),
                    ],
                  ),
                ),
              );
            },
          ),
        ),
      ),
    );
  }
}

class _StorageDonutPainter extends CustomPainter {
  const _StorageDonutPainter({
    required this.progress,
    required this.strokeWidth,
  });

  final double progress;
  final double strokeWidth;

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final radius = (math.min(size.width, size.height) - strokeWidth) / 2;
    if (radius <= 0) return;

    final trackPaint = Paint()
      ..color = const Color(0xFFE6EEF9)
      ..style = PaintingStyle.stroke
      ..strokeWidth = strokeWidth
      ..strokeCap = StrokeCap.round;
    canvas.drawCircle(center, radius, trackPaint);

    if (progress <= 0.001) return;

    final rect = Rect.fromCircle(center: center, radius: radius);
    const sweepGradient = SweepGradient(
      startAngle: -math.pi / 2,
      endAngle: math.pi * 1.5,
      colors: <Color>[
        Color(0xFF38BDF8),
        Color(0xFF2563EB),
        Color(0xFF6366F1),
        Color(0xFF38BDF8),
      ],
      stops: <double>[0.0, 0.45, 0.8, 1.0],
    );

    final progressPaint = Paint()
      ..shader = sweepGradient.createShader(rect)
      ..style = PaintingStyle.stroke
      ..strokeWidth = strokeWidth
      ..strokeCap = StrokeCap.round;

    canvas.drawArc(
      rect,
      -math.pi / 2,
      math.pi * 2 * progress,
      false,
      progressPaint,
    );
  }

  @override
  bool shouldRepaint(covariant _StorageDonutPainter oldDelegate) {
    return oldDelegate.progress != progress ||
        oldDelegate.strokeWidth != strokeWidth;
  }
}

/// 语义状态徽章（圆点 + 柔色底胶囊）。
enum NeuStatusTone { neutral, accent, healthy, warning, danger }

class NeuStatusBadge extends StatelessWidget {
  const NeuStatusBadge({
    super.key,
    required this.label,
    this.tone = NeuStatusTone.neutral,
    this.showDot = true,
  });

  final String label;
  final NeuStatusTone tone;
  final bool showDot;

  (Color fg, Color bg) _resolveColors() {
    return switch (tone) {
      NeuStatusTone.neutral => (NeuTokens.textSecondary, NeuTokens.insetSurface),
      NeuStatusTone.accent => (NeuTokens.accent, NeuTokens.accentSoft),
      NeuStatusTone.healthy => (NeuTokens.healthyAlt, NeuTokens.healthySoft),
      NeuStatusTone.warning => (NeuTokens.warning, NeuTokens.folderGoldSoft),
      NeuStatusTone.danger => (NeuTokens.danger, NeuTokens.dangerSoft),
    };
  }

  @override
  Widget build(BuildContext context) {
    final (fg, bg) = _resolveColors();
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
      decoration: BoxDecoration(
        color: bg,
        borderRadius: const BorderRadius.all(Radius.circular(NeuRadius.pill)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: <Widget>[
          if (showDot) ...<Widget>[
            Container(
              width: 7,
              height: 7,
              decoration: BoxDecoration(
                color: fg,
                shape: BoxShape.circle,
              ),
            ),
            const SizedBox(width: 6),
          ],
          Text(
            label,
            style: TextStyle(
              color: fg,
              fontSize: 12,
              fontWeight: FontWeight.w600,
            ),
          ),
        ],
      ),
    );
  }
}

/// 纯白轻拟态玻璃卡片：支持凸起 / 凹陷 / 浅色语义底色与可选触控按压物理反馈。
class NeuCard extends StatefulWidget {
  const NeuCard({
    super.key,
    required this.child,
    this.padding = const EdgeInsets.all(NeuSpace.md),
    this.radius,
    this.inset = false,
    this.backgroundColor,
    this.borderColor,
    this.semanticLabel,
    this.onTap,
  });

  final Widget child;
  final EdgeInsetsGeometry padding;
  final double? radius;
  final bool inset;
  final Color? backgroundColor;
  final Color? borderColor;
  final String? semanticLabel;
  final VoidCallback? onTap;

  @override
  State<NeuCard> createState() => _NeuCardState();
}

class _NeuCardState extends State<NeuCard> {
  bool _hovered = false;
  bool _pressed = false;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final resolvedRadius = widget.radius ?? neu.borderRadius;
    final interactive = widget.onTap != null;
    final reduced = NeuMotion.isReduced(context);
    final pressDuration = NeuMotion.resolvePress(context);
    final pressCurve = NeuMotion.resolvePressCurve(context);

    final resolvedBg = widget.backgroundColor ??
        (widget.inset
            ? neu.insetColor
            : (_hovered && interactive ? NeuTokens.hoverSurface : neu.surfaceColor));
    final resolvedBorder = widget.borderColor ??
        (widget.inset
            ? const Color(0xFFE0E8F4)
            : (_hovered && interactive
                ? NeuTokens.accent.withValues(alpha: 0.35)
                : NeuTokens.hairlineBorder));
    final resolvedShadows = widget.inset
        ? null
        : (_pressed && interactive ? neu.pressedShadows : neu.extrudedShadows);

    Widget box = AnimatedContainer(
      duration: pressDuration,
      curve: pressCurve,
      padding: widget.padding,
      decoration: BoxDecoration(
        color: resolvedBg,
        borderRadius: BorderRadius.all(Radius.circular(resolvedRadius)),
        border: Border.all(
          color: resolvedBorder,
          width: 1,
        ),
        boxShadow: resolvedShadows,
      ),
      child: widget.child,
    );

    if (interactive) {
      final targetScale =
          (_pressed && !reduced) ? NeuMotion.cardPressedScale : 1.0;
      final targetOffsetY =
          (_hovered && !_pressed && !reduced) ? -1.5 : (_pressed && !reduced ? 1.0 : 0.0);
      box = MouseRegion(
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
            tween: Tween<double>(end: targetOffsetY),
            duration: pressDuration,
            curve: pressCurve,
            builder: (context, dy, inner) {
              return Transform.translate(
                offset: Offset(0, dy),
                child: inner,
              );
            },
            child: TweenAnimationBuilder<double>(
              tween: Tween<double>(end: targetScale),
              duration: pressDuration,
              curve: pressCurve,
              builder: (context, scale, inner) {
                return Transform.scale(
                  scale: scale,
                  alignment: Alignment.center,
                  child: inner,
                );
              },
              child: box,
            ),
          ),
        ),
      );
    }

    final wrapped = RepaintBoundary(child: box);
    if (widget.semanticLabel == null) return wrapped;
    return Semantics(
      container: true,
      button: interactive,
      label: widget.semanticLabel,
      child: wrapped,
    );
  }
}

/// 120Hz 双向顺滑展开/收起容器（展开与收起均有连续高度与透明度过渡）。
class NeuAnimatedExpand extends StatelessWidget {
  const NeuAnimatedExpand({
    super.key,
    required this.expanded,
    required this.child,
  });

  final bool expanded;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final duration = NeuMotion.resolveState(context);
    final curve = NeuMotion.resolveStateCurve(context);
    return ClipRect(
      child: AnimatedAlign(
        alignment: Alignment.topCenter,
        heightFactor: expanded ? 1.0 : 0.0,
        duration: duration,
        curve: curve,
        child: AnimatedOpacity(
          opacity: expanded ? 1.0 : 0.0,
          duration: Duration(milliseconds: (duration.inMilliseconds * 0.85).round()),
          curve: curve,
          child: IgnorePointer(
            ignoring: !expanded,
            child: child,
          ),
        ),
      ),
    );
  }
}

/// 轻拟态内容切换器（顶部对齐、快速退场消除双重文字重影，保留丝滑上浮渐显）。
class NeuMotionSwitcher extends StatelessWidget {
  const NeuMotionSwitcher({
    super.key,
    required this.child,
  });

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final reduced = NeuMotion.isReduced(context);
    return AnimatedSwitcher(
      duration: reduced
          ? const Duration(milliseconds: 100)
          : const Duration(milliseconds: 180),
      reverseDuration: const Duration(milliseconds: 50),
      switchInCurve: Curves.linearToEaseOut,
      switchOutCurve: Curves.easeIn,
      layoutBuilder: (currentChild, previousChildren) {
        return Stack(
          alignment: Alignment.topLeft,
          children: <Widget>[
            ...previousChildren,
            if (currentChild != null) currentChild,
          ],
        );
      },
      transitionBuilder: (widget, animation) {
        if (reduced) {
          return FadeTransition(opacity: animation, child: widget);
        }
        final slide = Tween<Offset>(
          begin: const Offset(0, 0.015),
          end: Offset.zero,
        ).animate(animation);
        return FadeTransition(
          opacity: animation,
          child: SlideTransition(
            position: slide,
            child: widget,
          ),
        );
      },
      child: child,
    );
  }
}

/// 统一轻拟态弹窗容器（220ms `scale(0.95 -> 1.0) + fade` 物理入场动效）。
Future<T?> showNeuDialog<T>({
  required BuildContext context,
  required WidgetBuilder builder,
  bool barrierDismissible = true,
}) {
  return showGeneralDialog<T>(
    context: context,
    barrierDismissible: barrierDismissible,
    barrierLabel: '关闭弹窗',
    barrierColor: const Color(0x520F172A),
    transitionDuration: const Duration(milliseconds: 220),
    pageBuilder: (dialogContext, _, __) => builder(dialogContext),
    transitionBuilder: (dialogContext, animation, secondaryAnimation, child) {
      final curved = CurvedAnimation(
        parent: animation,
        curve: Curves.linearToEaseOut,
        reverseCurve: Curves.easeIn,
      );
      final scale = Tween<double>(begin: 0.94, end: 1.0).animate(curved);
      return FadeTransition(
        opacity: curved,
        child: ScaleTransition(
          scale: scale,
          child: child,
        ),
      );
    },
  );
}

enum NeuButtonTone { neutral, primary, danger, healthy }

/// 轻拟态按钮：主按钮使用设计稿同款科技蓝渐变 + 柔和投影，中性按钮使用纯白 + 细腻冷蓝边框。
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

  bool get _enabled => widget.onPressed != null && !widget.loading;

  @override
  void dispose() {
    _focusNode.dispose();
    super.dispose();
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

  Color? _resolveBackground(NeuTheme neu) {
    if (!_enabled) return neu.insetColor;
    return switch (widget.tone) {
      NeuButtonTone.neutral =>
        _hovered && !_pressed ? NeuTokens.hoverSurface : neu.surfaceColor,
      NeuButtonTone.primary => _pressed ? NeuTokens.accentPressed : null,
      NeuButtonTone.danger => NeuTokens.danger,
      NeuButtonTone.healthy => NeuTokens.healthy,
    };
  }

  Gradient? _resolveGradient() {
    if (!_enabled || widget.tone != NeuButtonTone.primary) {
      return null;
    }
    if (_pressed) {
      return const LinearGradient(
        begin: Alignment.centerLeft,
        end: Alignment.centerRight,
        colors: <Color>[NeuTokens.accentPressed, NeuTokens.accent],
      );
    }
    return NeuTokens.primaryGradient;
  }

  List<BoxShadow>? _resolveShadows(NeuTheme neu) {
    if (!_enabled) return null;
    if (_pressed) return neu.pressedShadows;
    if (widget.tone == NeuButtonTone.primary) {
      return NeuTokens.primaryButtonShadows;
    }
    return NeuTokens.controlShadows;
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
    final gradient = _resolveGradient();
    final targetScale =
        (_pressed && _enabled && !reduced) ? NeuMotion.pressedScale : 1.0;

    final content = Row(
      mainAxisSize: widget.expanded ? MainAxisSize.max : MainAxisSize.min,
      mainAxisAlignment: MainAxisAlignment.center,
      children: <Widget>[
        if (widget.loading || widget.icon != null) ...<Widget>[
          SizedBox(
            width: 18,
            height: 18,
            child: AnimatedSwitcher(
              duration: const Duration(milliseconds: 150),
              child: widget.loading
                  ? SizedBox(
                      key: const ValueKey<String>('loading'),
                      width: 15,
                      height: 15,
                      child: CircularProgressIndicator(
                        strokeWidth: 2,
                        valueColor: AlwaysStoppedAnimation<Color>(fg),
                      ),
                    )
                  : Icon(
                      widget.icon,
                      key: ValueKey<IconData?>(widget.icon),
                      size: 17,
                      color: fg,
                    ),
            ),
          ),
          const SizedBox(width: 6),
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
        horizontal: 16,
        vertical: 10,
      ),
      decoration: BoxDecoration(
        color: gradient == null ? bg : null,
        gradient: gradient,
        borderRadius: BorderRadius.all(Radius.circular(widget.radius)),
        border: widget.tone == NeuButtonTone.neutral || !_enabled
            ? Border.all(
                color: _hovered && _enabled
                    ? NeuTokens.controlOutline
                    : NeuTokens.hairlineBorder,
                width: 1.0,
              )
            : null,
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
              onTapDown: _enabled ? (_) => _setPressed(true) : null,
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

/// 轻拟态开关（支持完整标题行或紧凑纯开关两种展示）。
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
  bool _pressed = false;

  bool get _enabled => widget.onChanged != null;

  @override
  void dispose() {
    _focusNode.dispose();
    super.dispose();
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
    final textTheme = Theme.of(context).textTheme;
    final stateDuration = NeuMotion.resolveState(context);
    final stateCurve = NeuMotion.resolveStateCurve(context);
    final pressDuration = NeuMotion.resolvePress(context);
    final pressCurve = NeuMotion.resolvePressCurve(context);
    final reduced = NeuMotion.isReduced(context);

    final thumbScale =
        (_pressed && _enabled && !reduced) ? NeuMotion.pressedScale : 1.0;

    final switchTrack = AnimatedContainer(
      duration: stateDuration,
      curve: stateCurve,
      width: 48,
      height: 28,
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(
        color: !_enabled
            ? const Color(0xFFE2E8F0)
            : (widget.value ? null : const Color(0xFFD6DFEE)),
        gradient: _enabled && widget.value ? NeuTokens.primaryGradient : null,
        borderRadius: const BorderRadius.all(Radius.circular(NeuRadius.pill)),
      ),
      child: AnimatedAlign(
        alignment: widget.value ? Alignment.centerRight : Alignment.centerLeft,
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
            decoration: const BoxDecoration(
              color: Colors.white,
              shape: BoxShape.circle,
              boxShadow: <BoxShadow>[
                BoxShadow(
                  color: Color(0x260F172A),
                  offset: Offset(0, 2),
                  blurRadius: 4,
                ),
              ],
            ),
          ),
        ),
      ),
    );

    final compactMode = widget.title.isEmpty && widget.subtitle == null;

    return RepaintBoundary(
      child: Semantics(
        toggled: widget.value,
        enabled: _enabled,
        label: widget.semanticLabel ??
            (widget.title.isEmpty ? '开关' : widget.title),
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
              onTapDown: _enabled ? (_) => setState(() => _pressed = true) : null,
              onTapUp: _enabled ? (_) => setState(() => _pressed = false) : null,
              onTapCancel:
                  _enabled ? () => setState(() => _pressed = false) : null,
              onTap: _enabled ? _toggle : null,
              child: compactMode
                  ? Padding(
                      padding: const EdgeInsets.all(4),
                      child: switchTrack,
                    )
                  : ConstrainedBox(
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
                            switchTrack,
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

/// 分段切换器（设计稿设备页/快照页同款：选中态蓝渐变胶囊白字 + 冰蓝凹陷底槽）。
class NeuSegmentedControl<T> extends StatefulWidget {
  const NeuSegmentedControl({
    super.key,
    required this.segments,
    required this.value,
    required this.onChanged,
    this.enabled = true,
    this.semanticLabel,
  }) : assert(
          segments.length >= 2,
          'NeuSegmentedControl requires at least 2 segments.',
        );

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

  @override
  void dispose() {
    _focusNode.dispose();
    super.dispose();
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

    const double outerRadius = NeuRadius.segmentedTrack;
    const double trackPadding = 3.0;
    final double thumbRadius = NeuRadius.concentric(outerRadius, trackPadding);

    return RepaintBoundary(
      child: Semantics(
        container: true,
        label: widget.semanticLabel,
        child: Focus(
          focusNode: _focusNode,
          canRequestFocus: widget.enabled,
          onKeyEvent: _handleKey,
          child: Container(
            padding: const EdgeInsets.all(trackPadding),
            decoration: BoxDecoration(
              color: neu.insetColor,
              borderRadius: const BorderRadius.all(
                Radius.circular(outerRadius),
              ),
              border: Border.all(
                color: NeuTokens.hairlineBorder,
                width: 1.0,
              ),
            ),
            child: SizedBox(
              height: 38,
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
                            gradient: NeuTokens.primaryGradient,
                            borderRadius: BorderRadius.all(
                              Radius.circular(thumbRadius),
                            ),
                            boxShadow: const <BoxShadow>[
                              BoxShadow(
                                color: Color(0x282563EB),
                                offset: Offset(0, 3),
                                blurRadius: 8,
                              ),
                            ],
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
                                    ? () => widget.onChanged(segment.value)
                                    : null,
                                child: Center(
                                  child: Padding(
                                    padding: const EdgeInsets.symmetric(
                                      horizontal: 8,
                                    ),
                                    child: AnimatedDefaultTextStyle(
                                      duration: duration,
                                      curve: curve,
                                      style: (Theme.of(context)
                                                  .textTheme
                                                  .labelLarge ??
                                              const TextStyle(fontSize: 13.5))
                                          .copyWith(
                                        fontSize: 13.5,
                                        color: segment.value == widget.value
                                            ? Colors.white
                                            : NeuTokens.textSecondary,
                                      ),
                                      child: Text(
                                        segment.label,
                                        maxLines: 1,
                                        overflow: TextOverflow.ellipsis,
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

/// 凹陷输入框（冰蓝槽 `#EEF3FA` + 聚焦科技蓝描边）。
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
        : (_focused ? NeuTokens.accent : NeuTokens.hairlineBorder);

    return RepaintBoundary(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (widget.label != null) ...<Widget>[
            Text(
              widget.label!,
              style: Theme.of(context).textTheme.labelMedium?.copyWith(
                    color: NeuTokens.textPrimary,
                  ),
            ),
            const SizedBox(height: 6),
          ],
          AnimatedContainer(
            duration: duration,
            curve: curve,
            constraints: BoxConstraints(minHeight: minTouch),
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 2),
            decoration: BoxDecoration(
              color: _focused ? NeuTokens.surface : neu.insetColor,
              borderRadius: const BorderRadius.all(
                Radius.circular(NeuRadius.control),
              ),
              border: Border.all(
                color: borderColor,
                width: _focused || hasError ? 1.4 : 1.0,
              ),
              boxShadow: _focused
                  ? const <BoxShadow>[
                      BoxShadow(
                        color: Color(0x1F2563EB),
                        blurRadius: 8,
                        spreadRadius: 0,
                      ),
                    ]
                  : null,
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
                      color:
                          _focused ? NeuTokens.accent : NeuTokens.textSecondary,
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
                          fontSize: 15,
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
                            fontSize: 14.5,
                            color: NeuTokens.textSecondary,
                          ),
                      contentPadding: const EdgeInsets.symmetric(vertical: 11),
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

/// 轻拟态容量/进度条：设计稿同款圆角蓝渐变进度条 + 冰蓝槽底。
class NeuProgressBar extends StatelessWidget {
  const NeuProgressBar({
    super.key,
    required this.value,
    this.height = 8,
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
    const radius = BorderRadius.all(Radius.circular(NeuRadius.pill));
    final percent = (value * 100).round();
    final usePrimaryGradient = fillColor == NeuTokens.accent;

    final track = Container(
      height: height,
      decoration: BoxDecoration(
        color: neu.insetColor,
        borderRadius: radius,
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
              color: usePrimaryGradient ? null : fillColor,
              gradient: usePrimaryGradient ? NeuTokens.primaryGradient : null,
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

/// 下拉选择项模型。
@immutable
class NeuDropdownOption<T> {
  const NeuDropdownOption({
    required this.value,
    required this.label,
    this.subtitle,
  });

  final T value;
  final String label;
  final String? subtitle;
}

/// 冰蓝槽下拉选择框。
class NeuDropdown<T> extends StatelessWidget {
  const NeuDropdown({
    super.key,
    required this.options,
    required this.value,
    required this.onChanged,
    this.label,
    this.emptyHint = '暂无可选项',
    this.prefixIcon,
    this.enabled = true,
  });

  final List<NeuDropdownOption<T>> options;
  final T? value;
  final ValueChanged<T?>? onChanged;
  final String? label;
  final String emptyHint;
  final IconData? prefixIcon;
  final bool enabled;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final minTouch = NeuTokens.resolveMinTouchTarget(context);
    final isEmpty = options.isEmpty;
    final canSelect = enabled && !isEmpty && onChanged != null;
    final hasMatch = options.any((o) => o.value == value);
    final effectiveValue = hasMatch ? value : null;

    return RepaintBoundary(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (label != null) ...<Widget>[
            Text(
              label!,
              style: Theme.of(context).textTheme.labelMedium?.copyWith(
                    color: NeuTokens.textPrimary,
                  ),
            ),
            const SizedBox(height: 6),
          ],
          Container(
            constraints: BoxConstraints(minHeight: minTouch),
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 2),
            decoration: BoxDecoration(
              color: neu.insetColor,
              borderRadius: const BorderRadius.all(
                Radius.circular(NeuRadius.control),
              ),
              border: Border.all(
                color: NeuTokens.hairlineBorder,
                width: 1.0,
              ),
            ),
            child: Row(
              children: <Widget>[
                if (prefixIcon != null) ...<Widget>[
                  Icon(prefixIcon, size: 18, color: NeuTokens.textSecondary),
                  const SizedBox(width: NeuSpace.sm),
                ],
                Expanded(
                  child: isEmpty
                      ? Padding(
                          padding: const EdgeInsets.symmetric(vertical: 11),
                          child: Text(
                            emptyHint,
                            style: Theme.of(context)
                                .textTheme
                                .bodyMedium
                                ?.copyWith(
                                  fontSize: 14.5,
                                  color: NeuTokens.textSecondary,
                                ),
                          ),
                        )
                      : DropdownButtonHideUnderline(
                          child: DropdownButton<T>(
                            value: effectiveValue,
                            isExpanded: true,
                            dropdownColor: NeuTokens.surface,
                            borderRadius: const BorderRadius.all(
                              Radius.circular(NeuRadius.control),
                            ),
                            icon: Icon(
                              Icons.keyboard_arrow_down_rounded,
                              color: canSelect
                                  ? NeuTokens.textPrimary
                                  : NeuTokens.textSecondary,
                            ),
                            hint: Text(
                              emptyHint,
                              style: Theme.of(context)
                                  .textTheme
                                  .bodyMedium
                                  ?.copyWith(
                                    fontSize: 14.5,
                                    color: NeuTokens.textSecondary,
                                  ),
                            ),
                            style: Theme.of(context)
                                .textTheme
                                .bodyMedium
                                ?.copyWith(
                                  fontSize: 14.5,
                                  color: NeuTokens.textPrimary,
                                ),
                            onChanged: canSelect ? onChanged : null,
                            items: <DropdownMenuItem<T>>[
                              for (final opt in options)
                                DropdownMenuItem<T>(
                                  value: opt.value,
                                  child: Text(
                                    opt.subtitle == null
                                        ? opt.label
                                        : '${opt.label} (${opt.subtitle})',
                                    overflow: TextOverflow.ellipsis,
                                  ),
                                ),
                            ],
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
