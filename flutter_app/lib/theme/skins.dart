import 'dart:math' as math;

import 'package:flutter/foundation.dart' show listEquals;
import 'package:material_ui/material_ui.dart';

enum SkinFamily { defaultSkin, mono, duo }

enum SkinGroupId {
  base,
  inkGray,
  blue,
  green,
  warmWhite,
  pinkViolet,
  redOrange,
  yellow,
  duoContrast,
  duoPremium,
  duoComfort,
}

const String defaultSkinId = 'skin_default';

/// Mixes `first` toward `second` by `amount` in 0..1.
///
/// `Color` exposes 0..1 doubles while `Color.fromARGB` takes 0..255 ints, so
/// every channel is scaled on the way out. Negative amounts used to be the
/// light palettes' way of asking for a darker tint; callers now pass an
/// explicit darker target instead (see `_darken`), because a clamped negative
/// silently returns `first` and collapses the whole light hierarchy.
Color mixColor(Color first, Color second, double amount) {
  final ratio = amount.clamp(0.0, 1.0);
  int channel(double from, double to) =>
      ((from + (to - from) * ratio) * 255).round().clamp(0, 255);

  return Color.fromARGB(
    channel(first.a, second.a),
    channel(first.r, second.r),
    channel(first.g, second.g),
    channel(first.b, second.b),
  );
}

/// Darkens [color] by [amount] (0..1) toward black, preserving alpha.
Color _darken(Color color, double amount) =>
    mixColor(color, const Color(0xFF000000), amount);

Color _composite(Color foreground, Color background) {
  final alpha = foreground.a.clamp(0.0, 1.0);
  int channel(double fg, double bg) =>
      ((fg * alpha + bg * (1 - alpha)) * 255).round().clamp(0, 255);
  return Color.fromARGB(
    255,
    channel(foreground.r, background.r),
    channel(foreground.g, background.g),
    channel(foreground.b, background.b),
  );
}

double _linearChannel(double value) {
  return value <= 0.04045
      ? value / 12.92
      : math.pow((value + 0.055) / 1.055, 2.4).toDouble();
}

double relativeLuminance(Color color) {
  final rgb = color.toARGB32();
  final red = _linearChannel(((rgb >> 16) & 0xFF) / 255);
  final green = _linearChannel(((rgb >> 8) & 0xFF) / 255);
  final blue = _linearChannel((rgb & 0xFF) / 255);
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
}

bool isDarkColor(Color color) => relativeLuminance(color) < 0.32;

double contrastRatio(Color first, Color second) {
  final firstLuminance = relativeLuminance(first);
  final secondLuminance = relativeLuminance(second);
  final lighter =
      firstLuminance > secondLuminance ? firstLuminance : secondLuminance;
  final darker =
      firstLuminance < secondLuminance ? firstLuminance : secondLuminance;
  return (lighter + 0.05) / (darker + 0.05);
}

Color pickContrastInk(Color background) {
  const white = Color(0xFFFFFFFF);
  const ink = Color(0xFF10141B);
  return contrastRatio(white, background) >= contrastRatio(ink, background)
      ? white
      : ink;
}

/// Picks the label colour for [accent] and, when neither white nor near-black
/// clears 4.5:1 (mid-tone accents such as `#887B3F` peak around 4.4), nudges the
/// accent itself darker/lighter until the pair passes.
(Color, Color) readableAccentPair(Color accent, {double target = 4.5}) {
  var ink = pickContrastInk(accent);
  if (contrastRatio(ink, accent) >= target) return (accent, ink);
  final towards =
      relativeLuminance(accent) > 0.5 ? const Color(0xFF000000) : const Color(0xFFFFFFFF);
  var candidate = accent;
  for (var step = 1; step <= 24; step++) {
    candidate = mixColor(accent, towards, step / 48);
    final nextInk = pickContrastInk(candidate);
    if (contrastRatio(nextInk, candidate) >= target) return (candidate, nextInk);
  }
  return (candidate, pickContrastInk(candidate));
}

Color ensureVisibleLine(Color canvas, Color surface, Color line) {
  final cardSurface = _composite(surface, canvas);
  if (contrastRatio(line, cardSurface) >= 1.6) return line;
  final towards =
      isDarkColor(canvas) ? const Color(0xFFFFFFFF) : const Color(0xFF000000);
  var candidate = line;
  for (var step = 1; step <= 24; step++) {
    candidate = mixColor(line, towards, step / 24);
    if (contrastRatio(candidate, cardSurface) >= 1.6) return candidate;
  }
  return candidate;
}

Color ensureReadableAccent(Color source, Color canvas, bool dark) {
  final target = dark ? 3.0 : 3.8;
  final towards = dark ? const Color(0xFFFFFFFF) : const Color(0xFF000000);
  var candidate = source;
  for (var step = 0; step <= 24; step++) {
    candidate = mixColor(source, towards, step / 24 * 0.8);
    if (contrastRatio(candidate, canvas) >= target) return candidate;
  }
  return candidate;
}

/// Pushes [source] toward black or white until it clears [target] against
/// [background]. Used for ink tokens (`textPrimary`/`textSecondary`/
/// `slateMuted`) that previously bypassed every guard.
Color _ensureReadableInk(Color source, Color background, {required double target}) {
  if (contrastRatio(source, background) >= target) return source;
  final towards =
      relativeLuminance(background) > 0.32
          ? const Color(0xFF000000)
          : const Color(0xFFFFFFFF);
  var candidate = source;
  for (var step = 1; step <= 24; step++) {
    candidate = mixColor(source, towards, step / 24);
    if (contrastRatio(candidate, background) >= target) return candidate;
  }
  return candidate;
}

Color _ensureInkContrast(Color source, Color ink) {
  final towards = relativeLuminance(ink) > 0.32
      ? const Color(0xFFFFFFFF)
      : const Color(0xFF000000);
  var candidate = source;
  for (var step = 0; step <= 24; step++) {
    candidate = mixColor(source, towards, step / 24 * 0.8);
    if (contrastRatio(candidate, ink) >= 4.58) return candidate;
  }
  return candidate;
}

List<BoxShadow> _derivedShadows(Color ink, bool dark) {
  return <BoxShadow>[
    BoxShadow(
      color: ink.withValues(alpha: dark ? 0.28 : 0.08),
      offset: const Offset(0, 6),
      blurRadius: 20,
      spreadRadius: -2,
    ),
    BoxShadow(
      color: ink.withValues(alpha: dark ? 0.20 : 0.06),
      offset: const Offset(0, 2),
      blurRadius: 6,
    ),
  ];
}

@immutable
class SkinPalette {
  const SkinPalette({
    required this.canvas,
    required this.surface,
    required this.insetSurface,
    required this.hoverSurface,
    required this.chromeSurface,
    required this.surfacePressed,
    required this.hairlineBorder,
    required this.controlOutline,
    required this.dividerSubtle,
    required this.textPrimary,
    required this.textSecondary,
    required this.slateMuted,
    required this.accent,
    required this.accentLight,
    required this.accentPressed,
    required this.accentSoft,
    required this.onAccent,
    required this.healthy,
    required this.healthyAlt,
    required this.healthyButton,
    required this.healthySoft,
    required this.folderGold,
    required this.folderGoldSoft,
    required this.warning,
    required this.danger,
    required this.dangerSoft,
    required this.disabledDarkSurface,
    required this.disabledDarkBorder,
    required this.disabledDarkForeground,
    required this.chartSky,
    required this.chartIndigo,
    required this.chartIndigoSoft,
    required this.timelineInactive,
    required this.timelineHalo,
    required this.timelineLine,
    required this.dialogBarrier,
    required this.switchTrackDisabled,
    required this.switchTrackOff,
    required this.extrudedShadows,
    required this.controlShadows,
    required this.primaryButtonShadows,
    required this.pressedShadows,
    required this.transparent,
  });

  final Color canvas;
  final Color surface;
  final Color insetSurface;
  final Color hoverSurface;
  final Color chromeSurface;
  final Color surfacePressed;
  final Color hairlineBorder;
  final Color controlOutline;
  final Color dividerSubtle;
  final Color textPrimary;
  final Color textSecondary;
  final Color slateMuted;
  final Color accent;
  final Color accentLight;
  final Color accentPressed;
  final Color accentSoft;
  final Color onAccent;
  final Color healthy;
  final Color healthyAlt;
  final Color healthyButton;
  final Color healthySoft;
  final Color folderGold;
  final Color folderGoldSoft;
  final Color warning;
  final Color danger;
  final Color dangerSoft;
  final Color disabledDarkSurface;
  final Color disabledDarkBorder;
  final Color disabledDarkForeground;
  final Color chartSky;
  final Color chartIndigo;
  final Color chartIndigoSoft;
  final Color timelineInactive;
  final Color timelineHalo;
  final Color timelineLine;
  final Color dialogBarrier;
  final Color switchTrackDisabled;
  final Color switchTrackOff;
  final List<BoxShadow> extrudedShadows;
  final List<BoxShadow> controlShadows;
  final List<BoxShadow> primaryButtonShadows;
  final List<BoxShadow> pressedShadows;
  final Color transparent;

  SkinPalette copyWith({
    Color? canvas,
    Color? surface,
    Color? insetSurface,
    Color? hoverSurface,
    Color? chromeSurface,
    Color? surfacePressed,
    Color? hairlineBorder,
    Color? controlOutline,
    Color? dividerSubtle,
    Color? textPrimary,
    Color? textSecondary,
    Color? slateMuted,
    Color? accent,
    Color? accentLight,
    Color? accentPressed,
    Color? accentSoft,
    Color? onAccent,
    Color? healthy,
    Color? healthyAlt,
    Color? healthyButton,
    Color? healthySoft,
    Color? folderGold,
    Color? folderGoldSoft,
    Color? warning,
    Color? danger,
    Color? dangerSoft,
    Color? disabledDarkSurface,
    Color? disabledDarkBorder,
    Color? disabledDarkForeground,
    Color? chartSky,
    Color? chartIndigo,
    Color? chartIndigoSoft,
    Color? timelineInactive,
    Color? timelineHalo,
    Color? timelineLine,
    Color? dialogBarrier,
    Color? switchTrackDisabled,
    Color? switchTrackOff,
    List<BoxShadow>? extrudedShadows,
    List<BoxShadow>? controlShadows,
    List<BoxShadow>? primaryButtonShadows,
    List<BoxShadow>? pressedShadows,
    Color? transparent,
  }) {
    return SkinPalette(
      canvas: canvas ?? this.canvas,
      surface: surface ?? this.surface,
      insetSurface: insetSurface ?? this.insetSurface,
      hoverSurface: hoverSurface ?? this.hoverSurface,
      chromeSurface: chromeSurface ?? this.chromeSurface,
      surfacePressed: surfacePressed ?? this.surfacePressed,
      hairlineBorder: hairlineBorder ?? this.hairlineBorder,
      controlOutline: controlOutline ?? this.controlOutline,
      dividerSubtle: dividerSubtle ?? this.dividerSubtle,
      textPrimary: textPrimary ?? this.textPrimary,
      textSecondary: textSecondary ?? this.textSecondary,
      slateMuted: slateMuted ?? this.slateMuted,
      accent: accent ?? this.accent,
      accentLight: accentLight ?? this.accentLight,
      accentPressed: accentPressed ?? this.accentPressed,
      accentSoft: accentSoft ?? this.accentSoft,
      onAccent: onAccent ?? this.onAccent,
      healthy: healthy ?? this.healthy,
      healthyAlt: healthyAlt ?? this.healthyAlt,
      healthyButton: healthyButton ?? this.healthyButton,
      healthySoft: healthySoft ?? this.healthySoft,
      folderGold: folderGold ?? this.folderGold,
      folderGoldSoft: folderGoldSoft ?? this.folderGoldSoft,
      warning: warning ?? this.warning,
      danger: danger ?? this.danger,
      dangerSoft: dangerSoft ?? this.dangerSoft,
      disabledDarkSurface: disabledDarkSurface ?? this.disabledDarkSurface,
      disabledDarkBorder: disabledDarkBorder ?? this.disabledDarkBorder,
      disabledDarkForeground:
          disabledDarkForeground ?? this.disabledDarkForeground,
      chartSky: chartSky ?? this.chartSky,
      chartIndigo: chartIndigo ?? this.chartIndigo,
      chartIndigoSoft: chartIndigoSoft ?? this.chartIndigoSoft,
      timelineInactive: timelineInactive ?? this.timelineInactive,
      timelineHalo: timelineHalo ?? this.timelineHalo,
      timelineLine: timelineLine ?? this.timelineLine,
      dialogBarrier: dialogBarrier ?? this.dialogBarrier,
      switchTrackDisabled: switchTrackDisabled ?? this.switchTrackDisabled,
      switchTrackOff: switchTrackOff ?? this.switchTrackOff,
      extrudedShadows: extrudedShadows ?? this.extrudedShadows,
      controlShadows: controlShadows ?? this.controlShadows,
      primaryButtonShadows: primaryButtonShadows ?? this.primaryButtonShadows,
      pressedShadows: pressedShadows ?? this.pressedShadows,
      transparent: transparent ?? this.transparent,
    );
  }

  SkinPalette lerp(SkinPalette other, double t) {
    Color color(Color first, Color second) => Color.lerp(first, second, t)!;
    List<BoxShadow> shadows(List<BoxShadow> first, List<BoxShadow> second) =>
        BoxShadow.lerpList(first, second, t)!;
    return SkinPalette(
      canvas: color(canvas, other.canvas),
      surface: color(surface, other.surface),
      insetSurface: color(insetSurface, other.insetSurface),
      hoverSurface: color(hoverSurface, other.hoverSurface),
      chromeSurface: color(chromeSurface, other.chromeSurface),
      surfacePressed: color(surfacePressed, other.surfacePressed),
      hairlineBorder: color(hairlineBorder, other.hairlineBorder),
      controlOutline: color(controlOutline, other.controlOutline),
      dividerSubtle: color(dividerSubtle, other.dividerSubtle),
      textPrimary: color(textPrimary, other.textPrimary),
      textSecondary: color(textSecondary, other.textSecondary),
      slateMuted: color(slateMuted, other.slateMuted),
      accent: color(accent, other.accent),
      accentLight: color(accentLight, other.accentLight),
      accentPressed: color(accentPressed, other.accentPressed),
      accentSoft: color(accentSoft, other.accentSoft),
      onAccent: color(onAccent, other.onAccent),
      healthy: color(healthy, other.healthy),
      healthyAlt: color(healthyAlt, other.healthyAlt),
      healthyButton: color(healthyButton, other.healthyButton),
      healthySoft: color(healthySoft, other.healthySoft),
      folderGold: color(folderGold, other.folderGold),
      folderGoldSoft: color(folderGoldSoft, other.folderGoldSoft),
      warning: color(warning, other.warning),
      danger: color(danger, other.danger),
      dangerSoft: color(dangerSoft, other.dangerSoft),
      disabledDarkSurface:
          color(disabledDarkSurface, other.disabledDarkSurface),
      disabledDarkBorder: color(disabledDarkBorder, other.disabledDarkBorder),
      disabledDarkForeground:
          color(disabledDarkForeground, other.disabledDarkForeground),
      chartSky: color(chartSky, other.chartSky),
      chartIndigo: color(chartIndigo, other.chartIndigo),
      chartIndigoSoft: color(chartIndigoSoft, other.chartIndigoSoft),
      timelineInactive: color(timelineInactive, other.timelineInactive),
      timelineHalo: color(timelineHalo, other.timelineHalo),
      timelineLine: color(timelineLine, other.timelineLine),
      dialogBarrier: color(dialogBarrier, other.dialogBarrier),
      switchTrackDisabled:
          color(switchTrackDisabled, other.switchTrackDisabled),
      switchTrackOff: color(switchTrackOff, other.switchTrackOff),
      extrudedShadows: shadows(extrudedShadows, other.extrudedShadows),
      controlShadows: shadows(controlShadows, other.controlShadows),
      primaryButtonShadows:
          shadows(primaryButtonShadows, other.primaryButtonShadows),
      pressedShadows: shadows(pressedShadows, other.pressedShadows),
      transparent: color(transparent, other.transparent),
    );
  }

  @override
  bool operator ==(Object other) {
    if (identical(this, other)) return true;
    return other is SkinPalette &&
        other.canvas == canvas &&
        other.surface == surface &&
        other.insetSurface == insetSurface &&
        other.hoverSurface == hoverSurface &&
        other.chromeSurface == chromeSurface &&
        other.surfacePressed == surfacePressed &&
        other.hairlineBorder == hairlineBorder &&
        other.controlOutline == controlOutline &&
        other.dividerSubtle == dividerSubtle &&
        other.textPrimary == textPrimary &&
        other.textSecondary == textSecondary &&
        other.slateMuted == slateMuted &&
        other.accent == accent &&
        other.accentLight == accentLight &&
        other.accentPressed == accentPressed &&
        other.accentSoft == accentSoft &&
        other.onAccent == onAccent &&
        other.healthy == healthy &&
        other.healthyAlt == healthyAlt &&
        other.healthyButton == healthyButton &&
        other.healthySoft == healthySoft &&
        other.folderGold == folderGold &&
        other.folderGoldSoft == folderGoldSoft &&
        other.warning == warning &&
        other.danger == danger &&
        other.dangerSoft == dangerSoft &&
        other.disabledDarkSurface == disabledDarkSurface &&
        other.disabledDarkBorder == disabledDarkBorder &&
        other.disabledDarkForeground == disabledDarkForeground &&
        other.chartSky == chartSky &&
        other.chartIndigo == chartIndigo &&
        other.chartIndigoSoft == chartIndigoSoft &&
        other.timelineInactive == timelineInactive &&
        other.timelineHalo == timelineHalo &&
        other.timelineLine == timelineLine &&
        other.dialogBarrier == dialogBarrier &&
        other.switchTrackDisabled == switchTrackDisabled &&
        other.switchTrackOff == switchTrackOff &&
        listEquals(other.extrudedShadows, extrudedShadows) &&
        listEquals(other.controlShadows, controlShadows) &&
        listEquals(other.primaryButtonShadows, primaryButtonShadows) &&
        listEquals(other.pressedShadows, pressedShadows) &&
        other.transparent == transparent;
  }

  @override
  int get hashCode => Object.hashAll(<Object>[
        canvas,
        surface,
        insetSurface,
        hoverSurface,
        chromeSurface,
        surfacePressed,
        hairlineBorder,
        controlOutline,
        dividerSubtle,
        textPrimary,
        textSecondary,
        slateMuted,
        accent,
        accentLight,
        accentPressed,
        accentSoft,
        onAccent,
        healthy,
        healthyAlt,
        healthyButton,
        healthySoft,
        folderGold,
        folderGoldSoft,
        warning,
        danger,
        dangerSoft,
        disabledDarkSurface,
        disabledDarkBorder,
        disabledDarkForeground,
        chartSky,
        chartIndigo,
        chartIndigoSoft,
        timelineInactive,
        timelineHalo,
        timelineLine,
        dialogBarrier,
        switchTrackDisabled,
        switchTrackOff,
        Object.hashAll(extrudedShadows),
        Object.hashAll(controlShadows),
        Object.hashAll(primaryButtonShadows),
        Object.hashAll(pressedShadows),
        transparent,
      ]);
}

SkinPalette buildDerivedPalette({
  required Color canvas,
  required Color accentSource,
  required Brightness brightness,
}) {
  final dark = brightness == Brightness.dark;
  const white = Color(0xFFFFFFFF);
  const black = Color(0xFF000000);
  final surface = mixColor(canvas, white, dark ? 0.07 : 0.55);
  final insetSurface = dark
      ? mixColor(canvas, white, 0.05)
      : _darken(canvas, 0.05);
  final hoverSurface = mixColor(canvas, white, dark ? 0.12 : 0.02);
  final surfacePressed = dark
      ? mixColor(canvas, white, 0.16)
      : _darken(canvas, 0.08);
  final chromeSurface = mixColor(canvas, white, dark ? 0.025 : 0.035);
  final textPrimary = dark
      ? mixColor(canvas, white, 0.92)
      : _darken(canvas, 0.88);
  // Secondary ink used to be a flat 52% blend back toward the canvas, which
  // lands below 4.5:1 on both palettes. It is guarded against the darkest
  // surface it is actually drawn on, not against the canvas.
  final textSecondary = _ensureReadableInk(
    mixColor(textPrimary, canvas, dark ? 0.34 : 0.46),
    surface,
    target: 4.5,
  );
  final slateMuted = _ensureReadableInk(
    mixColor(textPrimary, canvas, dark ? 0.55 : 0.30),
    surface,
    target: 3.0,
  );
  final line = ensureVisibleLine(
    canvas,
    surface,
    mixColor(canvas, textPrimary, dark ? 0.18 : 0.14),
  );
  final (accent, onAccent) = readableAccentPair(
    ensureReadableAccent(accentSource, canvas, dark),
  );
  final accentLight = _ensureInkContrast(
    ensureReadableAccent(mixColor(accent, onAccent, 0.12), canvas, dark),
    onAccent,
  );
  final accentPressed = _ensureInkContrast(
    ensureReadableAccent(mixColor(accent, textPrimary, 0.18), canvas, dark),
    onAccent,
  );
  final accentSoft =
      mixColor(accent, dark ? canvas : white, dark ? 0.82 : 0.84);
  final healthySource =
      dark ? const Color(0xFFA9BE9F) : const Color(0xFF56704F);
  final warningSource =
      dark ? const Color(0xFFE0B36A) : const Color(0xFFB07A2A);
  final dangerSource = dark ? const Color(0xFFF08A74) : const Color(0xFFA23F31);
  final healthy = ensureReadableAccent(healthySource, canvas, dark);
  final warning = ensureReadableAccent(warningSource, canvas, dark);
  final danger = ensureReadableAccent(dangerSource, canvas, dark);
  final shadows = _derivedShadows(textPrimary, dark);
  final disabledSurface = mixColor(canvas, textPrimary, dark ? 0.10 : 0.12);
  final chartIndigo = mixColor(accent, dark ? white : black, 0.12);

  return SkinPalette(
    canvas: canvas,
    surface: surface,
    insetSurface: insetSurface,
    hoverSurface: hoverSurface,
    chromeSurface: chromeSurface,
    surfacePressed: surfacePressed,
    hairlineBorder: line,
    controlOutline: ensureVisibleLine(
      canvas,
      surface,
      mixColor(line, textPrimary, 0.24),
    ),
    dividerSubtle: mixColor(line, canvas, 0.28),
    textPrimary: textPrimary,
    textSecondary: textSecondary,
    slateMuted: slateMuted,
    accent: accent,
    accentLight: accentLight,
    accentPressed: accentPressed,
    accentSoft: accentSoft,
    onAccent: onAccent,
    healthy: healthy,
    healthyAlt: healthy,
    healthyButton: _ensureInkContrast(
      ensureReadableAccent(mixColor(healthy, textPrimary, 0.20), canvas, dark),
      pickContrastInk(healthy),
    ),
    healthySoft: mixColor(healthy, dark ? canvas : white, 0.84),
    folderGold: warning,
    folderGoldSoft: mixColor(warning, dark ? canvas : white, 0.84),
    warning: warning,
    danger: danger,
    dangerSoft: mixColor(danger, dark ? canvas : white, 0.84),
    disabledDarkSurface: disabledSurface,
    disabledDarkBorder: mixColor(disabledSurface, textPrimary, 0.22),
    disabledDarkForeground: mixColor(textPrimary, canvas, 0.28),
    chartSky: mixColor(accent, white, 0.18),
    chartIndigo: chartIndigo,
    chartIndigoSoft: mixColor(chartIndigo, dark ? canvas : white, 0.84),
    timelineInactive: line,
    timelineHalo: accentSoft,
    timelineLine: mixColor(line, canvas, 0.28),
    dialogBarrier: black.withValues(alpha: dark ? 0.66 : 0.48),
    switchTrackDisabled: disabledSurface,
    switchTrackOff: insetSurface,
    extrudedShadows: shadows,
    controlShadows: shadows.sublist(1),
    primaryButtonShadows: <BoxShadow>[
      BoxShadow(
        color: accent.withValues(alpha: dark ? 0.32 : 0.24),
        offset: const Offset(0, 6),
        blurRadius: 14,
        spreadRadius: -2,
      ),
    ],
    pressedShadows: shadows.sublist(1),
    transparent: white.withValues(alpha: 0),
  );
}

({SkinPalette light, SkinPalette dark}) monoSkin(Color seed) {
  const white = Color(0xFFFFFFFF);
  const black = Color(0xFF000000);
  return (
    light: buildDerivedPalette(
      canvas: mixColor(seed, white, 0.84),
      accentSource: seed,
      brightness: Brightness.light,
    ),
    dark: buildDerivedPalette(
      canvas: mixColor(seed, black, 0.74),
      accentSource: seed,
      brightness: Brightness.dark,
    ),
  );
}

({SkinPalette light, SkinPalette dark}) duoSkin(Color first, Color second) {
  final firstLighter = relativeLuminance(first) >= relativeLuminance(second);
  final lighter = firstLighter ? first : second;
  final darker = firstLighter ? second : first;
  return (
    light: buildDerivedPalette(
      canvas: mixColor(lighter, const Color(0xFFFFFFFF), 0.5),
      accentSource: darker,
      brightness: Brightness.light,
    ),
    dark: buildDerivedPalette(
      canvas: mixColor(darker, const Color(0xFF000000), 0.62),
      accentSource: lighter,
      brightness: Brightness.dark,
    ),
  );
}

@immutable
class SkinDefinition {
  const SkinDefinition({
    required this.id,
    required this.name,
    required this.family,
    required this.swatches,
    required this.light,
    required this.dark,
  });

  final String id;
  final String name;
  final SkinFamily family;
  final List<Color> swatches;
  final SkinPalette light;
  final SkinPalette dark;

  SkinPalette paletteFor(Brightness brightness) =>
      brightness == Brightness.dark ? dark : light;
}

@immutable
class SkinGroup {
  const SkinGroup({
    required this.id,
    required this.label,
    required this.skins,
  });

  final SkinGroupId id;
  final String label;
  final List<SkinDefinition> skins;
}

const SkinPalette defaultLightPalette = SkinPalette(
  canvas: Color(0xFFF3F6FC),
  surface: Color(0xFFFFFFFF),
  insetSurface: Color(0xFFEEF3FA),
  hoverSurface: Color(0xFFF6F9FF),
  chromeSurface: Color(0xFFFAFBFD),
  surfacePressed: Color(0xFFE6EEF9),
  hairlineBorder: Color(0xFFE6ECF5),
  controlOutline: Color(0xFFCBD5E1),
  dividerSubtle: Color(0xFFE6EEF9),
  textPrimary: Color(0xFF0F172A),
  textSecondary: Color(0xFF64748B),
  slateMuted: Color(0xFF475569),
  accent: Color(0xFF2563EB),
  accentLight: Color(0xFF3B82F6),
  accentPressed: Color(0xFF1D4ED8),
  accentSoft: Color(0xFFEFF6FF),
  onAccent: Color(0xFFFFFFFF),
  healthy: Color(0xFF10B981),
  healthyAlt: Color(0xFF059669),
  healthyButton: Color(0xFF047857),
  healthySoft: Color(0xFFECFDF5),
  folderGold: Color(0xFFF59E0B),
  folderGoldSoft: Color(0xFFFEF3C7),
  warning: Color(0xFFD97706),
  danger: Color(0xFFEF4444),
  dangerSoft: Color(0xFFFEF2F2),
  disabledDarkSurface: Color(0xFF1E293B),
  disabledDarkBorder: Color(0xFF334155),
  disabledDarkForeground: Color(0xFFCBD5E1),
  chartSky: Color(0xFF38BDF8),
  chartIndigo: Color(0xFF6366F1),
  chartIndigoSoft: Color(0xFFEEF2FF),
  timelineInactive: Color(0xFFCBD5E1),
  timelineHalo: Color(0xFFBFDBFE),
  timelineLine: Color(0xFFD6E2F5),
  dialogBarrier: Color(0x520F172A),
  switchTrackDisabled: Color(0xFFE2E8F0),
  switchTrackOff: Color(0xFFD6DFEE),
  extrudedShadows: <BoxShadow>[
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
  ],
  controlShadows: <BoxShadow>[
    BoxShadow(
      color: Color(0x101E3A8A),
      offset: Offset(0, 3),
      blurRadius: 10,
      spreadRadius: -1,
    ),
  ],
  primaryButtonShadows: <BoxShadow>[
    BoxShadow(
      color: Color(0x332563EB),
      offset: Offset(0, 6),
      blurRadius: 14,
      spreadRadius: -2,
    ),
  ],
  pressedShadows: <BoxShadow>[
    BoxShadow(
      color: Color(0x0A1E3A8A),
      offset: Offset(0, 1),
      blurRadius: 3,
    ),
  ],
  transparent: Color(0x00000000),
);

const SkinPalette defaultDarkPalette = SkinPalette(
  canvas: Color(0xFF0F141E),
  surface: Color(0xFF182030),
  insetSurface: Color(0xFF202A3D),
  hoverSurface: Color(0xFF232F44),
  chromeSurface: Color(0xFF131926),
  surfacePressed: Color(0xFF2A3850),
  hairlineBorder: Color(0xFF34445F),
  controlOutline: Color(0xFF536785),
  dividerSubtle: Color(0xFF29364C),
  textPrimary: Color(0xFFECF2FA),
  textSecondary: Color(0xFFA6B5CB),
  slateMuted: Color(0xFF8799B3),
  accent: Color(0xFF60A5FA),
  accentLight: Color(0xFF7AB8FF),
  accentPressed: Color(0xFF4C8BF5),
  accentSoft: Color(0xFF1E2E4A),
  onAccent: Color(0xFF0F172A),
  healthy: Color(0xFF6EE7B7),
  healthyAlt: Color(0xFF5DD6A8),
  healthyButton: Color(0xFF34D399),
  healthySoft: Color(0xFF17382F),
  folderGold: Color(0xFFFBBF24),
  folderGoldSoft: Color(0xFF3D321C),
  warning: Color(0xFFFBBF24),
  danger: Color(0xFFFCA5A5),
  dangerSoft: Color(0xFF44262D),
  disabledDarkSurface: Color(0xFF273448),
  disabledDarkBorder: Color(0xFF40516A),
  disabledDarkForeground: Color(0xFFA6B5CB),
  chartSky: Color(0xFF67E8F9),
  chartIndigo: Color(0xFFA5B4FC),
  chartIndigoSoft: Color(0xFF2C3150),
  timelineInactive: Color(0xFF4B5D76),
  timelineHalo: Color(0xFF264365),
  timelineLine: Color(0xFF33445F),
  dialogBarrier: Color(0xB3060A12),
  switchTrackDisabled: Color(0xFF273448),
  switchTrackOff: Color(0xFF34445F),
  extrudedShadows: <BoxShadow>[
    BoxShadow(
      color: Color(0x66000000),
      offset: Offset(0, 8),
      blurRadius: 22,
      spreadRadius: -2,
    ),
    BoxShadow(
      color: Color(0x40000000),
      offset: Offset(0, 2),
      blurRadius: 6,
    ),
  ],
  controlShadows: <BoxShadow>[
    BoxShadow(
      color: Color(0x52000000),
      offset: Offset(0, 3),
      blurRadius: 10,
      spreadRadius: -1,
    ),
  ],
  primaryButtonShadows: <BoxShadow>[
    BoxShadow(
      color: Color(0x554C8BF5),
      offset: Offset(0, 6),
      blurRadius: 14,
      spreadRadius: -2,
    ),
  ],
  pressedShadows: <BoxShadow>[
    BoxShadow(
      color: Color(0x40000000),
      offset: Offset(0, 1),
      blurRadius: 3,
    ),
  ],
  transparent: Color(0x00000000),
);

SkinPalette _terracottaLight() {
  return buildDerivedPalette(
    canvas: const Color(0xFFF7F7F5),
    accentSource: const Color(0xFF262624),
    brightness: Brightness.light,
  ).copyWith(
    surface: const Color(0xFFFFFFFF),
    insetSurface: const Color(0xFFF0F0ED),
    surfacePressed: const Color(0xFFE7E7E3),
    hairlineBorder: const Color(0x1A111110),
    textPrimary: const Color(0xFF171716),
    textSecondary: const Color(0xFF6E6E69),
    slateMuted: const Color(0xFF7B7B76),
    accent: const Color(0xFF262624),
    accentPressed: const Color(0xFF111110),
    accentSoft: const Color(0xFFECECEA),
    onAccent: const Color(0xFFFFFFFF),
    healthy: const Color(0xFF56704F),
    healthyAlt: const Color(0xFF7E9A78),
    healthyButton: const Color(0xFF56704F),
    healthySoft: const Color(0xFFEDF1EA),
    warning: const Color(0xFFB07A2A),
    danger: const Color(0xFFA23F31),
    dangerSoft: const Color(0xFFF6E9E6),
    folderGold: const Color(0xFFB07A2A),
    folderGoldSoft: const Color(0xFFF5EEDC),
    chartSky: const Color(0xFF7E9A78),
    chartIndigo: const Color(0xFF56704F),
    chartIndigoSoft: const Color(0xFFEDF1EA),
    timelineInactive: const Color(0xFFD2D2CD),
    timelineHalo: const Color(0xFFE4E7DE),
    timelineLine: const Color(0xFFE0E0DB),
    extrudedShadows: <BoxShadow>[
      BoxShadow(
        color: const Color(0x0A111110),
        offset: const Offset(0, 1),
        blurRadius: 2,
      ),
      BoxShadow(
        color: const Color(0x0D111110),
        offset: const Offset(0, 8),
        blurRadius: 24,
      ),
    ],
    controlShadows: <BoxShadow>[
      BoxShadow(
        color: const Color(0x0A111110),
        offset: const Offset(0, 2),
        blurRadius: 6,
      ),
    ],
    primaryButtonShadows: <BoxShadow>[
      BoxShadow(
        color: const Color(0x1F111110),
        offset: const Offset(0, 6),
        blurRadius: 14,
        spreadRadius: -2,
      ),
    ],
    pressedShadows: <BoxShadow>[
      BoxShadow(
        color: const Color(0x0A111110),
        offset: const Offset(0, 1),
        blurRadius: 3,
      ),
    ],
  );
}

SkinPalette _terracottaDark() {
  return buildDerivedPalette(
    canvas: const Color(0xFF1C1A18),
    accentSource: const Color(0xFFE0A184),
    brightness: Brightness.dark,
  ).copyWith(
    surface: const Color(0xFF26231F),
    insetSurface: const Color(0xFF302C27),
    surfacePressed: const Color(0xFF3A3530),
    hairlineBorder: const Color(0x1AF3EEE7),
    textPrimary: const Color(0xFFF3EEE7),
    textSecondary: const Color(0xFFBDB5AA),
    slateMuted: const Color(0xFF948C81),
    accent: const Color(0xFFE0A184),
    accentLight: const Color(0xFFF0B99F),
    accentPressed: const Color(0xFFEAC0AB),
    accentSoft: const Color(0xFF3A2B25),
    onAccent: const Color(0xFF1C1A18),
    healthy: const Color(0xFFA9BE9F),
    healthyAlt: const Color(0xFF8FA887),
    healthyButton: const Color(0xFF8FA887),
    healthySoft: const Color(0xFF29332A),
    warning: const Color(0xFFE0B36A),
    danger: const Color(0xFFF08A74),
    dangerSoft: const Color(0xFF452A26),
    folderGold: const Color(0xFFE0B36A),
    folderGoldSoft: const Color(0xFF3D3226),
    chartSky: const Color(0xFF8FA887),
    chartIndigo: const Color(0xFFA9BE9F),
    chartIndigoSoft: const Color(0xFF29332A),
    timelineInactive: const Color(0xFF5D5750),
    timelineHalo: const Color(0xFF47342C),
    timelineLine: const Color(0xFF423C35),
    extrudedShadows: <BoxShadow>[
      BoxShadow(
        color: const Color(0x4D000000),
        offset: const Offset(0, 1),
        blurRadius: 2,
      ),
      BoxShadow(
        color: const Color(0x3D000000),
        offset: const Offset(0, 8),
        blurRadius: 24,
      ),
    ],
    controlShadows: <BoxShadow>[
      BoxShadow(
        color: const Color(0x40000000),
        offset: const Offset(0, 2),
        blurRadius: 6,
      ),
    ],
    primaryButtonShadows: <BoxShadow>[
      BoxShadow(
        color: const Color(0x4D000000),
        offset: const Offset(0, 6),
        blurRadius: 14,
        spreadRadius: -2,
      ),
    ],
    pressedShadows: <BoxShadow>[
      BoxShadow(
        color: const Color(0x40000000),
        offset: const Offset(0, 1),
        blurRadius: 3,
      ),
    ],
  );
}

final SkinDefinition defaultSkin = SkinDefinition(
  id: defaultSkinId,
  name: '柔冰蓝',
  family: SkinFamily.defaultSkin,
  swatches: const <Color>[Color(0xFF2563EB), Color(0xFF3B82F6)],
  light: defaultLightPalette,
  dark: defaultDarkPalette,
);

final SkinDefinition terracottaSkin = SkinDefinition(
  id: 'skin_terracotta',
  name: '赤陶纸',
  family: SkinFamily.defaultSkin,
  swatches: const <Color>[Color(0xFFF7F7F5), Color(0xFF1C1A18)],
  light: _terracottaLight(),
  dark: _terracottaDark(),
);

SkinDefinition _mono(String name, Color seed) {
  final palettes = monoSkin(seed);
  final argb = seed.toARGB32().toRadixString(16).padLeft(8, '0').toUpperCase();
  return SkinDefinition(
    id: 'skin_mono_$argb',
    name: name,
    family: SkinFamily.mono,
    swatches: <Color>[seed],
    light: palettes.light,
    dark: palettes.dark,
  );
}

SkinDefinition _duo(int index, String name, Color base, Color accent) {
  final palettes = duoSkin(base, accent);
  return SkinDefinition(
    id: 'skin_duo_${index.toString().padLeft(2, '0')}',
    name: name,
    family: SkinFamily.duo,
    swatches: <Color>[base, accent],
    light: palettes.light,
    dark: palettes.dark,
  );
}

SkinGroup _monoGroup(
        SkinGroupId id, String label, List<SkinDefinition> skins) =>
    SkinGroup(id: id, label: label, skins: skins);

SkinGroup _duoGroup(SkinGroupId id, String label, List<SkinDefinition> skins) =>
    SkinGroup(id: id, label: label, skins: skins);

final List<SkinGroup> baseSkinGroups = <SkinGroup>[
  SkinGroup(
    id: SkinGroupId.base,
    label: '基础',
    skins: <SkinDefinition>[defaultSkin, terracottaSkin],
  ),
];

final List<SkinGroup> monoSkinGroups = <SkinGroup>[
  _monoGroup(SkinGroupId.inkGray, '墨灰', <SkinDefinition>[
    _mono('玄墨', const Color(0xFF030100)),
    _mono('炭灰', const Color(0xFF0E0E0E)),
    _mono('黛玄', const Color(0xFF202425)),
    _mono('幽黛', const Color(0xFF2B313F)),
    _mono('沉黛', const Color(0xFF2B3C3D)),
    _mono('褐墨', const Color(0xFF373834)),
  ]),
  _monoGroup(SkinGroupId.blue, '蓝色', <SkinDefinition>[
    _mono('绀青', const Color(0xFF002FA7)),
    _mono('藏蓝', const Color(0xFF0C3967)),
    _mono('靛蓝', const Color(0xFF1660AB)),
    _mono('霁蓝', const Color(0xFF1781B5)),
    _mono('蔚穹', const Color(0xFF0081FF)),
    _mono('钴蓝', const Color(0xFF0947FE)),
    _mono('青碧', const Color(0xFF28CCC9)),
    _mono('水青', const Color(0xFF81D8CF)),
    _mono('松石', const Color(0xFF509296)),
    _mono('天青', const Color(0xFFC6E6E8)),
    _mono('冰蓝', const Color(0xFFC9F4FF)),
    _mono('灰蓝', const Color(0xFF606DA1)),
    _mono('烟蓝', const Color(0xFF707899)),
  ]),
  _monoGroup(SkinGroupId.green, '绿色', <SkinDefinition>[
    _mono('翡冷', const Color(0xFF01847F)),
    _mono('青玉', const Color(0xFF2C9678)),
    _mono('苔青', const Color(0xFF67AA66)),
    _mono('竹翠', const Color(0xFF67B971)),
    _mono('柳黄', const Color(0xFFAFCD53)),
    _mono('流翠', const Color(0xFF00FF00)),
    _mono('玉色', const Color(0xFFB8DED0)),
  ]),
  _monoGroup(SkinGroupId.warmWhite, '暖白', <SkinDefinition>[
    _mono('嫩菊', const Color(0xFFE2E7BF)),
    _mono('鹅黄', const Color(0xFFF9FFD3)),
    _mono('豆白', const Color(0xFFFAFBE6)),
    _mono('宣纸', const Color(0xFFF9F2E0)),
    _mono('霜白', const Color(0xFFFFFBF8)),
    _mono('云白', const Color(0xFFF5F4F7)),
    _mono('烟白', const Color(0xFFE3E3E5)),
    _mono('茶白', const Color(0xFFE6E3C5)),
    _mono('米霜', const Color(0xFFF6F9E4)),
    _mono('麦驼', const Color(0xFFEAD0B0)),
    _mono('深驼', const Color(0xFFDDCDB7)),
    _mono('檀棕', const Color(0xFFBA8677)),
  ]),
  _monoGroup(SkinGroupId.pinkViolet, '粉紫', <SkinDefinition>[
    _mono('丁香', const Color(0xFFA48CE6)),
    _mono('紫绀', const Color(0xFF652C97)),
    _mono('桃夭', const Color(0xFFEE819E)),
    _mono('樱粉', const Color(0xFFFD7B99)),
    _mono('脂粉', const Color(0xFFFFD8DD)),
    _mono('肤粉', const Color(0xFFF7ECE7)),
  ]),
  _monoGroup(SkinGroupId.redOrange, '红橙', <SkinDefinition>[
    _mono('朱颜', const Color(0xFFFF6067)),
    _mono('赤红', const Color(0xFFD70123)),
    _mono('枣红', const Color(0xFFA61B29)),
    _mono('胭脂', const Color(0xFF990033)),
    _mono('丹砂', const Color(0xFFEE3F4D)),
    _mono('晚橙', const Color(0xFFFB9966)),
    _mono('烈橙', const Color(0xFFFF7400)),
    _mono('杏脂', const Color(0xFFFFAA93)),
  ]),
  _monoGroup(SkinGroupId.yellow, '黄色', <SkinDefinition>[
    _mono('藤黄', const Color(0xFFFFC408)),
    _mono('鎏金', const Color(0xFFFFC32D)),
    _mono('明黄', const Color(0xFFFFCF14)),
    _mono('蜜黄', const Color(0xFFFFE677)),
    _mono('稚黄', const Color(0xFFFEF99D)),
  ]),
];

final List<SkinGroup> duoSkinGroups = <SkinGroup>[
  _duoGroup(SkinGroupId.duoContrast, '撞色', <SkinDefinition>[
    _duo(1, '竹影描霜', const Color(0xFF67B971), const Color(0xFFF6F9E4)),
    _duo(2, '赤火熔金', const Color(0xFFD70123), const Color(0xFFFFC32D)),
    _duo(3, '深海浮沙', const Color(0xFF0C3967), const Color(0xFFEAD0B0)),
    _duo(4, '寒夜星蓝', const Color(0xFF030100), const Color(0xFF0081FF)),
    _duo(5, '墨底桃花', const Color(0xFF0E0E0E), const Color(0xFFFD7B99)),
    _duo(6, '碧波沉黛', const Color(0xFF28CCC9), const Color(0xFF202425)),
  ]),
  _duoGroup(SkinGroupId.duoPremium, '高级', <SkinDefinition>[
    _duo(7, '松风煮茶', const Color(0xFFB8DED0), const Color(0xFFE6E3C5)),
    _duo(8, '金盏檀台', const Color(0xFFFFC408), const Color(0xFFBA8677)),
    _duo(9, '雾蓝晚橙', const Color(0xFF606DA1), const Color(0xFFFB9966)),
    _duo(10, '新柳绯樱', const Color(0xFFAFCD53), const Color(0xFFEE819E)),
    _duo(11, '松石映雪', const Color(0xFF509296), const Color(0xFFFFFBF8)),
  ]),
  _duoGroup(SkinGroupId.duoComfort, '舒适', <SkinDefinition>[
    _duo(12, '水天霁色', const Color(0xFFC9F4FF), const Color(0xFF1781B5)),
    _duo(13, '纸映鸢尾', const Color(0xFFF9F2E0), const Color(0xFF1660AB)),
    _duo(14, '浅豆丁香', const Color(0xFFFAFBE6), const Color(0xFFA48CE6)),
    _duo(15, '脂底胭红', const Color(0xFFF7ECE7), const Color(0xFFA61B29)),
    _duo(16, '嫩黄衔红', const Color(0xFFF9FFD3), const Color(0xFFEE3F4D)),
    _duo(17, '菊影幽窗', const Color(0xFFE2E7BF), const Color(0xFF2B313F)),
    _duo(18, '天青入墨', const Color(0xFFC6E6E8), const Color(0xFF373834)),
    _duo(19, '云间青玉', const Color(0xFFF5F4F7), const Color(0xFF2C9678)),
    _duo(20, '烟雨幽兰', const Color(0xFFE3E3E5), const Color(0xFF707899)),
  ]),
];

final List<SkinGroup> skinGroups = <SkinGroup>[
  ...baseSkinGroups,
  ...monoSkinGroups,
  ...duoSkinGroups,
];

final List<SkinDefinition> builtInSkins = <SkinDefinition>[
  for (final group in skinGroups) ...group.skins,
];

final Map<String, SkinDefinition> _skinsById = <String, SkinDefinition>{
  for (final skin in builtInSkins) skin.id: skin,
  'skin_white': defaultSkin,
  'skin_black': defaultSkin,
};

SkinDefinition? getSkinDefinition(String? id) => _skinsById[id];

String? canonicalSkinId(String? id) {
  if (id == 'skin_white' || id == 'skin_black') return defaultSkinId;
  return id;
}

bool isSkinId(String? id) => id != null && _skinsById.containsKey(id);

List<SkinDefinition> searchSkins(
  String query, {
  SkinFamily? family,
  SkinGroupId? groupId,
}) {
  final normalized = query.trim().toLowerCase();
  final groups = groupId == null
      ? skinGroups
      : skinGroups.where((group) => group.id == groupId);
  return <SkinDefinition>[
    for (final group in groups)
      for (final skin in group.skins)
        if ((family == null || skin.family == family) &&
            (normalized.isEmpty ||
                skin.name.toLowerCase().contains(normalized) ||
                skin.id.toLowerCase().contains(normalized) ||
                group.label.contains(normalized)))
          skin,
  ];
}
