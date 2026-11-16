import 'package:flutter/material.dart';

import '../node.dart';
import '../theme.dart';
import '../widgets/person_avatar.dart';
import '../widgets/sky_band.dart';
import 'advanced_screen.dart';
import 'node_summary_screen.dart';
import 'people_screen.dart';
import 'profile_screen.dart';
import 'recently_opened_screen.dart';
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
    final sky = OspColors.of(context);
    return ListView(
      padding: const EdgeInsets.only(bottom: 88),
      children: [
        // You on your own strip of sky, your star in starlight, flush
        // under the bar so the two are one sky.
        Card(
          margin: const EdgeInsets.only(bottom: Space.s),
          shape: const RoundedRectangleBorder(
            borderRadius: BorderRadius.vertical(
              bottom: Radius.circular(Corner.m),
            ),
          ),
          color: sky.sky,
          child: InkWell(
            onTap: identity == null
                ? null
                : () => open(
                    ProfileScreen(node: node, identity: identity, name: name),
                  ),
            child: Stack(
              children: [
                Positioned.fill(
                  child: SkyBand(
                    seed: identity ?? name,
                    glow: sky.yourStar,
                    glowAt: const Alignment(-0.72, 0),
                  ),
                ),
                Padding(
                  padding: const EdgeInsets.all(Space.l),
                  child: Row(
                    children: [
                      PersonAvatar(
                        node: node,
                        name: name,
                        identity: identity,
                        picture: me?.picture,
                        radius: 30,
                      ),
                      const SizedBox(width: Space.m),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              name.isEmpty ? 'You' : name,
                              style: theme.textTheme.headlineSmall?.copyWith(
                                color: sky.onSky,
                              ),
                            ),
                            Text(
                              'Your profile and posts',
                              style: theme.textTheme.bodyMedium?.copyWith(
                                color: sky.onSkyQuiet,
                              ),
                            ),
                          ],
                        ),
                      ),
                      Icon(Icons.chevron_right, color: sky.onSky),
                    ],
                  ),
                ),
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
          leading: const Icon(Icons.history),
          title: const Text('Recently opened'),
          subtitle: const Text('Posts you opened from shared codes'),
          onTap: () => open(RecentlyOpenedScreen(node: node)),
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
