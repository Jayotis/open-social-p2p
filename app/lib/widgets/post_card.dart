import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../screens/image_screen.dart';
import '../screens/player_screen.dart';
import '../screens/profile_screen.dart';
import '../screens/thread_screen.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';
import 'like_button.dart';
import 'person_avatar.dart';
import 'post_action.dart';

/// One post in the feed: author, text, photos and videos, and reshare.
class PostCard extends StatelessWidget {
  const PostCard({super.key, required this.node, required this.post});

  final HomeNode node;
  final Post post;

  Future<void> _reshare(BuildContext context) async {
    final host = await showModalBottomSheet<bool>(
      context: context,
      builder: (context) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ListTile(
              leading: const Icon(Icons.repeat),
              title: const Text('Reshare'),
              onTap: () => Navigator.pop(context, false),
            ),
            if (post.media.isNotEmpty)
              ListTile(
                leading: const Icon(Icons.cloud_upload_outlined),
                title: const Text('Share, and help keep it available'),
                subtitle: const Text(
                  'Your node keeps a copy of the photos and videos and passes '
                  "it on, so they stay available when the author's node is "
                  'off. This uses your home upload bandwidth.',
                ),
                onTap: () => Navigator.pop(context, true),
              ),
          ],
        ),
      ),
    );
    if (host == null || !context.mounted) return;
    final messenger = ScaffoldMessenger.of(context);
    try {
      await node.app.reshare(postId: post.id, host: host);
      messenger.showSnackBar(
        SnackBar(
          content: Text(host ? 'Reshared, and hosting a copy.' : 'Reshared.'),
        ),
      );
      await node.refresh();
    } catch (e) {
      messenger.showSnackBar(
        SnackBar(content: Text("Couldn't reshare: ${describeError(e)}")),
      );
    }
  }

  /// Deletes the owner's post or reshare after a minute, with an undo
  /// meanwhile, saying first what deleting does (ADR 0019).
  Future<void> _delete(BuildContext context) async {
    final id = post.deletable!;
    final reshare = id != post.id;
    final what = reshare ? 'your reshare' : 'your post';
    final sure = await showModalBottomSheet<bool>(
      context: context,
      builder: (context) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ListTile(
              leading: const Icon(Icons.delete_outline),
              title: Text('Delete $what'),
              subtitle: Text(
                reshare
                    ? 'It goes from the feeds of those who follow you, and '
                          "your node stops hosting the post's media."
                    : 'Nodes running this software drop it, and its photos '
                          'and videos, as they hear: within minutes for those '
                          "online. Copies people saved stay. It can't be "
                          'undone once sent.',
              ),
              onTap: () => Navigator.pop(context, true),
            ),
          ],
        ),
      ),
    );
    if (sure != true || !context.mounted) return;
    final messenger = ScaffoldMessenger.of(context);
    node.deleteLater(
      id,
      onFailed: (e) => messenger.showSnackBar(
        SnackBar(content: Text("Couldn't delete it: ${describeError(e)}")),
      ),
    );
    messenger.showSnackBar(
      SnackBar(
        content: Text('Deleting $what in a minute.'),
        duration: HomeNode.deleteWait,
        action: SnackBarAction(
          label: 'Undo',
          onPressed: () => node.undoDelete(id),
        ),
      ),
    );
  }

  void _openProfile(BuildContext context) => Navigator.of(context).push(
    MaterialPageRoute(
      builder: (_) => ProfileScreen(
        node: node,
        identity: post.authorIdentity,
        name: post.author,
      ),
    ),
  );

  void _openThread(BuildContext context) => Navigator.of(context).push(
    MaterialPageRoute(
      builder: (_) => ThreadScreen(node: node, post: post),
    ),
  );

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final quiet = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final published = DateTime.tryParse(post.published);
    final online = node.reachability == Reachability.online;
    final resharers = post.resharers ?? 0;
    return Card(
      margin: const EdgeInsets.fromLTRB(Space.m, Space.s, Space.m, 0),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (post.resharedBy != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(Space.m, Space.m, Space.m, 0),
              child: Row(
                children: [
                  Icon(Icons.repeat, size: 16, color: quiet?.color),
                  const SizedBox(width: Space.s),
                  Expanded(
                    child: Text(
                      'Reshared by ${post.resharedBy}${post.resharerHosts ? ', who hosts a copy' : ''}',
                      style: quiet,
                    ),
                  ),
                ],
              ),
            ),
          Padding(
            padding: const EdgeInsets.fromLTRB(Space.m, Space.s, Space.xs, 0),
            child: Row(
              children: [
                Expanded(
                  child: InkWell(
                    borderRadius: BorderRadius.circular(Corner.s),
                    onTap: () => _openProfile(context),
                    child: Padding(
                      padding: const EdgeInsets.symmetric(vertical: Space.s),
                      child: Row(
                        children: [
                          PersonAvatar(
                            node: node,
                            name: post.author,
                            identity: post.authorIdentity,
                            radius: 18,
                          ),
                          const SizedBox(width: Space.s + 4),
                          Flexible(
                            child: Text(
                              post.author.isEmpty ? 'Someone' : post.author,
                              style: theme.textTheme.titleSmall,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                            ),
                          ),
                          if (published != null) ...[
                            Text('  ·  ', style: quiet),
                            Text(formatAgo(published), style: quiet),
                          ],
                        ],
                      ),
                    ),
                  ),
                ),
                if (post.deletable != null)
                  PopupMenuButton<String>(
                    tooltip: 'More',
                    icon: Icon(Icons.more_horiz, color: quiet?.color),
                    enabled: online,
                    onSelected: (_) => _delete(context),
                    itemBuilder: (_) => const [
                      PopupMenuItem(
                        value: 'delete',
                        child: ListTile(
                          leading: Icon(Icons.delete_outline),
                          title: Text('Delete'),
                          contentPadding: EdgeInsets.zero,
                        ),
                      ),
                    ],
                  )
                else
                  const SizedBox(height: 48),
              ],
            ),
          ),
          if (post.foundThrough != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(Space.m, 0, Space.m, Space.s),
              child: Text(listedBy(post.foundThrough!), style: quiet),
            ),
          if (post.text.isNotEmpty)
            Padding(
              padding: const EdgeInsets.fromLTRB(Space.m, 0, Space.m, Space.s),
              child: SelectableText(
                post.text,
                style: theme.textTheme.bodyLarge,
              ),
            ),
          for (final media in post.media)
            _MediaTile(node: node, post: post, media: media),
          Padding(
            padding: const EdgeInsets.fromLTRB(Space.xs, 0, Space.xs, Space.xs),
            child: Row(
              children: [
                LikeButton(node: node, post: post, compact: true),
                PostAction(
                  icon: Icons.chat_bubble_outline,
                  label: 'Replies',
                  onPressed: online ? () => _openThread(context) : null,
                ),
                PostAction(
                  icon: Icons.repeat,
                  label: switch (resharers) {
                    0 => 'Reshare',
                    1 => 'Reshare. Reshared by 1 person',
                    _ => 'Reshare. Reshared by $resharers people',
                  },
                  count: resharers,
                  onPressed: online ? () => _reshare(context) : null,
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _MediaTile extends StatelessWidget {
  const _MediaTile({
    required this.node,
    required this.post,
    required this.media,
  });

  final HomeNode node;
  final Post post;
  final Media media;

  void _open(BuildContext context) {
    final screen = media.isVideo
        ? PlayerScreen(node: node, post: post, media: media)
        : ImageScreen(node: node, media: media);
    Navigator.of(context).push(MaterialPageRoute(builder: (_) => screen));
  }

  @override
  Widget build(BuildContext context) {
    final aspect = media.width > 0 && media.height > 0
        ? media.width / media.height
        : 16 / 9;
    // Tall portrait media gets a shorter box in the feed; it opens in full.
    final box = aspect < 0.8 ? 0.8 : aspect;
    final thumb = media.thumbnailBlob;
    final scheme = Theme.of(context).colorScheme;
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.m, Space.xs, Space.m, Space.xs),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(Corner.s + 4),
        child: AspectRatio(
          aspectRatio: box,
          child: InkWell(
            onTap: () => _open(context),
            child: Stack(
              fit: StackFit.expand,
              children: [
                ColoredBox(color: scheme.surfaceContainerHighest),
                Center(
                  child: Icon(
                    media.isVideo ? Icons.movie_outlined : Icons.image_outlined,
                    size: 40,
                    color: scheme.outline,
                  ),
                ),
                if (thumb != null)
                  FutureBuilder<String>(
                    future: node.mediaUrl(thumb, 'image/webp'),
                    builder: (context, url) => url.hasData
                        ? Image.network(
                            url.data!,
                            fit: BoxFit.cover,
                            errorBuilder: (_, _, _) => const Center(
                              child: Icon(Icons.broken_image_outlined),
                            ),
                          )
                        : const SizedBox.shrink(),
                  ),
                if (media.isVideo)
                  Center(
                    child: CircleAvatar(
                      radius: 28,
                      backgroundColor: Palette.night.withValues(alpha: 0.6),
                      child: const Icon(
                        Icons.play_arrow,
                        color: Palette.moon,
                        size: 36,
                      ),
                    ),
                  ),
                Positioned(
                  right: 8,
                  bottom: 8,
                  child: _Badge(
                    text: [
                      if (media.durationMs != null)
                        formatDuration(media.durationMs!),
                      formatBytes(media.size),
                      if (!media.onNode) 'not on your node yet',
                    ].join(' · '),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _Badge extends StatelessWidget {
  const _Badge({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: Space.s, vertical: 3),
      decoration: BoxDecoration(
        color: Palette.night.withValues(alpha: 0.7),
        borderRadius: BorderRadius.circular(Corner.s),
      ),
      child: Text(
        text,
        style: Theme.of(
          context,
        ).textTheme.labelSmall?.copyWith(color: Palette.moon),
      ),
    );
  }
}
