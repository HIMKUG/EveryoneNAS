import 'dart:async';

import 'package:material_ui/material_ui.dart';

import 'neu_theme.dart';

class SlideSwitchViewport extends StatefulWidget {
  const SlideSwitchViewport({
    required this.index,
    required this.children,
    this.axis = Axis.horizontal,
    this.keepAlive = true,
    super.key,
  }) : assert(index >= 0 && index < children.length);

  final int index;
  final List<Widget> children;
  final Axis axis;
  final bool keepAlive;

  @override
  State<SlideSwitchViewport> createState() => _SlideSwitchViewportState();
}

class _SlideSwitchViewportState extends State<SlideSwitchViewport>
    with SingleTickerProviderStateMixin {
  late int _index;
  late AnimationController _controller;
  double _direction = 1;
  double _startOffset = 0;
  double _startOpacity = 0;
  final Map<int, Widget> _mountedChildren = <int, Widget>{};

  @override
  void initState() {
    super.initState();
    _index = widget.index;
    _controller = AnimationController(vsync: this);
    _controller.addStatusListener(_handleStatus);
  }

  @override
  void didUpdateWidget(covariant SlideSwitchViewport oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.children, widget.children)) {
      _mountedChildren.clear();
    }
    if (_index == widget.index) return;

    final direction = widget.index > _index ? 1.0 : -1.0;
    final progress = Curves.easeOutCubic.transform(_controller.value);
    final currentOffset =
        _controller.isAnimating ? _startOffset * (1 - progress) : 0.0;
    final currentOpacity = _controller.isAnimating
        ? _startOpacity + (1 - _startOpacity) * progress
        : 0.0;
    _index = widget.index;
    _direction = direction;
    if (NeuMotion.isReduced(context)) {
      _controller.stop();
      _controller.value = 0;
      _startOffset = 0;
      _startOpacity = 0;
      return;
    }

    final startOffset =
        currentOffset.abs() < 0.5 ? 24 * direction : currentOffset;
    final startOpacity = currentOffset.abs() < 0.5 ? 0.0 : currentOpacity;
    _controller.stop();
    _controller.value = 0;
    _startOffset = startOffset;
    _startOpacity = startOpacity;
    setState(() {});
    _controller.forward(from: 0);
  }

  void _handleStatus(AnimationStatus status) {
    if (status != AnimationStatus.completed) return;
    _startOffset = 0;
    _startOpacity = 0;
    _controller.value = 0;
    if (mounted) setState(() {});
    unawaited(_synchronizeChildren());
  }

  Future<void> _synchronizeChildren() async {
    await WidgetsBinding.instance.endOfFrame;
    if (!mounted) return;
    final nextChildren = <int, Widget>{};
    for (var pageIndex = 0; pageIndex < widget.children.length; pageIndex++) {
      if (!widget.keepAlive && pageIndex != _index) continue;
      final child = widget.children[pageIndex];
      nextChildren[pageIndex] = identical(_mountedChildren[pageIndex], child)
          ? _mountedChildren[pageIndex]!
          : child;
    }
    if (!mounted) return;
    setState(() {
      _mountedChildren
        ..clear()
        ..addAll(nextChildren);
    });
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _controller.duration = NeuMotion.scaleDuration(
      NeuMotion.pageEnterDuration,
      NeuMotion.resolveSpeed(context),
      reduced: NeuMotion.isReduced(context),
    );
    unawaited(_synchronizeChildren());
  }

  @override
  void deactivate() {
    if (widget.keepAlive) _mountedChildren.clear();
    super.deactivate();
  }

  @override
  void dispose() {
    _controller.removeStatusListener(_handleStatus);
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (!widget.keepAlive) {
      _mountedChildren
        ..clear()
        ..[_index] = widget.children[_index];
    }
    return ClipRect(
      child: Stack(
        fit: StackFit.expand,
        children: <Widget>[
          for (final entry in _mountedChildren.entries)
            _buildMountedPage(entry),
        ],
      ),
    );
  }

  Widget _buildMountedPage(MapEntry<int, Widget> entry) {
    final reduced = NeuMotion.isReduced(context);
    final progress = Curves.easeOutCubic.transform(_controller.value);
    final opacity =
        reduced ? 1.0 : _startOpacity + (1 - _startOpacity) * progress;
    final offsetValue =
        reduced ? 0.0 : _startOffset * (1 - progress) * _direction;
    return Transform.translate(
      offset: entry.key == _index
          ? (widget.axis == Axis.horizontal
              ? Offset(offsetValue, 0)
              : Offset(0, offsetValue))
          : Offset.zero,
      child: Opacity(
        opacity: entry.key == _index ? opacity.clamp(0.0, 1.0) : 1.0,
        child: ExcludeSemantics(
          excluding: entry.key != _index,
          child: TickerMode(
            enabled: entry.key == _index || _controller.isAnimating,
            child: KeyedSubtree(
              key: ValueKey<int>(entry.key),
              child: entry.value,
            ),
          ),
        ),
      ),
    );
  }
}

class NeuSlidePageRoute<T> extends PageRoute<T> {
  NeuSlidePageRoute({
    required this.builder,
    super.settings,
    Color? modalBarrierColor,
    String? barrierLabel,
  })  : _barrierColor = modalBarrierColor,
        _barrierLabel = barrierLabel;

  final WidgetBuilder builder;
  @override
  bool get maintainState => true;

  @override
  bool get allowSnapshotting => true;

  Duration _transitionDuration = NeuMotion.reducedDuration;
  Duration _reverseTransitionDuration = NeuMotion.reducedDuration;
  final Color? _barrierColor;
  final String? _barrierLabel;

  @override
  Color? get barrierColor => _barrierColor;

  @override
  String? get barrierLabel => _barrierLabel;

  @override
  bool get opaque => true;

  @override
  bool get barrierDismissible => false;

  @override
  Duration get transitionDuration => _transitionDuration;

  @override
  Duration get reverseTransitionDuration => _reverseTransitionDuration;

  @override
  Widget buildPage(
    BuildContext context,
    Animation<double> animation,
    Animation<double> secondaryAnimation,
  ) {
    final content = builder(context);
    final reduced = NeuMotion.isReduced(context);
    final speed = NeuMotion.resolveSpeed(context);
    final duration = NeuMotion.scaleDuration(
      NeuMotion.pageEnterDuration,
      speed,
      reduced: reduced,
    );
    final reverseDuration = NeuMotion.scaleDuration(
      NeuMotion.pageExitDuration,
      speed,
      reduced: reduced,
    );
    _transitionDuration = duration;
    _reverseTransitionDuration = reverseDuration;
    final theme = Theme.of(context);
    return Semantics(
      scopesRoute: true,
      explicitChildNodes: true,
      child: AnimatedBuilder(
        animation: animation,
        builder: (context, child) => ClipRect(
          child: Stack(
            fit: StackFit.expand,
            children: <Widget>[
              const ColoredBox(color: Colors.black),
              SlideTransition(
                position: Tween<Offset>(
                  begin: const Offset(1, 0),
                  end: Offset.zero,
                ).animate(CurvedAnimation(
                  parent: animation,
                  curve: reduced ? Curves.linear : NeuMotion.appleSheetCurve,
                )),
                child: child,
              ),
            ],
          ),
        ),
        child: ColoredBox(
          color: theme.scaffoldBackgroundColor,
          child: content,
        ),
      ),
    );
  }

  @override
  Widget buildTransitions(
    BuildContext context,
    Animation<double> animation,
    Animation<double> secondaryAnimation,
    Widget child,
  ) {
    final reduced = NeuMotion.isReduced(context);
    if (reduced) return child;
    final speed = NeuMotion.resolveSpeed(context);
    return FadeTransition(
      opacity: CurvedAnimation(
        parent: secondaryAnimation,
        curve: Interval(
          0,
          NeuMotion.scaleDuration(
                NeuMotion.pageExitDuration,
                speed,
              ).inMicroseconds /
              NeuMotion.scaleDuration(
                NeuMotion.pageEnterDuration,
                speed,
              ).inMicroseconds.clamp(0, 1000000) /
              1000000,
          curve: NeuMotion.resolveExitCurve(context),
        ),
      ),
      child: child,
    );
  }
}
