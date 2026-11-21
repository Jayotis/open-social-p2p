import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../theme.dart';
import 'advanced_screen.dart';
import 'carrying_screen.dart';
import 'events_screen.dart';
import '../widgets/sky_bar.dart';

/// Your node in plain words (ADR 0031 §1): opened from your star in the
/// Universe and from You. Everything technical is under Advanced details.
/// It says what the node has been doing, from its own counts (ADR 0036 §3).
class NodeSummaryScreen extends StatefulWidget {
  const NodeSummaryScreen({super.key, required this.node, required this.open});

  final HomeNode node;
  final void Function(Widget screen) open;

  @override
  State<NodeSummaryScreen> createState() => _NodeSummaryScreenState();
}

class _NodeSummaryScreenState extends State<NodeSummaryScreen> {
  @override
  void initState() {
    super.initState();
    widget.node.checkStatus();
  }

  @override
  Widget build(BuildContext context) {
    final node = widget.node;
    final theme = Theme.of(context);
    return Scaffold(
      appBar: SkyBar(AppBar(title: const Text('Your node'))),
      body: ListenableBuilder(
        listenable: node,
        builder: (context, _) {
          final s = node.status;
          final online = node.reachability == Reachability.online;
          final problems = s?.problems.length ?? 0;
          return ListView(
            padding: const EdgeInsets.all(Space.m),
            children: [
              const Text(
                'Your node is your home computer. It keeps working while your '
                'phone is off: it gathers the posts of people you follow, and '
                'keeps yours available to them.',
              ),
              const SizedBox(height: Space.l),
              _Line(
                icon: online ? Icons.check_circle : Icons.cloud_off,
                text: switch (node.reachability) {
                  Reachability.online => 'Your phone reaches it.',
                  Reachability.offline =>
                    "Your phone can't reach it right now.",
                  _ => 'Reaching it…',
                },
                detail: !online && node.lastContact != null
                    ? 'Last reached ${formatAgo(node.lastContact!)}'
                    : null,
              ),
              if (online && s != null) ...[
                _Line(
                  icon: Icons.schedule,
                  text: 'Running for ${formatUptime(s.uptimeSecs)}.',
                ),
                // What it did for the network, from its own counts
                // (ADR 0036 §3).
                if (s.hostedPosts > 0)
                  _Line(
                    icon: Icons.inventory_2_outlined,
                    text: s.hostedPosts == 1
                        ? 'It keeps 1 post you shared available to others.'
                        : 'It keeps ${s.hostedPosts} posts you shared '
                              'available to others.',
                  ),
                _Line(
                  icon: Icons.upload_outlined,
                  text: s.servedLatelyBytes == 0
                      ? 'It sent no photos or videos to others in the last '
                            '30 days.'
                      : 'It sent ${formatBytes(s.servedLatelyBytes)} of '
                            'photos and videos to others in the last 30 days.',
                ),
                if (s.carrier case final c?)
                  _Line(
                    icon: Icons.route_outlined,
                    text: carryingLine(c),
                    detail: 'Carrying: limits and your own video',
                    onTap: () => widget.open(CarryingScreen(node: node)),
                  ),
                _Line(
                  icon: Icons.people_outline,
                  text:
                      'You follow ${_people(s.follows)}, and '
                      '${_people(s.followers)} ${s.followers == 1 ? 'follows' : 'follow'} you.',
                ),
                _Line(
                  icon: Icons.storage_outlined,
                  text:
                      'It uses ${formatBytes(s.spaceBytes)} of space for OSP.',
                ),
                _Line(
                  icon: problems == 0 ? Icons.favorite_border : Icons.warning,
                  text: problems == 0
                      ? 'It reports no problems.'
                      : problems == 1
                      ? 'It reports a problem, described under Advanced details.'
                      : 'It reports $problems problems, described under '
                            'Advanced details.',
                ),
              ],
              if (node.reachability == Reachability.offline)
                Padding(
                  padding: const EdgeInsets.only(top: Space.s),
                  child: Text(
                    'Check that the computer is on and connected. What went '
                    'wrong is described under Advanced details.',
                    style: theme.textTheme.bodySmall,
                  ),
                ),
              const SizedBox(height: Space.l),
              Wrap(
                spacing: Space.s,
                runSpacing: Space.s,
                children: [
                  if (!online)
                    FilledButton(
                      onPressed: node.refresh,
                      child: const Text('Try again'),
                    ),
                  OutlinedButton(
                    onPressed: () => widget.open(
                      EventsScreen(node: node, open: widget.open),
                    ),
                    child: const Text('What it did today'),
                  ),
                  OutlinedButton(
                    onPressed: () => widget.open(AdvancedScreen(node: node)),
                    child: const Text('Advanced details'),
                  ),
                ],
              ),
            ],
          );
        },
      ),
    );
  }
}

String _people(int n) => n == 1 ? '1 person' : '$n people';

class _Line extends StatelessWidget {
  const _Line({
    required this.icon,
    required this.text,
    this.detail,
    this.onTap,
  });

  final IconData icon;
  final String text;
  final String? detail;

  /// Set, the line opens more, and shows it with an arrow.
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final line = Padding(
      padding: const EdgeInsets.symmetric(vertical: Space.s),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, size: 20),
          const SizedBox(width: Space.m),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(text),
                if (detail != null)
                  Text(detail!, style: Theme.of(context).textTheme.bodySmall),
              ],
            ),
          ),
          if (onTap != null) const Icon(Icons.chevron_right, size: 20),
        ],
      ),
    );
    return onTap == null ? line : InkWell(onTap: onTap, child: line);
  }
}
