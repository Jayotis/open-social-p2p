import 'dart:math';

import 'package:flutter/material.dart';

import '../node.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';
import 'node_summary_screen.dart';
import 'found_people_screen.dart';

/// The Universe (ADR 0031 §1): the network as a map, with your star at the
/// centre. For now your star, the sky, and who your node has found as a
/// list (ADR 0035's "People your node found", brought forward): the map
/// around your star comes with P3 and P4 (ADRs 0033, 0034). What it says
/// comes from the node's real state only (ADR 0036).
class UniverseTab extends StatelessWidget {
  const UniverseTab({super.key, required this.node, required this.open});

  final HomeNode node;
  final void Function(Widget screen) open;

  @override
  Widget build(BuildContext context) {
    final sky = OspColors.of(context);
    final theme = Theme.of(context);
    final name = node.status?.name ?? node.paired?.name ?? '';
    final line = switch (node.reachability) {
      Reachability.online => 'Your node is running.',
      Reachability.offline => "Your phone can't reach your node right now.",
      _ => 'Reaching your node…',
    };
    return ColoredBox(
      color: sky.sky,
      child: Stack(
        children: [
          Positioned.fill(child: CustomPaint(painter: _Sky(sky))),
          Center(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Semantics(
                  button: true,
                  label: 'Your node',
                  child: Tooltip(
                    message: 'Your node',
                    child: InkResponse(
                      onTap: () =>
                          open(NodeSummaryScreen(node: node, open: open)),
                      radius: 56,
                      child: _YourStar(color: sky.yourStar),
                    ),
                  ),
                ),
                const SizedBox(height: Space.s),
                Text(
                  name.isEmpty ? 'You' : name,
                  style: theme.textTheme.titleMedium?.copyWith(
                    color: sky.onSky,
                  ),
                ),
                const SizedBox(height: Space.xs),
                Text(
                  line,
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: sky.onSkyQuiet,
                  ),
                ),
              ],
            ),
          ),
          Positioned(
            left: Space.m,
            right: Space.m,
            bottom: 96,
            child: _FoundPeople(node: node, open: open),
          ),
        ],
      ),
    );
  }
}

class _YourStar extends StatelessWidget {
  const _YourStar({required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 40,
      height: 40,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: color,
        boxShadow: [
          BoxShadow(color: color.withValues(alpha: 0.6), blurRadius: 24),
          BoxShadow(
            color: color.withValues(alpha: 0.25),
            blurRadius: 64,
            spreadRadius: 16,
          ),
        ],
      ),
    );
  }
}

/// Faint background stars, the same each time: decoration, not people.
class _Sky extends CustomPainter {
  _Sky(this.colors);

  final OspColors colors;

  @override
  void paint(Canvas canvas, Size size) {
    final centre = size.center(Offset.zero);
    canvas.drawCircle(
      centre,
      size.shortestSide * 0.6,
      Paint()
        ..shader = RadialGradient(colors: [colors.skyHaze, colors.sky])
            .createShader(
              Rect.fromCircle(center: centre, radius: size.shortestSide * 0.6),
            ),
    );
    final random = Random(31);
    final paint = Paint();
    for (var i = 0; i < 90; i++) {
      final at = Offset(
        random.nextDouble() * size.width,
        random.nextDouble() * size.height,
      );
      paint.color = colors.distantStar.withValues(
        alpha: 0.2 + random.nextDouble() * 0.5,
      );
      canvas.drawCircle(at, 0.6 + random.nextDouble() * 1.2, paint);
    }
  }

  @override
  bool shouldRepaint(_Sky old) => old.colors != colors;
}

/// Who your node has found, as a card on the sky that opens the list.
class _FoundPeople extends StatelessWidget {
  const _FoundPeople({required this.node, required this.open});

  final HomeNode node;
  final void Function(Widget screen) open;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final people = node.people;
    final (title, detail) = switch (people) {
      null when node.peopleError != null => (
        "Couldn't ask your node who it found",
        'Tap to try again.',
      ),
      null => ('Your node is finding people…', null),
      [] => ('Your node is looking for people', 'It checks every few minutes.'),
      [final one] => ('Your node found 1 person', one.name),
      _ => ('Your node found ${people.length} people', _names(people)),
    };
    return Card(
      child: ListTile(
        leading: const Icon(Icons.people_outline),
        title: Text(title, style: theme.textTheme.titleSmall),
        subtitle: detail == null
            ? null
            : Text(detail, maxLines: 1, overflow: TextOverflow.ellipsis),
        trailing: const Icon(Icons.chevron_right),
        onTap: () => open(FoundPeopleScreen(node: node)),
      ),
    );
  }

  static String _names(List<FoundPerson> people) {
    final named = people.map((p) => p.name).where((n) => n.isNotEmpty);
    final first = named.take(3).toList();
    final more = people.length - first.length;
    return more > 0 ? '${first.join(', ')} and $more more' : first.join(', ');
  }
}
