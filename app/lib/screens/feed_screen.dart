import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import '../widgets/node_banner.dart';
import '../widgets/post_card.dart';
import 'compose_screen.dart';
import 'people_screen.dart';
import 'status_screen.dart';
import 'trust_screen.dart';

/// The home screen: your feed, gathered by your home node.
class FeedScreen extends StatelessWidget {
  const FeedScreen({super.key, required this.node});

  final HomeNode node;

  void _open(BuildContext context, Widget screen) {
    Navigator.of(context)
        .push(MaterialPageRoute(builder: (_) => screen))
        .then((_) => node.checkUnsent());
  }

  @override
  Widget build(BuildContext context) {
    final online = node.reachability == Reachability.online;
    // Requests to be a trusted follower waiting for an answer.
    final asking = node.status?.trustRequests ?? 0;
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
            icon: Badge(
              isLabelVisible: asking > 0,
              label: Text('$asking'),
              child: const Icon(Icons.more_vert),
            ),
            onSelected: (choice) => switch (choice) {
              'people' => _open(context, PeopleScreen(node: node)),
              'trust' => _open(context, TrustScreen(node: node)),
              _ => _open(context, StatusScreen(node: node)),
            },
            itemBuilder: (_) => [
              const PopupMenuItem(
                value: 'people',
                child: Text('Follow and be followed'),
              ),
              PopupMenuItem(
                value: 'trust',
                child: Badge(
                  isLabelVisible: asking > 0,
                  label: Text('$asking'),
                  offset: const Offset(16, -4),
                  child: const Text('Trusted followers'),
                ),
              ),
              const PopupMenuItem(
                value: 'status',
                child: Text('Home node status'),
              ),
            ],
          ),
        ],
      ),
      body: Column(
        children: [
          NodeBanner(node: node),
          if (node.unsent case final unsent? when !node.uploading)
            _UnsentCard(
              node: node,
              unsent: unsent,
              onContinue: () =>
                  _open(context, ComposeScreen(node: node, unsent: unsent)),
            ),
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

/// A post that didn't finish sending, say because the app was stopped:
/// carry on with it, or discard it.
class _UnsentCard extends StatelessWidget {
  const _UnsentCard({
    required this.node,
    required this.unsent,
    required this.onContinue,
  });

  final HomeNode node;
  final UnsentPost unsent;
  final VoidCallback onContinue;

  Future<void> _discard(BuildContext context) async {
    final messenger = ScaffoldMessenger.of(context);
    final go = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Discard this post?'),
        content: const Text(
          "It hasn't been published. What was sent so far is deleted.",
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Keep it'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Discard'),
          ),
        ],
      ),
    );
    if (go != true) return;
    try {
      await node.discardPost();
      messenger.showSnackBar(const SnackBar(content: Text('Post discarded.')));
    } catch (e) {
      messenger.showSnackBar(
        SnackBar(content: Text('Too late to discard: ${describeError(e)}')),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final files = unsent.files.length;
    final size = unsent.files.fold(0, (sum, f) => sum + f.size);
    final started = DateTime.tryParse(unsent.started);
    final what = [
      if (unsent.text.isNotEmpty) '"${unsent.text.split('\n').first}"',
      if (files > 0)
        '$files ${files == 1 ? 'file' : 'files'}, ${formatBytes(size)}',
      if (started != null) 'started ${formatAgo(started)}',
    ].join(' · ');
    return Card(
      margin: const EdgeInsets.fromLTRB(12, 12, 12, 0),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 12, 8, 4),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              "A post didn't finish sending",
              style: Theme.of(context).textTheme.titleSmall,
            ),
            const SizedBox(height: 4),
            Text(
              what,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.bodySmall,
            ),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                TextButton(
                  onPressed: () => _discard(context),
                  child: const Text('Discard'),
                ),
                TextButton(
                  onPressed: onContinue,
                  child: const Text('Continue'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}
