import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:material_ui/material_ui.dart';

import 'neu_theme.dart';

class ThemeRevealHost extends StatefulWidget {
  const ThemeRevealHost({
    required this.child,
    this.boundaryKey,
    super.key,
  });

  final Widget child;
  final GlobalKey? boundaryKey;

  static ThemeRevealHostState of(BuildContext context) {
    final state = context.findAncestorStateOfType<ThemeRevealHostState>();
    assert(state != null, 'ThemeRevealHost is missing above this context.');
    return state!;
  }

  static ThemeRevealHostState? maybeOf(BuildContext context) =>
      context.findAncestorStateOfType<ThemeRevealHostState>();

  @override
  ThemeRevealHostState createState() => ThemeRevealHostState();
}

class ThemeRevealHostState extends State<ThemeRevealHost>
    with SingleTickerProviderStateMixin {
  final GlobalKey _boundaryKey = GlobalKey();
  final OverlayPortalController _portal = OverlayPortalController();
  late final AnimationController _controller;
  Animation<double>? _radius;
  ui.Image? _oldImage;
  Offset _origin = Offset.zero;
  double _maxRadius = 0;
  bool _capturing = false;
  int _switchToken = 0;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(vsync: this);
    _controller.addStatusListener(_handleStatus);
  }

  Future<void> reveal(
    BuildContext triggerContext,
    VoidCallback applyChange, {
    Offset? globalOrigin,
  }) async {
    final token = ++_switchToken;
    if (NeuMotion.isReduced(triggerContext)) {
      _cancelReveal();
      if (!mounted) return;
      applyChange();
      return;
    }

    RenderRepaintBoundary? boundary = (widget.boundaryKey ?? _boundaryKey)
        .currentContext
        ?.findRenderObject() as RenderRepaintBoundary?;
    if (boundary is! RenderRepaintBoundary) {
      if (!mounted) return;
      applyChange();
      return;
    }

    final resolvedOrigin = globalOrigin ?? _resolveOrigin(triggerContext);
    final mediaQuery = MediaQuery.maybeOf(triggerContext);
    final screenSize = mediaQuery?.size ?? Size.zero;
    final localOrigin = resolvedOrigin == Offset.zero
        ? boundary.size.center(Offset.zero)
        : boundary.globalToLocal(resolvedOrigin);
    final maxRadius = screenSize == Size.zero
        ? math.max(boundary.size.width, boundary.size.height)
        : math.sqrt(
            math.max(resolvedOrigin.dx, screenSize.width - resolvedOrigin.dx) *
                    math.max(resolvedOrigin.dx,
                        screenSize.width - resolvedOrigin.dx) +
                math.max(resolvedOrigin.dy,
                        screenSize.height - resolvedOrigin.dy) *
                    math.max(
                      resolvedOrigin.dy,
                      screenSize.height - resolvedOrigin.dy,
                    ),
          );

    if (_capturing) {
      while (_capturing && mounted && token == _switchToken) {
        await Future<void>.delayed(const Duration(milliseconds: 16));
      }
      if (!mounted || token != _switchToken) return;
      boundary = (widget.boundaryKey ?? _boundaryKey)
          .currentContext
          ?.findRenderObject() as RenderRepaintBoundary?;
      if (boundary is! RenderRepaintBoundary) return;
    }

    _capturing = true;
    ui.Image? captured;
    try {
      captured = await boundary.toImage(
        pixelRatio: mediaQuery?.devicePixelRatio ?? 1,
      );
    } on Object {
      captured = null;
    } finally {
      _capturing = false;
    }
    if (!mounted || token != _switchToken) {
      captured?.dispose();
      return;
    }

    if (NeuMotion.isReduced(context)) {
      captured?.dispose();
      applyChange();
      return;
    }

    _cancelReveal();
    _oldImage = captured;
    _origin = localOrigin;
    _maxRadius = math.max(1, maxRadius);
    final duration = NeuMotion.resolveReveal(context);
    _controller.duration =
        duration <= Duration.zero ? const Duration(milliseconds: 1) : duration;
    _radius = Tween<double>(begin: 0, end: _maxRadius).animate(
      CurvedAnimation(
        parent: _controller,
        curve: NeuMotion.resolveRevealCurve(context),
      ),
    );
    try {
      applyChange();
    } on Object {
      _cancelReveal();
      rethrow;
    }
    _portal.show();
    _controller.forward(from: 0);
  }

  Offset _resolveOrigin(BuildContext context) {
    final renderObject = context.findRenderObject();
    if (renderObject is RenderBox && renderObject.hasSize) {
      return renderObject.localToGlobal(renderObject.size.center(Offset.zero));
    }
    final mediaQuery = MediaQuery.maybeOf(context);
    if (mediaQuery != null) {
      return mediaQuery.size.center(Offset.zero);
    }
    return Offset.zero;
  }

  void _handleStatus(AnimationStatus status) {
    if (status == AnimationStatus.completed ||
        status == AnimationStatus.dismissed) {
      _cancelReveal();
    }
  }

  void _cancelReveal() {
    _controller.stop();
    _radius = null;
    if (_portal.isShowing) _portal.hide();
    _oldImage?.dispose();
    _oldImage = null;
  }

  void skip() {
    if (_oldImage != null) _cancelReveal();
  }

  bool _handlePointer(PointerDownEvent event) {
    skip();
    return false;
  }

  bool _handleScroll(PointerScrollEvent event) {
    skip();
    return false;
  }

  @override
  void dispose() {
    _switchToken++;
    _controller.removeStatusListener(_handleStatus);
    _controller.dispose();
    _oldImage?.dispose();
    _oldImage = null;
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return RepaintBoundary(
      key: widget.boundaryKey ?? _boundaryKey,
      child: OverlayPortal(
        controller: _portal,
        overlayChildBuilder: (context) => _buildOverlay(context),
        child: Listener(
          behavior: HitTestBehavior.translucent,
          onPointerDown: _handlePointer,
          onPointerSignal: (event) {
            if (event is PointerScrollEvent) _handleScroll(event);
          },
          child: widget.child,
        ),
      ),
    );
  }

  Widget _buildOverlay(BuildContext context) {
    return Positioned.fill(
      child: IgnorePointer(
        child: RepaintBoundary(
          child: AnimatedBuilder(
            animation: _controller,
            builder: (context, child) => CustomPaint(
              painter: _ThemeRevealPainter(
                image: _oldImage,
                radius: _radius?.value ?? 0,
                origin: _origin,
                maxRadius: _maxRadius,
              ),
              size: Size.infinite,
            ),
          ),
        ),
      ),
    );
  }
}

class _ThemeRevealPainter extends CustomPainter {
  _ThemeRevealPainter({
    required this.image,
    required this.radius,
    required this.origin,
    required this.maxRadius,
  });

  final ui.Image? image;
  final double radius;
  final Offset origin;
  final double maxRadius;

  @override
  void paint(Canvas canvas, Size size) {
    final snapshot = image;
    if (snapshot == null) return;
    canvas.save();
    canvas.clipRect(Offset.zero & size);
    if (radius >= maxRadius) {
      canvas.restore();
      return;
    }

    final fullRect = Path()..addRect(Offset.zero & size);
    final circle = Path()
      ..addOval(Rect.fromCircle(center: origin, radius: radius));
    final mask = Path.combine(
      PathOperation.difference,
      fullRect,
      circle,
    );
    canvas.clipPath(mask);
    canvas.drawImageRect(
      snapshot,
      Offset.zero & Size(snapshot.width.toDouble(), snapshot.height.toDouble()),
      Offset.zero & size,
      Paint()..filterQuality = FilterQuality.none,
    );
    canvas.restore();
  }

  @override
  bool shouldRepaint(_ThemeRevealPainter oldDelegate) =>
      oldDelegate.image != image ||
      oldDelegate.radius != radius ||
      oldDelegate.origin != origin ||
      oldDelegate.maxRadius != maxRadius;
}
