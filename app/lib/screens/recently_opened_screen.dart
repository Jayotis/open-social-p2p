import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import '../widgets/sky_bar.dart';
import 'thread_screen.dart';
import 'trust_screen.dart' show shownName;

/// Recently opened (ADR 0039 §4): the posts Kim opened from shared codes,
/// whose authors she doesn't follow, kept on her node for 7 days after she
/// last opened, liked or replied to one. On her node alone; not Activity,
/// and not a record of everything she views.
class RecentlyOpenedScreen extends StatefulWidget {
  const RecentlyOpenedScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<RecentlyOpenedScreen> createState() => _RecentlyOpenedScreenState();
}

class _RecentlyOpenedScreenState extends State<RecentlyOpenedScreen> {
  List<Post>? _posts;
  String? _error;
  bool _busy = false;

  OspApp get _app => widget.node.app;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final posts = await _app.openedPosts();
      if (!mounted) return;
      setState(() {
        _posts = posts;
        _error = null;
      });
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    }
  }

  /// Opens one: its 7 days start over. Back here, the list is read again,
  /// as following its author or removing it there takes it off.
  Future<void> _open(Post post) async {
    final navigator = Navigator.of(context);
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _busy = true);
    try {
      final fresh = await _app.reopenPost(postId: post.id);
      await navigator.push(
        MaterialPageRoute(
          builder: (_) => ThreadScreen(node: widget.node, post: fresh),
        ),
      );
    } catch (e) {
      messenger.showSnackBar(
        SnackBar(content: Text("Couldn't open it: ${describeError(e)}")),
      );
    } finally {
      if (mounted) setState(() => _busy = false);
      await _load();
    }
  }

  Future<void> _remove(Post post) async {
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _posts?.removeWhere((p) => p.id == post.id));
    try {
      await _app.forgetOpened(postId: post.id);
    } catch (e) {
      messenger.showSnackBar(
        SnackBar(content: Text("Couldn't remove it: ${describeError(e)}")),
      );
    }
    await _load();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final posts = _posts;
    return Scaffold(
      appBar: SkyBar(AppBar(title: const Text('Recently opened'))),
      body: RefreshIndicator(
        onRefresh: _load,
        child: ListView(
          padding: const EdgeInsets.symmetric(vertical: 8),
          children: [
            const Padding(
              padding: EdgeInsets.fromLTRB(16, 8, 16, 8),
              child: Text(
                'Posts you opened from shared codes, by people you don’t '
                'follow. Only on your node, and each goes 7 days after you '
                'last open it.',
              ),
            ),
            if (_busy) const LinearProgressIndicator(),
            if (_error case final error?)
              Padding(padding: const EdgeInsets.all(16), child: Text(error))
            else if (posts == null)
              const Padding(
                padding: EdgeInsets.all(24),
                child: Center(child: CircularProgressIndicator()),
              )
            else if (posts.isEmpty)
              Padding(
                padding: const EdgeInsets.all(16),
                child: Text(
                  'Nothing opened lately.',
                  style: theme.textTheme.bodyLarge,
                ),
              )
            else
              for (final post in posts)
                ListTile(
                  title: Text(shownName(post.author)),
                  subtitle: Text(
                    post.text.isEmpty
                        ? (post.media.isEmpty ? 'A post' : 'Photos or video')
                        : post.text,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                  ),
                  onTap: _busy ? null : () => _open(post),
                  trailing: IconButton(
                    tooltip: 'Remove',
                    icon: const Icon(Icons.close),
                    onPressed: _busy ? null : () => _remove(post),
                  ),
                ),
          ],
        ),
      ),
    );
  }
}
