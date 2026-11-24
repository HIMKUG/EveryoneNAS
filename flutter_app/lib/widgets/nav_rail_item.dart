import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../theme/neu_theme.dart';

/// Shared navigation item used by both the primary rail and nested settings rail.
class NavRailItem extends StatefulWidget {
  const NavRailItem({
    super.key,
    required this.label,
    required this.icon,
    required this.selectedIcon,
    required this.selected,
    required this.onTap,
    this.paintSelectedSurface = true,
  });

  final String label;
  final IconData icon;
  final IconData selectedIcon;
  final bool selected;
  final VoidCallback onTap;
  final bool paintSelectedSurface;

  @override
  State<NavRailItem> createState() => _NavRailItemState();
}

class _NavRailItemState extends State<NavRailItem> {
  final FocusNode _focusNode = FocusNode(debugLabel: 'NavRailItem');
  bool _focused = false;
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
    final foreground = widget.selected
        ? Colors.white
        : (_hovered ? NeuTokens.textPrimary : NeuTokens.textSecondary);
    final pressDuration = NeuMotion.resolvePress(context);
    final pressCurve = NeuMotion.resolvePressCurve(context);
    final reduced = NeuMotion.isReduced(context);
    final targetScale = (_pressed && !reduced) ? NeuMotion.pressedScale : 1.0;
    final showSelection = widget.selected && widget.paintSelectedSurface;

    return RepaintBoundary(
      child: Semantics(
        button: true,
        selected: widget.selected,
        label: widget.label,
        child: Focus(
          focusNode: _focusNode,
          onKeyEvent: _handleKey,
          onFocusChange: (focused) => setState(() => _focused = focused),
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
                child: Container(
                  height: 44,
                  padding: const EdgeInsets.symmetric(horizontal: 13),
                  decoration: BoxDecoration(
                    color: (!widget.selected && _hovered)
                        ? NeuTokens.insetSurface
                        : null,
                    gradient: showSelection ? NeuTokens.primaryGradient : null,
                    borderRadius: const BorderRadius.all(Radius.circular(12)),
                    border: Border.all(
                      color: _focused
                          ? (widget.selected ? Colors.white : NeuTokens.accent)
                          : Colors.transparent,
                    ),
                    boxShadow:
                        showSelection ? NeuTokens.primaryButtonShadows : null,
                  ),
                  child: Row(
                    children: <Widget>[
                      Icon(
                        widget.selected ? widget.selectedIcon : widget.icon,
                        size: 19,
                        color: foreground,
                      ),
                      const SizedBox(width: 11),
                      Expanded(
                        child: Text(
                          widget.label,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style:
                              Theme.of(context).textTheme.labelLarge?.copyWith(
                                    color: foreground,
                                    fontWeight: widget.selected
                                        ? FontWeight.w600
                                        : FontWeight.w500,
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
