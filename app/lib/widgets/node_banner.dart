import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../screens/node_summary_screen.dart';

/// A strip above Home when your node can't be reached, or reports a problem
/// (ADR 0006: the phone shows prominently whether the node is reachable).
/// The node's own words for a problem can name its mechanisms, so they're
/// under Advanced details, a tap away through the node's summary.
class NodeBanner extends StatelessWidget {
  const NodeBanner({super.key, required this.node, required this.open});

  final HomeNode node;
  final void Function(Widget screen) open;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final details = TextButton(
      onPressed: () => open(NodeSummaryScreen(node: node, open: open)),
      child: const Text('Details'),
    );
    switch (node.reachability) {
      case Reachability.connecting:
        return _strip(
          context,
          color: scheme.surfaceContainerHighest,
          icon: Icons.sync,
          text: 'Connecting to your node…',
        );
      case Reachability.offline:
        final saved = node.savedAt != null
            ? ' Showing posts saved at ${formatClock(node.savedAt!)}.'
            : '';
        return _strip(
          context,
          color: scheme.errorContainer,
          icon: Icons.cloud_off,
          text: "Can't reach your node.$saved",
          actions: [
            details,
            TextButton(onPressed: node.refresh, child: const Text('Retry')),
          ],
        );
      case Reachability.online:
        final problems = node.status?.problems ?? const [];
        if (problems.isEmpty) return const SizedBox.shrink();
        return _strip(
          context,
          color: scheme.tertiaryContainer,
          icon: Icons.warning_amber,
          text: problems.length == 1
              ? 'Your node reports a problem.'
              : 'Your node reports ${problems.length} problems.',
          actions: [details],
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
    List<Widget> actions = const [],
  }) {
    return Material(
      color: color,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 8, 8, 8),
        child: Row(
          children: [
            Icon(icon, size: 20),
            const SizedBox(width: 12),
            Expanded(child: Text(text)),
            ...actions,
          ],
        ),
      ),
    );
  }
}
