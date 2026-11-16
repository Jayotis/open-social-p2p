import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';
import '../widgets/node_banner.dart';
import '../widgets/post_card.dart';
import 'compose_screen.dart';
import '../widgets/empty_state.dart';

/// Home (ADR 0031): the posts of people you follow and what they reshared,
/// newest first, gathered by your node.
class HomeTab extends StatelessWidget {
  const HomeTab({
    super.key,
    required this.node,
    required this.open,
    required this.onExplore,
  });

  final HomeNode node;

  /// Opens a screen on top of the app's places.
  final void Function(Widget screen) open;

  /// Goes to the Universe, where an empty Home points.
  final VoidCallback onExplore;

  @override
  Widget build(BuildContext context) {
    final posts = node.visiblePosts;
    return Column(
      children: [
        NodeBanner(node: node, open: open),
        if (node.unsent case final unsent? when !node.uploading)
          _UnsentCard(
            node: node,
            unsent: unsent,
            onContinue: () => open(ComposeScreen(node: node, unsent: unsent)),
          ),
        Expanded(
          child: RefreshIndicator(
            onRefresh: () => node.refresh(sync: true),
            child: posts.isEmpty
                ? _Empty(node: node, onExplore: onExplore)
                : ListView.builder(
                    padding: const EdgeInsets.only(bottom: 88),
                    itemCount: posts.length,
                    itemBuilder: (context, i) =>
                        PostCard(node: node, post: posts[i]),
                  ),
          ),
        ),
      ],
    );
  }
}

class _Empty extends StatelessWidget {
  const _Empty({required this.node, required this.onExplore});

  final HomeNode node;
  final VoidCallback onExplore;

  @override
  Widget build(BuildContext context) {
    final online = node.reachability == Reachability.online;
    final message = switch (node.reachability) {
      Reachability.connecting || Reachability.unknown => 'Loading Home…',
      Reachability.offline =>
        'Nothing saved on this phone yet. Pull down to try again.',
      Reachability.online =>
        'Home shows the posts of people you follow, and you follow nobody '
            'yet.\n\nExplore the Universe to find people and what they post.',
    };
    // A scrollable child, so pull-to-refresh works on an empty Home too.
    return ListView(
      children: [
        const SizedBox(height: Space.xl),
        EmptyState(
          icon: switch (node.reachability) {
            Reachability.offline => Icons.cloud_off,
            Reachability.online => Icons.auto_awesome,
            _ => Icons.hourglass_empty,
          },
          message: message,
          action: online
              ? FilledButton.icon(
                  onPressed: onExplore,
                  icon: const Icon(Icons.auto_awesome),
                  label: const Text('Explore the Universe'),
                )
              : null,
          footnote: node.lastContact != null && !online
              ? 'Last reached ${formatAgo(node.lastContact!)}'
              : null,
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
