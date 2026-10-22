import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';

/// A strip above the feed when the home node can't be reached, or reports
/// a problem (ADR 0006: the phone shows prominently whether the node is
/// reachable).
class NodeBanner extends StatelessWidget {
  const NodeBanner({super.key, required this.node});

  final HomeNode node;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    switch (node.reachability) {
      case Reachability.connecting:
        return _strip(
          context,
          color: scheme.surfaceContainerHighest,
          icon: Icons.sync,
          text: 'Connecting to your home node…',
        );
      case Reachability.offline:
        final saved = node.savedAt != null
            ? ' Showing posts saved at ${formatClock(node.savedAt!)}.'
            : '';
        return _strip(
          context,
          color: scheme.errorContainer,
          icon: Icons.cloud_off,
          text: "Can't reach your home node.$saved",
          detail: node.lastError,
          action: TextButton(
            onPressed: node.refresh,
            child: const Text('Retry'),
          ),
        );
      case Reachability.online:
        final problems = node.status?.problems ?? const [];
        if (problems.isEmpty) return const SizedBox.shrink();
        return _strip(
          context,
          color: Colors.amber.withValues(alpha: 0.3),
          icon: Icons.warning_amber,
          text: 'Your home node reports: ${problems.join('; ')}',
        );
      case Reachability.unknown:
        return const SizedBox.shrink();
    }
  }

  Widget _strip(
    BuildContext context, {
    required Color color,
    required IconData icon,
    required String text,
    String? detail,
    Widget? action,
  }) {
    return Material(
      color: color,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 8, 8, 8),
        child: Row(
          children: [
            Icon(icon, size: 20),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(text),
                  if (detail != null)
                    Text(detail, style: Theme.of(context).textTheme.bodySmall),
                ],
              ),
            ),
            ?action,
          ],
        ),
      ),
    );
  }
}
