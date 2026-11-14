import 'package:flutter/material.dart';

import '../node.dart';
import '../theme.dart';
import 'reply_requests_screen.dart';
import 'trust_screen.dart';

/// Activity (ADR 0031 §1): what's waiting for you. Until likes and the
/// inbox of ADR 0038 (P8), that's replies to answer and people who say they
/// know you.
class ActivityScreen extends StatefulWidget {
  const ActivityScreen({super.key, required this.node, required this.open});

  final HomeNode node;
  final void Function(Widget screen) open;

  @override
  State<ActivityScreen> createState() => _ActivityScreenState();
}

class _ActivityScreenState extends State<ActivityScreen> {
  @override
  void initState() {
    super.initState();
    widget.node.checkStatus();
  }

  @override
  Widget build(BuildContext context) {
    final node = widget.node;
    return Scaffold(
      appBar: AppBar(title: const Text('Activity')),
      body: ListenableBuilder(
        listenable: node,
        builder: (context, _) {
          final replies = node.status?.replyRequests ?? 0;
          final knows = node.status?.trustRequests ?? 0;
          return ListView(
            children: [
              if (replies + knows == 0)
                const Padding(
                  padding: EdgeInsets.all(Space.l),
                  child: Text('Nothing waiting for you.'),
                ),
              _Entry(
                icon: Icons.forum_outlined,
                title: 'Replies waiting for you',
                count: replies,
                onTap: () => widget.open(ReplyRequestsScreen(node: node)),
              ),
              _Entry(
                icon: Icons.handshake_outlined,
                title: 'People who say they know you',
                count: knows,
                onTap: () => widget.open(TrustScreen(node: node)),
              ),
            ],
          );
        },
      ),
    );
  }
}

class _Entry extends StatelessWidget {
  const _Entry({
    required this.icon,
    required this.title,
    required this.count,
    required this.onTap,
  });

  final IconData icon;
  final String title;
  final int count;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return ListTile(
      leading: Icon(icon),
      title: Text(title),
      trailing: count > 0
          ? Badge(label: Text('$count'))
          : const Icon(Icons.chevron_right),
      onTap: onTap,
    );
  }
}
