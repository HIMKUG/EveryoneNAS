// EveryoneNAS 全新柔冰蓝 / 纯白轻拟态玻璃卡片设计系统
// 完整保留 NeuCard / NeuButton / NeuSwitch / NeuInsetField / NeuSegmentedControl /
// NeuDropdown / NeuProgressBar 公开 API，并新增 NeuStatusBadge / StorageDonutRing /
// BrandLogoMark / BrandLogoFull。

import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import 'skins.dart';

enum MotionSpeed {
  fast(1.0, '极速'),
  balanced(1.46, '均衡'),
  elegant(1.7, '优雅');

  const MotionSpeed(this.scale, this.label);

  final double scale;
  final String label;
}

/// 统一动效 Token。
abstract final class NeuMotion {
  static const Curve enterCurve = Cubic(0.16, 1.0, 0.3, 1.0);
  static const Curve exitCurve = Cubic(0.4, 0.0, 1.0, 1.0);
  static const Curve appleSheetCurve = Cubic(0.32, 0.72, 0.0, 1.0);
  static const Curve revealCurve = Cubic(0.22, 1.0, 0.36, 1.0);

  static const Duration pressDuration = Duration(milliseconds: 180);
  static const Duration pageEnterDuration = Duration(milliseconds: 220);
  static const Duration pageExitDuration = Duration(milliseconds: 140);
  static const Duration modalDuration = Duration(milliseconds: 240);
  static const Duration revealDuration = Duration(milliseconds: 280);
  static const Duration sheetDuration = Duration(milliseconds: 420);

  static const Curve pressCurve = enterCurve;
  static const Duration stateDuration = pageEnterDuration;
  static const Curve stateCurve = enterCurve;

  static const Duration reducedDuration = Duration(milliseconds: 100);
  static const Curve reducedCurve = Curves.easeOut;

  static MotionSpeed speed = MotionSpeed.balanced;

  static const double pressedScale = 0.968;
  static const double cardPressedScale = 0.985;

  static bool isReduced(BuildContext context) {
    return MediaQuery.maybeDisableAnimationsOf(context) ?? false;
  }

  static MotionSpeed resolveSpeed(BuildContext context) {
    return Theme.of(context).extension<NeuTheme>()?.motionSpeed ?? speed;
  }

  static Duration scaleDuration(
    Duration duration,
    MotionSpeed speed, {
    bool reduced = false,
  }) {
    if (reduced) return reducedDuration;
    return Duration(
      microseconds: (duration.inMicroseconds * speed.scale).round(),
    );
  }

  static Duration resolvePress(BuildContext context) {
    return scaleDuration(
      pressDuration,
      resolveSpeed(context),
      reduced: isReduced(context),
    );
  }

  static Duration resolvePageEnter(BuildContext context) {
    return scaleDuration(
      pageEnterDuration,
      resolveSpeed(context),
      reduced: isReduced(context),
    );
  }

  static Duration resolvePageExit(BuildContext context) {
    return scaleDuration(
      pageExitDuration,
      resolveSpeed(context),
      reduced: isReduced(context),
    );
  }

  static Duration resolveModal(BuildContext context) {
    return scaleDuration(
      modalDuration,
      resolveSpeed(context),
      reduced: isReduced(context),
    );
  }

  static Duration resolveReveal(BuildContext context) {
    return scaleDuration(
      revealDuration,
      resolveSpeed(context),
      reduced: isReduced(context),
    );
  }

  static Duration resolveSheet(BuildContext context) {
    return scaleDuration(
      sheetDuration,
      resolveSpeed(context),
      reduced: isReduced(context),
    );
  }

  static Duration resolveState(BuildContext context) {
    return resolvePageEnter(context);
  }

  static Curve resolvePressCurve(BuildContext context) {
    return isReduced(context) ? reducedCurve : pressCurve;
  }

  static Curve resolveStateCurve(BuildContext context) {
    return isReduced(context) ? reducedCurve : stateCurve;
  }

  static Curve resolveExitCurve(BuildContext context) {
    return isReduced(context) ? reducedCurve : exitCurve;
  }

  static Curve resolveSheetCurve(BuildContext context) {
    return isReduced(context) ? reducedCurve : appleSheetCurve;
  }

  static Curve resolveRevealCurve(BuildContext context) {
    return isReduced(context) ? reducedCurve : revealCurve;
  }
}

/// 同心圆角 Token 与嵌套计算公式。
abstract final class NeuRadius {
  static const double card = 18.0;
  static const double group = 14.0;
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

/// 布局与平台 Token；组件颜色统一从 [NeuTheme] 读取。
abstract final class NeuTokens {
  static const String appFontFamily = 'SourceHanSansSC';
  static const double breakpointWide = 840.0;

  static const double minTouchTarget = 42.0;
  static const double minMobileTouchTarget = 46.0;

  static const double spaceXs = NeuSpace.xs;
  static const double spaceSm = NeuSpace.sm;
  static const double spaceMd = NeuSpace.md;
  static const double spaceLg = NeuSpace.lg;

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
    this.palette = defaultLightPalette,
    this.brightness = Brightness.light,
    this.skinId = defaultSkinId,
    this.motionSpeed = MotionSpeed.balanced,
    this.borderRadius = NeuRadius.card,
  });

  final SkinPalette palette;
  final Brightness brightness;
  final String skinId;
  final MotionSpeed motionSpeed;
  final double borderRadius;

  static NeuTheme of(BuildContext context) {
    final ext = Theme.of(context).extension<NeuTheme>();
    final platformBrightness = Theme.of(context).brightness;
    return ext ??
        NeuTheme(
          palette: platformBrightness == Brightness.dark
              ? defaultDarkPalette
              : defaultLightPalette,
          brightness: platformBrightness,
        );
  }

  Color get canvas => palette.canvas;
  Color get surface => palette.surface;
  Color get surfaceColor => palette.surface;
  Color get insetSurface => palette.insetSurface;
  Color get insetColor => palette.insetSurface;
  Color get hoverSurface => palette.hoverSurface;
  Color get chromeSurface => palette.chromeSurface;
  Color get surfacePressed => palette.surfacePressed;
  Color get hairlineBorder => palette.hairlineBorder;
  Color get controlOutline => palette.controlOutline;
  Color get dividerSubtle => palette.dividerSubtle;
  Color get textPrimary => palette.textPrimary;
  Color get textSecondary => palette.textSecondary;
  Color get slateMuted => palette.slateMuted;
  Color get accent => palette.accent;
  Color get accentLight => palette.accentLight;
  Color get accentPressed => palette.accentPressed;
  Color get accentSoft => palette.accentSoft;
  Color get onAccent => palette.onAccent;
  Color get healthy => palette.healthy;
  Color get healthyAlt => palette.healthyAlt;
  Color get healthyButton => palette.healthyButton;
  Color get healthySoft => palette.healthySoft;
  Color get folderGold => palette.folderGold;
  Color get folderGoldSoft => palette.folderGoldSoft;
  Color get warning => palette.warning;
  Color get danger => palette.danger;
  Color get dangerSoft => palette.dangerSoft;
  Color get disabledDarkSurface => palette.disabledDarkSurface;
  Color get disabledDarkBorder => palette.disabledDarkBorder;
  Color get disabledDarkForeground => palette.disabledDarkForeground;
  Color get chartSky => palette.chartSky;
  Color get chartIndigo => palette.chartIndigo;
  Color get chartIndigoSoft => palette.chartIndigoSoft;
  Color get timelineInactive => palette.timelineInactive;
  Color get timelineHalo => palette.timelineHalo;
  Color get timelineLine => palette.timelineLine;
  Color get dialogBarrier => palette.dialogBarrier;
  Color get switchTrackDisabled => palette.switchTrackDisabled;
  Color get switchTrackOff => palette.switchTrackOff;
  List<BoxShadow> get extrudedShadows => palette.extrudedShadows;
  List<BoxShadow> get controlShadows => palette.controlShadows;
  List<BoxShadow> get primaryButtonShadows => palette.primaryButtonShadows;
  List<BoxShadow> get pressedShadows => palette.pressedShadows;
  Color get transparent => palette.transparent;

  LinearGradient get primaryGradient => LinearGradient(
        begin: Alignment.centerLeft,
        end: Alignment.centerRight,
        colors: <Color>[accent, accentLight],
      );

  LinearGradient get pressedGradient => LinearGradient(
        begin: Alignment.centerLeft,
        end: Alignment.centerRight,
        colors: <Color>[accentPressed, accent],
      );

  SweepGradient get chartGradient => SweepGradient(
        startAngle: -math.pi / 2,
        endAngle: math.pi * 1.5,
        colors: <Color>[chartSky, accent, chartIndigo, chartSky],
        stops: const <double>[0.0, 0.45, 0.8, 1.0],
      );

  bool get isDark => brightness == Brightness.dark;

  @override
  NeuTheme copyWith({
    SkinPalette? palette,
    Brightness? brightness,
    String? skinId,
    MotionSpeed? motionSpeed,
    double? borderRadius,
  }) {
    return NeuTheme(
      palette: palette ?? this.palette,
      brightness: brightness ?? this.brightness,
      skinId: skinId ?? this.skinId,
      motionSpeed: motionSpeed ?? this.motionSpeed,
      borderRadius: borderRadius ?? this.borderRadius,
    );
  }

  @override
  NeuTheme lerp(ThemeExtension<NeuTheme>? other, double t) {
    if (other is! NeuTheme) return this;
    final nextBrightness = t < 0.5 ? brightness : other.brightness;
    return NeuTheme(
      palette: palette.lerp(other.palette, t),
      brightness: nextBrightness,
      skinId: t < 0.5 ? skinId : other.skinId,
      motionSpeed: t < 0.5 ? motionSpeed : other.motionSpeed,
      borderRadius: borderRadius + (other.borderRadius - borderRadius) * t,
    );
  }

  @override
  bool operator ==(Object other) {
    if (identical(this, other)) return true;
    return other is NeuTheme &&
        other.palette == palette &&
        other.brightness == brightness &&
        other.skinId == skinId &&
        other.motionSpeed == motionSpeed &&
        other.borderRadius == borderRadius;
  }

  @override
  int get hashCode => Object.hash(
        palette,
        brightness,
        skinId,
        motionSpeed,
        borderRadius,
      );
}

ThemeData buildLanVaultTheme({
  SkinPalette? palette,
  Brightness brightness = Brightness.light,
  String skinId = defaultSkinId,
  MotionSpeed motionSpeed = MotionSpeed.balanced,
}) {
  final activePalette = palette ??
      (brightness == Brightness.dark
          ? defaultDarkPalette
          : defaultLightPalette);
  final colorScheme = brightness == Brightness.dark
      ? ColorScheme.dark(
          surface: activePalette.surface,
          primary: activePalette.accent,
          onPrimary: activePalette.onAccent,
          secondary: activePalette.healthy,
          onSecondary: pickContrastInk(activePalette.healthy),
          error: activePalette.danger,
          onError: pickContrastInk(activePalette.danger),
          onSurface: activePalette.textPrimary,
          onSurfaceVariant: activePalette.textSecondary,
          outline: activePalette.hairlineBorder,
        )
      : ColorScheme.light(
          surface: activePalette.surface,
          primary: activePalette.accent,
          onPrimary: activePalette.onAccent,
          secondary: activePalette.healthy,
          onSecondary: pickContrastInk(activePalette.healthy),
          error: activePalette.danger,
          onError: pickContrastInk(activePalette.danger),
          onSurface: activePalette.textPrimary,
          onSurfaceVariant: activePalette.textSecondary,
          outline: activePalette.hairlineBorder,
        );
  final neu = NeuTheme(
    palette: activePalette,
    brightness: brightness,
    skinId: skinId,
    motionSpeed: motionSpeed,
  );

  return ThemeData(
    useMaterial3: true,
    fontFamily: NeuTokens.appFontFamily,
    colorScheme: colorScheme,
    scaffoldBackgroundColor: activePalette.canvas,
    canvasColor: activePalette.canvas,
    dividerColor: activePalette.dividerSubtle,
    dialogTheme: DialogThemeData(
      backgroundColor: activePalette.surface,
      barrierColor: activePalette.dialogBarrier,
    ),
    textTheme: TextTheme(
      headlineSmall: TextStyle(
        color: activePalette.textPrimary,
        fontSize: 24,
        fontWeight: FontWeight.w700,
        letterSpacing: -0.4,
      ),
      titleMedium: TextStyle(
        color: activePalette.textPrimary,
        fontSize: 16,
        fontWeight: FontWeight.w600,
        letterSpacing: -0.2,
      ),
      titleSmall: TextStyle(
        color: activePalette.textPrimary,
        fontSize: 14,
        fontWeight: FontWeight.w600,
      ),
      bodyMedium: TextStyle(
        color: activePalette.textPrimary,
        fontSize: 14,
        height: 1.45,
      ),
      bodySmall: TextStyle(
        color: activePalette.textSecondary,
        fontSize: 13,
        height: 1.4,
      ),
      labelLarge: TextStyle(
        color: activePalette.textPrimary,
        fontSize: 14,
        fontWeight: FontWeight.w600,
      ),
      labelMedium: TextStyle(
        color: activePalette.textSecondary,
        fontSize: 12,
        fontWeight: FontWeight.w600,
      ),
    ),
    extensions: <ThemeExtension<dynamic>>[neu],
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
    final neu = NeuTheme.of(context);

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
                  palette: neu.palette,
                ),
                child: Center(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: <Widget>[
                      Text(
                        '${(animatedValue * 100).round()}%',
                        style: TextStyle(
                          color: neu.textPrimary,
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
                            color: neu.textSecondary,
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
    required this.palette,
  });

  final double progress;
  final double strokeWidth;
  final SkinPalette palette;

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final radius = (math.min(size.width, size.height) - strokeWidth) / 2;
    if (radius <= 0) return;

    final trackPaint = Paint()
      ..color = palette.timelineInactive
      ..style = PaintingStyle.stroke
      ..strokeWidth = strokeWidth
      ..strokeCap = StrokeCap.round;
    canvas.drawCircle(center, radius, trackPaint);

    if (progress <= 0.001) return;

    final rect = Rect.fromCircle(center: center, radius: radius);
    final sweepGradient = SweepGradient(
      startAngle: -math.pi / 2,
      endAngle: math.pi * 1.5,
      colors: <Color>[
        palette.chartSky,
        palette.accent,
        palette.chartIndigo,
        palette.chartSky,
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
        oldDelegate.strokeWidth != strokeWidth ||
        oldDelegate.palette != palette;
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

  (Color fg, Color bg) _resolveColors(NeuTheme neu) {
    return switch (tone) {
      NeuStatusTone.neutral => (neu.textSecondary, neu.insetSurface),
      NeuStatusTone.accent => (neu.accent, neu.accentSoft),
      NeuStatusTone.healthy => (neu.healthyAlt, neu.healthySoft),
      NeuStatusTone.warning => (neu.warning, neu.folderGoldSoft),
      NeuStatusTone.danger => (neu.danger, neu.dangerSoft),
    };
  }

  @override
  Widget build(BuildContext context) {
    final (fg, bg) = _resolveColors(NeuTheme.of(context));
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
  final FocusNode _focusNode = FocusNode(debugLabel: 'NeuCard');
  bool _hovered = false;
  bool _pressed = false;
  bool _focused = false;

  @override
  void dispose() {
    _focusNode.dispose();
    super.dispose();
  }

  KeyEventResult _handleKey(FocusNode node, KeyEvent event) {
    if (widget.onTap == null || event is! KeyDownEvent) {
      return KeyEventResult.ignored;
    }
    if (event.logicalKey == LogicalKeyboardKey.enter ||
        event.logicalKey == LogicalKeyboardKey.numpadEnter ||
        event.logicalKey == LogicalKeyboardKey.space) {
      widget.onTap!();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

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
            : (_hovered && interactive ? neu.hoverSurface : neu.surfaceColor));
    final resolvedBorder = widget.borderColor ??
        (widget.inset
            ? neu.hairlineBorder
            : ((_hovered || _focused) && interactive
                ? neu.accent.withValues(alpha: _focused ? 0.55 : 0.35)
                : neu.hairlineBorder));
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
          width: _focused && interactive ? 1.5 : 1,
        ),
        boxShadow: resolvedShadows,
      ),
      child: widget.child,
    );

    if (interactive) {
      final targetScale =
          (_pressed && !reduced) ? NeuMotion.cardPressedScale : 1.0;
      final targetOffsetY = (_hovered && !_pressed && !reduced)
          ? -1.5
          : (_pressed && !reduced ? 1.0 : 0.0);
      box = Focus(
        focusNode: _focusNode,
        onFocusChange: (value) {
          if (_focused != value) {
            setState(() => _focused = value);
          }
        },
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
          duration:
              Duration(milliseconds: (duration.inMilliseconds * 0.85).round()),
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
          ? NeuMotion.reducedDuration
          : NeuMotion.resolvePageEnter(context),
      reverseDuration: reduced
          ? NeuMotion.reducedDuration
          : NeuMotion.resolvePageExit(context),
      switchInCurve: NeuMotion.resolvePressCurve(context),
      switchOutCurve: NeuMotion.resolveExitCurve(context),
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
  final reduced = NeuMotion.isReduced(context);
  final neu = NeuTheme.of(context);
  return showGeneralDialog<T>(
    context: context,
    barrierDismissible: barrierDismissible,
    barrierLabel: '关闭弹窗',
    barrierColor: neu.dialogBarrier,
    transitionDuration: NeuMotion.resolveModal(context),
    pageBuilder: (dialogContext, _, __) => builder(dialogContext),
    transitionBuilder: (dialogContext, animation, secondaryAnimation, child) {
      final curved = CurvedAnimation(
        parent: animation,
        curve: NeuMotion.resolveSheetCurve(context),
        reverseCurve: NeuMotion.resolveExitCurve(context),
      );
      if (reduced) {
        return FadeTransition(opacity: curved, child: child);
      }
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
    this.disabledLook = false,
    this.radius = NeuRadius.control,
    this.semanticLabel,
  });

  final String label;
  final VoidCallback? onPressed;
  final IconData? icon;
  final NeuButtonTone tone;
  final bool loading;
  final bool expanded;

  /// 为 true 时渲染为深炭黑色"未就绪/禁用态"，但若仍传入 [onPressed] 则保留点击以弹出提示。
  final bool disabledLook;
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
  bool get _looksDisabled => !_enabled || widget.disabledLook;

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

  Color _resolveForeground(NeuTheme neu, Color background) {
    if (widget.disabledLook) return neu.disabledDarkForeground;
    if (!_enabled) return neu.textSecondary;
    return switch (widget.tone) {
      NeuButtonTone.neutral => neu.textPrimary,
      NeuButtonTone.primary => pickContrastInk(background),
      NeuButtonTone.danger => pickContrastInk(background),
      NeuButtonTone.healthy => pickContrastInk(background),
    };
  }

  Color? _resolveBackground(NeuTheme neu) {
    if (widget.disabledLook) return neu.disabledDarkSurface;
    if (!_enabled) return neu.insetColor;
    return switch (widget.tone) {
      NeuButtonTone.neutral =>
        _hovered && !_pressed ? neu.hoverSurface : neu.surfaceColor,
      NeuButtonTone.primary => _pressed ? neu.accentPressed : neu.accent,
      NeuButtonTone.danger => neu.danger,
      NeuButtonTone.healthy => neu.healthyButton,
    };
  }

  Gradient? _resolveGradient(NeuTheme neu) {
    if (_looksDisabled || widget.tone != NeuButtonTone.primary) {
      return null;
    }
    if (_pressed) {
      return neu.pressedGradient;
    }
    return neu.primaryGradient;
  }

  List<BoxShadow>? _resolveShadows(NeuTheme neu) {
    if (_looksDisabled) return null;
    if (_pressed) return neu.pressedShadows;
    if (widget.tone == NeuButtonTone.primary) {
      return neu.primaryButtonShadows;
    }
    return neu.controlShadows;
  }

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final reduced = NeuMotion.isReduced(context);
    final duration = NeuMotion.resolvePress(context);
    final curve = NeuMotion.resolvePressCurve(context);
    final minTouch = NeuTokens.resolveMinTouchTarget(context);
    final bg = _resolveBackground(neu);
    final gradient = _resolveGradient(neu);
    final fg = _resolveForeground(
      neu,
      _pressed && widget.tone == NeuButtonTone.primary
          ? neu.accentPressed
          : bg ?? neu.accent,
    );
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
              duration: NeuMotion.resolveState(context),
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
        border: widget.disabledLook
            ? Border.all(color: neu.disabledDarkBorder, width: 1.0)
            : (widget.tone == NeuButtonTone.neutral || !_enabled
                ? Border.all(
                    color: _hovered && _enabled
                        ? neu.controlOutline
                        : neu.hairlineBorder,
                    width: 1.0,
                  )
                : null),
        boxShadow: _resolveShadows(neu),
      ),
      child: content,
    );

    return RepaintBoundary(
      child: Semantics(
        button: true,
        enabled: !_looksDisabled,
        label: widget.semanticLabel ?? widget.label,
        child: Focus(
          focusNode: _focusNode,
          canRequestFocus: _enabled,
          onKeyEvent: _handleKey,
          child: MouseRegion(
            cursor: !_looksDisabled
                ? SystemMouseCursors.click
                : (_enabled
                    ? SystemMouseCursors.basic
                    : SystemMouseCursors.forbidden),
            onEnter: (_) => _setHovered(true),
            onExit: (_) {
              _setHovered(false);
              _setPressed(false);
            },
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTapDown: _enabled && !widget.disabledLook
                  ? (_) => _setPressed(true)
                  : null,
              onTapUp: _enabled && !widget.disabledLook
                  ? (_) => _setPressed(false)
                  : null,
              onTapCancel: _enabled && !widget.disabledLook
                  ? () => _setPressed(false)
                  : null,
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
    final neu = NeuTheme.of(context);
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
            ? neu.switchTrackDisabled
            : (widget.value ? null : neu.switchTrackOff),
        gradient: _enabled && widget.value ? neu.primaryGradient : null,
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
            decoration: BoxDecoration(
              color: pickContrastInk(neu.switchTrackOff),
              shape: BoxShape.circle,
              boxShadow: neu.controlShadows,
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
              onTapDown:
                  _enabled ? (_) => setState(() => _pressed = true) : null,
              onTapUp:
                  _enabled ? (_) => setState(() => _pressed = false) : null,
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
                                          ? neu.textPrimary
                                          : neu.textSecondary,
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
                color: neu.hairlineBorder,
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
                            gradient: neu.primaryGradient,
                            borderRadius: BorderRadius.all(
                              Radius.circular(thumbRadius),
                            ),
                            boxShadow: <BoxShadow>[
                              BoxShadow(
                                color: neu.accent.withValues(alpha: 0.28),
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
                                            ? neu.onAccent
                                            : neu.textSecondary,
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
    final borderColor =
        hasError ? neu.danger : (_focused ? neu.accent : neu.hairlineBorder);

    return RepaintBoundary(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          if (widget.label != null) ...<Widget>[
            Text(
              widget.label!,
              style: Theme.of(context).textTheme.labelMedium?.copyWith(
                    color: neu.textPrimary,
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
              color: _focused ? neu.surface : neu.insetColor,
              borderRadius: const BorderRadius.all(
                Radius.circular(NeuRadius.control),
              ),
              border: Border.all(
                color: borderColor,
                width: _focused || hasError ? 1.4 : 1.0,
              ),
              boxShadow: _focused
                  ? <BoxShadow>[
                      BoxShadow(
                        color: neu.accent.withValues(alpha: 0.18),
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
                    padding:
                        EdgeInsets.only(top: widget.maxLines == 1 ? 0 : 12),
                    child: Icon(
                      widget.prefixIcon,
                      size: 18,
                      color: _focused ? neu.accent : neu.textSecondary,
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
                    cursorColor: neu.accent,
                    style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                          fontSize: 15,
                          height: 1.4,
                          color: widget.enabled
                              ? neu.textPrimary
                              : neu.textSecondary,
                        ),
                    decoration: InputDecoration(
                      isDense: true,
                      border: InputBorder.none,
                      hintText: widget.hint,
                      hintStyle:
                          Theme.of(context).textTheme.bodySmall?.copyWith(
                                fontSize: 14.5,
                                color: neu.textSecondary,
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
                  ?.copyWith(color: neu.danger),
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
    this.fillColor,
    this.showPercentage = true,
    this.semanticLabel,
  }) : assert(value >= 0.0 && value <= 1.0, 'value must be in [0.0, 1.0].');

  final double value;
  final double height;
  final Color? fillColor;
  final bool showPercentage;
  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    final duration = NeuMotion.resolveState(context);
    final curve = NeuMotion.resolveStateCurve(context);
    const radius = BorderRadius.all(Radius.circular(NeuRadius.pill));
    final percent = (value * 100).round();
    final effectiveFill = fillColor ?? neu.accent;
    final usePrimaryGradient = fillColor == null;

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
              color: usePrimaryGradient ? null : effectiveFill,
              gradient: usePrimaryGradient ? neu.primaryGradient : null,
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
                        color: neu.textPrimary,
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
                    color: neu.textPrimary,
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
                color: neu.hairlineBorder,
                width: 1.0,
              ),
            ),
            child: Row(
              children: <Widget>[
                if (prefixIcon != null) ...<Widget>[
                  Icon(prefixIcon, size: 18, color: neu.textSecondary),
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
                                  color: neu.textSecondary,
                                ),
                          ),
                        )
                      : DropdownButtonHideUnderline(
                          child: DropdownButton<T>(
                            value: effectiveValue,
                            isExpanded: true,
                            dropdownColor: neu.surface,
                            borderRadius: const BorderRadius.all(
                              Radius.circular(NeuRadius.control),
                            ),
                            icon: Icon(
                              Icons.keyboard_arrow_down_rounded,
                              color: canSelect
                                  ? neu.textPrimary
                                  : neu.textSecondary,
                            ),
                            hint: Text(
                              emptyHint,
                              style: Theme.of(context)
                                  .textTheme
                                  .bodyMedium
                                  ?.copyWith(
                                    fontSize: 14.5,
                                    color: neu.textSecondary,
                                  ),
                            ),
                            style: Theme.of(context)
                                .textTheme
                                .bodyMedium
                                ?.copyWith(
                                  fontSize: 14.5,
                                  color: neu.textPrimary,
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

/// 统一轻拟态圆角图标徽章。
class NeuIconBadge extends StatelessWidget {
  const NeuIconBadge({
    super.key,
    required this.icon,
    this.color,
    this.backgroundColor,
    this.size = 40,
    this.iconSize = 20,
    this.radius = NeuRadius.control,
  });

  final IconData icon;
  final Color? color;
  final Color? backgroundColor;
  final double size;
  final double iconSize;
  final double radius;

  @override
  Widget build(BuildContext context) {
    final neu = NeuTheme.of(context);
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        color: backgroundColor ?? neu.accentSoft,
        borderRadius: BorderRadius.all(Radius.circular(radius)),
      ),
      child: Icon(icon, size: iconSize, color: color ?? neu.accent),
    );
  }
}

/// 统一轻拟态弹窗外壳（标题 + 可选副标题 + 关闭按钮 + 内容区 + 底部操作区）。
class NeuDialog extends StatelessWidget {
  const NeuDialog({
    super.key,
    required this.title,
    required this.child,
    this.subtitle,
    this.maxWidth = 460,
    this.actions = const <Widget>[],
  });

  final String title;
  final String? subtitle;
  final Widget child;
  final double maxWidth;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final viewHeight = MediaQuery.sizeOf(context).height;
    final neu = NeuTheme.of(context);

    return Center(
      child: ConstrainedBox(
        constraints: BoxConstraints(
          maxWidth: maxWidth,
          maxHeight: viewHeight * 0.88,
        ),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 20),
          child: Material(
            color: neu.transparent,
            child: NeuCard(
              padding: const EdgeInsets.all(20),
              child: SingleChildScrollView(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: <Widget>[
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: <Widget>[
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: <Widget>[
                              Text(
                                title,
                                style: textTheme.titleMedium?.copyWith(
                                  fontWeight: FontWeight.w700,
                                ),
                              ),
                              if (subtitle != null && subtitle!.isNotEmpty) ...[
                                const SizedBox(height: 2),
                                Text(subtitle!, style: textTheme.bodySmall),
                              ],
                            ],
                          ),
                        ),
                        IconButton(
                          onPressed: () => Navigator.of(context).pop(),
                          tooltip: '关闭',
                          icon: Icon(
                            Icons.close_rounded,
                            size: 18,
                            color: neu.textSecondary,
                          ),
                        ),
                      ],
                    ),
                    const SizedBox(height: 14),
                    child,
                    if (actions.isNotEmpty) ...<Widget>[
                      const SizedBox(height: 18),
                      Wrap(
                        alignment: WrapAlignment.end,
                        spacing: 10,
                        runSpacing: 8,
                        children: actions,
                      ),
                    ],
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// 统一键值对展示行（桌面端水平对齐，窄屏自动折行不挤压）。
class NeuKeyValueRow extends StatelessWidget {
  const NeuKeyValueRow({
    super.key,
    required this.label,
    required this.value,
    this.leadingIcon,
    this.leadingColor,
    this.valueColor,
  });

  final String label;
  final String value;
  final IconData? leadingIcon;
  final Color? leadingColor;
  final Color? valueColor;

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    final neu = NeuTheme.of(context);
    final effectiveLeadingColor = leadingColor ?? neu.healthy;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: LayoutBuilder(
        builder: (context, constraints) {
          final narrow = constraints.maxWidth < 340;
          final labelWidget = Row(
            mainAxisSize: MainAxisSize.min,
            children: <Widget>[
              if (leadingIcon != null) ...<Widget>[
                Icon(leadingIcon, size: 17, color: effectiveLeadingColor),
                const SizedBox(width: 8),
              ],
              Flexible(
                child: Text(
                  label,
                  style: textTheme.bodyMedium?.copyWith(
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
            ],
          );
          final valueWidget = Text(
            value,
            style: textTheme.bodySmall?.copyWith(
              color: valueColor ?? neu.textSecondary,
            ),
            textAlign: narrow ? TextAlign.left : TextAlign.right,
          );

          if (narrow) {
            return Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                labelWidget,
                const SizedBox(height: 4),
                valueWidget,
              ],
            );
          }
          return Row(
            children: <Widget>[
              Expanded(child: labelWidget),
              const SizedBox(width: 12),
              Flexible(child: valueWidget),
            ],
          );
        },
      ),
    );
  }
}

/// 统一轻拟态空状态提示块。
class NeuEmptyState extends StatelessWidget {
  const NeuEmptyState({
    super.key,
    required this.icon,
    required this.title,
    required this.subtitle,
    this.action,
  });

  final IconData icon;
  final String title;
  final String subtitle;
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    final textTheme = Theme.of(context).textTheme;
    return NeuCard(
      inset: true,
      padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 24),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: <Widget>[
          NeuIconBadge(
            icon: icon,
            size: 46,
            iconSize: 22,
            radius: NeuRadius.pill,
          ),
          const SizedBox(height: 12),
          Text(
            title,
            style: textTheme.titleSmall,
            textAlign: TextAlign.center,
          ),
          const SizedBox(height: 4),
          Text(
            subtitle,
            style: textTheme.bodySmall,
            textAlign: TextAlign.center,
          ),
          if (action != null) ...<Widget>[
            const SizedBox(height: 14),
            action!,
          ],
        ],
      ),
    );
  }
}
