import 'package:flutter/material.dart';

import '../theme.dart';

/// One action under a post: an icon and, when there is one, a count.
/// [label] is what a screen reader says and a long press shows, so the
/// icon needs no word beside it (ADR 0040 §3).
class PostAction extends StatelessWidget {
  const PostAction({
    super.key,
    required this.icon,
    required this.label,
    required this.onPressed,
    this.count,
    this.color,
  });

  final IconData icon;
  final String label;
  final VoidCallback? onPressed;

  /// Shown beside the icon when above zero.
  final int? count;

  /// The icon's colour when on (a like given); the quiet colour otherwise.
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final enabled = onPressed != null;
    final tone = !enabled
        ? scheme.onSurface.withValues(alpha: 0.38)
        : color ?? scheme.onSurfaceVariant;
    final n = count ?? 0;
    return Tooltip(
      message: label,
      excludeFromSemantics: true,
      child: Semantics(
        button: true,
        enabled: enabled,
        label: label,
        excludeSemantics: true,
        child: InkWell(
          onTap: onPressed,
          customBorder: const StadiumBorder(),
          child: ConstrainedBox(
            // Room for a finger, as Material asks.
            constraints: const BoxConstraints(minHeight: 48, minWidth: 48),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: Space.s + 4),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  AnimatedSwitcher(
                    duration: Motion.of(context, Motion.quick),
                    transitionBuilder: (child, a) =>
                        ScaleTransition(scale: a, child: child),
                    child: Icon(
                      icon,
                      key: ValueKey(icon),
                      size: 20,
                      color: tone,
                    ),
                  ),
                  if (n > 0) ...[
                    const SizedBox(width: Space.xs + 2),
                    Text(
                      '$n',
                      style: Theme.of(context).textTheme.labelLarge?.copyWith(
                        color: tone,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
