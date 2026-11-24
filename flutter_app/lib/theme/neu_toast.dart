import 'dart:async';

import 'package:material_ui/material_ui.dart';

import 'neu_theme.dart';

enum NeuToastTone { info, success, warning, error }

/// 全局悬浮 Toast 提示系统：
/// 纯白玻璃质感卡片 + 柔和冷蓝阴影，使用 Overlay 悬浮于界面之上，绝不挤压页面布局。
abstract final class NeuToast {
  static OverlayEntry? _activeEntry;
  static Timer? _dismissTimer;

  static void info(BuildContext context, String message) {
    show(context, message, tone: NeuToastTone.info);
  }

  static void success(BuildContext context, String message) {
    show(context, message, tone: NeuToastTone.success);
  }

  static void warning(BuildContext context, String message) {
    show(context, message, tone: NeuToastTone.warning);
  }

  static void error(BuildContext context, String message) {
    show(context, message, tone: NeuToastTone.error);
  }

  static void show(
    BuildContext context,
    String message, {
    NeuToastTone tone = NeuToastTone.info,
    Duration duration = const Duration(milliseconds: 2800),
  }) {
    final overlay = Overlay.maybeOf(context, rootOverlay: true);
    if (overlay == null) return;

    dismiss();

    late final OverlayEntry entry;
    entry = OverlayEntry(
      builder: (overlayContext) => _NeuToastOverlay(
        message: message,
        tone: tone,
        onDismiss: () {
          if (_activeEntry == entry) {
            dismiss();
          }
        },
      ),
    );

    _activeEntry = entry;
    overlay.insert(entry);
    _dismissTimer = Timer(duration, () {
      if (_activeEntry == entry) {
        dismiss();
      }
    });
  }

  static void dismiss() {
    _dismissTimer?.cancel();
    _dismissTimer = null;
    if (_activeEntry?.mounted == true) {
      _activeEntry!.remove();
    }
    _activeEntry = null;
  }
}

class _NeuToastOverlay extends StatefulWidget {
  const _NeuToastOverlay({
    required this.message,
    required this.tone,
    required this.onDismiss,
  });

  final String message;
  final NeuToastTone tone;
  final VoidCallback onDismiss;

  @override
  State<_NeuToastOverlay> createState() => _NeuToastOverlayState();
}

class _NeuToastOverlayState extends State<_NeuToastOverlay>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;
  late final Animation<double> _opacity;
  late final Animation<Offset> _slide;
  late final Animation<double> _scale;
  late final CurvedAnimation _curved;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      vsync: this,
      duration: NeuMotion.pressDuration,
    );
    _curved = CurvedAnimation(
      parent: _controller,
      curve: NeuMotion.pressCurve,
    );
    _opacity = _curved;
    _slide = Tween<Offset>(
      begin: const Offset(0, -0.18),
      end: Offset.zero,
    ).animate(_curved);
    _scale = Tween<double>(begin: 0.94, end: 1.0).animate(_curved);
    _controller.forward();
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _controller.duration = NeuMotion.resolvePress(context);
    _curved.curve = NeuMotion.resolvePressCurve(context);
  }

  @override
  void dispose() {
    _curved.dispose();
    _controller.dispose();
    super.dispose();
  }

  (IconData, Color, Color) _resolveVisual() {
    return switch (widget.tone) {
      NeuToastTone.info => (
          Icons.info_outline_rounded,
          NeuTokens.accent,
          NeuTokens.accentSoft,
        ),
      NeuToastTone.success => (
          Icons.check_circle_rounded,
          NeuTokens.healthy,
          NeuTokens.healthySoft,
        ),
      NeuToastTone.warning => (
          Icons.warning_amber_rounded,
          NeuTokens.warning,
          NeuTokens.folderGoldSoft,
        ),
      NeuToastTone.error => (
          Icons.error_outline_rounded,
          NeuTokens.danger,
          NeuTokens.dangerSoft,
        ),
    };
  }

  @override
  Widget build(BuildContext context) {
    final isWide =
        MediaQuery.sizeOf(context).width >= NeuTokens.breakpointWide;
    final reduced = NeuMotion.isReduced(context);
    final (icon, accentColor, badgeBg) = _resolveVisual();

    final toastCard = Semantics(
      liveRegion: true,
      container: true,
      label: widget.message,
      child: GestureDetector(
        onTap: widget.onDismiss,
        child: Material(
          color: Colors.transparent,
          child: Container(
            constraints: const BoxConstraints(maxWidth: 420),
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 11),
            decoration: BoxDecoration(
              color: NeuTokens.surface,
              borderRadius: const BorderRadius.all(
                Radius.circular(NeuRadius.group),
              ),
              border: Border.all(
                color: NeuTokens.hairlineBorder,
                width: 1,
              ),
              boxShadow: const <BoxShadow>[
                BoxShadow(
                  color: Color(0x1A0F172A),
                  offset: Offset(0, 10),
                  blurRadius: 24,
                ),
                BoxShadow(
                  color: Color(0x122563EB),
                  offset: Offset(0, 2),
                  blurRadius: 8,
                ),
              ],
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: <Widget>[
                Container(
                  width: 28,
                  height: 28,
                  decoration: BoxDecoration(
                    color: badgeBg,
                    borderRadius: const BorderRadius.all(Radius.circular(8)),
                  ),
                  child: Icon(icon, size: 17, color: accentColor),
                ),
                const SizedBox(width: 10),
                Flexible(
                  child: Text(
                    widget.message,
                    style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                          color: NeuTokens.textPrimary,
                          fontWeight: FontWeight.w500,
                        ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );

    Widget animatedToast = toastCard;
    if (!reduced) {
      animatedToast = SlideTransition(
        position: _slide,
        child: ScaleTransition(
          scale: _scale,
          child: toastCard,
        ),
      );
    }

    return Positioned(
      top: isWide ? 20 : null,
      right: isWide ? 24 : 16,
      left: isWide ? null : 16,
      bottom: isWide ? null : 84,
      child: SafeArea(
        child: Align(
          alignment: isWide ? Alignment.topRight : Alignment.bottomCenter,
          child: FadeTransition(
            opacity: _opacity,
            child: animatedToast,
          ),
        ),
      ),
    );
  }
}
