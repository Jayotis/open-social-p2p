import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../widgets/node_banner.dart';
import '../widgets/post_card.dart';
import 'compose_screen.dart';
import 'people_screen.dart';
import 'status_screen.dart';

/// The home screen: your feed, gathered by your home node.
class FeedScreen extends StatelessWidget {
  const FeedScreen({super.key, required this.node});

  final HomeNode node;

  void _open(BuildContext context, Widget screen) {
    Navigator.of(context).push(MaterialPageRoute(builder: (_) => screen));
  }

  @override
  Widget build(BuildContext context) {
    final online = node.reachability == Reachability.online;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Feed'),
        actions: [
          IconButton(
            tooltip: 'Home node status',
            onPressed: () => _open(context, StatusScreen(node: node)),
            icon: _StatusDot(node: node),
          ),
          PopupMenuButton<String>(
            onSelected: (choice) => switch (choice) {
              'people' => _open(context, PeopleScreen(node: node)),
              _ => _open(context, StatusScreen(node: node)),
            },
            itemBuilder: (_) => const [
              PopupMenuItem(
                value: 'people',
                child: Text('Follow and be followed'),
              ),
              PopupMenuItem(value: 'status', child: Text('Home node status')),
            ],
          ),
        ],
      ),
      body: Column(
        children: [
          NodeBanner(node: node),
          Expanded(
            child: RefreshIndicator(
              onRefresh: () => node.refresh(sync: true),
              child: node.posts.isEmpty
                  ? _Empty(node: node)
                  : ListView.builder(
                      padding: const EdgeInsets.only(bottom: 88),
                      itemCount: node.posts.length,
                      itemBuilder: (context, i) =>
                          PostCard(node: node, post: node.posts[i]),
                    ),
            ),
          ),
        ],
      ),
      floatingActionButton: FloatingActionButton(
        tooltip: online
            ? 'New post'
            : "Can't post while your home node is unreachable",
        onPressed: online
            ? () => _open(context, ComposeScreen(node: node))
            : null,
        backgroundColor: online ? null : Theme.of(context).disabledColor,
        child: const Icon(Icons.edit),
      ),
    );
  }
}

class _StatusDot extends StatelessWidget {
  const _StatusDot({required this.node});

  final HomeNode node;

  @override
  Widget build(BuildContext context) {
    final problems = node.status?.problems.isNotEmpty ?? false;
    final color = switch (node.reachability) {
      Reachability.online when problems => Colors.amber,
      Reachability.online => Colors.green,
      Reachability.offline => Colors.red,
      _ => Colors.grey,
    };
    return Icon(Icons.circle, color: color, size: 16);
  }
}

class _Empty extends StatelessWidget {
  const _Empty({required this.node});

  final HomeNode node;

  @override
  Widget build(BuildContext context) {
    final message = switch (node.reachability) {
      Reachability.connecting || Reachability.unknown => 'Loading your feed…',
      Reachability.offline =>
        'Nothing saved on this phone yet. Pull down to try again.',
      Reachability.online =>
        "Nothing here yet. Follow someone (menu → Follow and be followed), "
            'or post something with the pencil button.',
    };
    // A scrollable child, so pull-to-refresh works on an empty feed too.
    return ListView(
      children: [
        Padding(
          padding: const EdgeInsets.all(32),
          child: Text(message, textAlign: TextAlign.center),
        ),
        if (node.lastError != null && node.reachability == Reachability.offline)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 32),
            child: Text(
              node.lastError!,
              textAlign: TextAlign.center,
              style: Theme.of(context).textTheme.bodySmall,
            ),
          ),
        if (node.lastContact != null)
          Text(
            'Last reached ${formatAgo(node.lastContact!)}',
            textAlign: TextAlign.center,
            style: Theme.of(context).textTheme.bodySmall,
          ),
      ],
    );
  }
}
