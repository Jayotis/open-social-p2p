import 'package:flutter/material.dart';

import '../node.dart';
import '../theme.dart';
import 'advanced_screen.dart';
import 'node_summary_screen.dart';
import 'people_screen.dart';
import 'trust_screen.dart';

/// You (ADR 0031 §1): you, the people you know, your node, and Advanced
/// details. Your profile and your own posts come with P2 (ADR 0032).
class YouTab extends StatelessWidget {
  const YouTab({super.key, required this.node, required this.open});

  final HomeNode node;
  final void Function(Widget screen) open;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final name = node.status?.name ?? node.paired?.name ?? '';
    return ListView(
      padding: const EdgeInsets.only(bottom: 88),
      children: [
        Padding(
          padding: const EdgeInsets.all(Space.l),
          child: Row(
            children: [
              CircleAvatar(
                radius: 28,
                child: Text(
                  name.isEmpty ? '?' : name[0].toUpperCase(),
                  style: theme.textTheme.headlineSmall,
                ),
              ),
              const SizedBox(width: Space.m),
              Expanded(
                child: Text(
                  name.isEmpty ? 'You' : name,
                  style: theme.textTheme.headlineSmall,
                ),
              ),
            ],
          ),
        ),
        ListTile(
          leading: const Icon(Icons.qr_code),
          title: const Text('Invite and follow'),
          subtitle: const Text(
            'Your invite code, and following someone with theirs',
          ),
          onTap: () => open(PeopleScreen(node: node)),
        ),
        ListTile(
          leading: const Icon(Icons.handshake_outlined),
          title: const Text('People you know'),
          onTap: () => open(TrustScreen(node: node)),
        ),
        ListTile(
          leading: const Icon(Icons.computer),
          title: const Text('Your node'),
          subtitle: Text(switch (node.reachability) {
            Reachability.online => 'Your home computer, running',
            Reachability.offline => "Your home computer, can't be reached",
            _ => 'Your home computer',
          }),
          onTap: () => open(NodeSummaryScreen(node: node, open: open)),
        ),
        const Divider(),
        ListTile(
          leading: const Icon(Icons.tune),
          title: const Text('Advanced details'),
          onTap: () => open(AdvancedScreen(node: node)),
        ),
      ],
    );
  }
}
