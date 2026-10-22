import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../screens/image_screen.dart';
import '../screens/player_screen.dart';
import '../src/rust/api/osp.dart';

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
                title: const Text('Reshare and host a copy'),
                subtitle: const Text(
                  'Your home node keeps the media and serves it to others, '
                  "so it stays available when the author's node is off. "
                  'This uses your home upload bandwidth.',
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

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final published = DateTime.tryParse(post.published);
    return Card(
      margin: const EdgeInsets.fromLTRB(12, 8, 12, 0),
      clipBehavior: Clip.antiAlias,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (post.resharedBy != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 10, 16, 0),
              child: Row(
                children: [
                  Icon(
                    Icons.repeat,
                    size: 16,
                    color: theme.colorScheme.secondary,
                  ),
                  const SizedBox(width: 6),
                  Expanded(
                    child: Text(
                      'Reshared by ${post.resharedBy}${post.resharerHosts ? ', who hosts a copy' : ''}',
                      style: theme.textTheme.bodySmall,
                    ),
                  ),
                ],
              ),
            ),
          ListTile(
            leading: CircleAvatar(
              child: Text(
                post.author.isEmpty ? '?' : post.author[0].toUpperCase(),
              ),
            ),
            title: Text(post.author),
            subtitle: published != null ? Text(formatAgo(published)) : null,
          ),
          if (post.text.isNotEmpty)
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
              child: SelectableText(
                post.text,
                style: theme.textTheme.bodyLarge,
              ),
            ),
          for (final media in post.media)
            _MediaTile(node: node, post: post, media: media),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              TextButton.icon(
                onPressed: node.reachability == Reachability.online
                    ? () => _reshare(context)
                    : null,
                icon: const Icon(Icons.repeat),
                label: const Text('Reshare'),
              ),
              const SizedBox(width: 8),
            ],
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
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: AspectRatio(
        aspectRatio: box,
        child: InkWell(
          onTap: () => _open(context),
          child: Stack(
            fit: StackFit.expand,
            children: [
              Container(color: Colors.black12),
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
                const Center(
                  child: CircleAvatar(
                    radius: 28,
                    backgroundColor: Colors.black54,
                    child: Icon(
                      Icons.play_arrow,
                      color: Colors.white,
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
    );
  }
}

class _Badge extends StatelessWidget {
  const _Badge({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(
        color: Colors.black54,
        borderRadius: BorderRadius.circular(4),
      ),
      child: Text(
        text,
        style: const TextStyle(color: Colors.white, fontSize: 12),
      ),
    );
  }
}
