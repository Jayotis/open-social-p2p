import 'package:flutter/material.dart';

import '../format.dart';
import '../network_events.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';

/// What the owner's node did or saw in the last day (ADR 0036 §1), newest
/// first, in plain words. Opened from the line under the map and from the
/// node's summary.
class EventsScreen extends StatefulWidget {
  const EventsScreen({super.key, required this.node, required this.open});

  final HomeNode node;
  final void Function(Widget screen) open;

  @override
  State<EventsScreen> createState() => _EventsScreenState();
}

class _EventsScreenState extends State<EventsScreen> {
  @override
  void initState() {
    super.initState();
    widget.node.loadEvents();
  }

  @override
  Widget build(BuildContext context) {
    final node = widget.node;
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(title: const Text('Your node today')),
      body: ListenableBuilder(
        listenable: node,
        builder: (context, _) {
          final events = node.events.today;
          final names = EventNames([
            ...?node.universeTop?.people,
            ...?node.universe?.people,
          ]);
          return ListView(
            padding: const EdgeInsets.symmetric(vertical: Space.s),
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(
                  Space.m,
                  Space.s,
                  Space.m,
                  Space.m,
                ),
                child: Text(
                  'The last day, from your node’s own record. It names the '
                  'posts it sent, never who asked for them, and nothing you '
                  'opened yourself.',
                  style: theme.textTheme.bodyMedium,
                ),
              ),
              if (events.isEmpty)
                const ListTile(
                  leading: Icon(Icons.nightlight_outlined),
                  title: Text('Nothing yet today'),
                  subtitle: Text(
                    'When your node finds people, keeps a post available, or '
                    'sends one to others, it shows here and on the map.',
                  ),
                ),
              for (final e in events)
                ListTile(
                  leading: Icon(eventIcon(e.kind)),
                  title: Text(describeEvent(e, names)),
                  subtitle: Text(formatAgo(eventTime(e))),
                ),
            ],
          );
        },
      ),
    );
  }
}

IconData eventIcon(EventKind kind) => switch (kind) {
  EventKind.found => Icons.person_add_alt,
  EventKind.reshared => Icons.repeat,
  EventKind.fetched => Icons.download_outlined,
  EventKind.served => Icons.upload_outlined,
  EventKind.machine => Icons.dns_outlined,
};
