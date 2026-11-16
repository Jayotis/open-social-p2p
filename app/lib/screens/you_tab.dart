import 'package:flutter/material.dart';

import '../node.dart';
import '../theme.dart';
import '../widgets/person_avatar.dart';
import 'advanced_screen.dart';
import 'node_summary_screen.dart';
import 'people_screen.dart';
import 'profile_screen.dart';
import 'trust_screen.dart';

/// You (ADR 0031 §1): your profile and posts (ADR 0032), the people you
/// know, your node, and Advanced details.
class YouTab extends StatelessWidget {
  const YouTab({super.key, required this.node, required this.open});

  final HomeNode node;
  final void Function(Widget screen) open;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final me = node.me;
    final name = me?.name ?? node.status?.name ?? node.paired?.name ?? '';
    final identity = node.paired?.identity;
    return ListView(
      padding: const EdgeInsets.only(bottom: 88),
      children: [
        InkWell(
          onTap: identity == null
              ? null
              : () => open(
                  ProfileScreen(node: node, identity: identity, name: name),
                ),
          child: Padding(
            padding: const EdgeInsets.all(Space.l),
            child: Row(
              children: [
                PersonAvatar(
                  node: node,
                  name: name,
                  identity: identity,
                  picture: me?.picture,
                  radius: 28,
                ),
                const SizedBox(width: Space.m),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        name.isEmpty ? 'You' : name,
                        style: theme.textTheme.headlineSmall,
                      ),
                      Text(
                        'Your profile and posts',
                        style: theme.textTheme.bodySmall,
                      ),
                    ],
                  ),
                ),
                const Icon(Icons.chevron_right),
              ],
            ),
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
