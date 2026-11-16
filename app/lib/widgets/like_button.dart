import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../screens/likers_screen.dart';
import '../src/rust/api/osp.dart';
import 'post_action.dart';

/// Like on someone's post (ADR 0038), shown liked at once; the node sends
/// it on, trying their node for days if it must. On the owner's own post,
/// how many like it, opening who.
class LikeButton extends StatefulWidget {
  const LikeButton({
    super.key,
    required this.node,
    required this.post,
    this.liked,
    this.onChanged,
    this.compact = false,
  });

  /// Under a post card: the icon and its count, without words (ADR 0040).
  final bool compact;

  final HomeNode node;
  final Post post;

  /// Whether the owner likes it, where it's known better than [post] says.
  final bool? liked;

  /// Called with the owner's new choice once their node has kept it.
  final void Function(bool liked)? onChanged;

  @override
  State<LikeButton> createState() => _LikeButtonState();
}

class _LikeButtonState extends State<LikeButton> {
  late bool _liked = widget.liked ?? widget.post.liked;
  bool _busy = false;

  bool get _own => widget.post.authorIdentity == widget.node.paired?.identity;

  @override
  void didUpdateWidget(LikeButton old) {
    super.didUpdateWidget(old);
    if (!_busy && widget.liked != null) _liked = widget.liked!;
  }

  Future<void> _toggle() async {
    final messenger = ScaffoldMessenger.of(context);
    final liked = !_liked;
    setState(() {
      _liked = liked;
      _busy = true;
    });
    try {
      await widget.node.app.like(postId: widget.post.id, liked: liked);
      widget.onChanged?.call(liked);
    } catch (e) {
      if (!mounted) return;
      setState(() => _liked = !liked);
      messenger.showSnackBar(
        SnackBar(
          content: Text(
            liked
                ? "Couldn't like it: ${describeError(e)}"
                : "Couldn't take your like back: ${describeError(e)}",
          ),
        ),
      );
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// How many like it, where known, with the owner's change shown at once.
  int? get _count {
    final n = widget.post.likes;
    if (n == null) return null;
    final was = widget.liked ?? widget.post.liked;
    return n + (_liked == was ? 0 : (_liked ? 1 : -1));
  }

  void _openLikers() => Navigator.of(context).push(
    MaterialPageRoute(
      builder: (_) => LikersScreen(node: widget.node, post: widget.post),
    ),
  );

  @override
  Widget build(BuildContext context) {
    final online = widget.node.reachability == Reachability.online;
    if (widget.compact) {
      final n = _count ?? 0;
      if (_own) {
        return PostAction(
          icon: Icons.favorite_border,
          label: n == 0 ? 'No likes yet' : likesLine(n),
          count: n,
          onPressed: n == 0 ? null : _openLikers,
        );
      }
      return PostAction(
        icon: _liked ? Icons.favorite : Icons.favorite_border,
        label: _liked ? 'Liked' : 'Like',
        count: n,
        color: _liked ? Theme.of(context).colorScheme.primary : null,
        onPressed: online && !_busy ? _toggle : null,
      );
    }
    if (_own) {
      final n = widget.post.likes ?? 0;
      return TextButton.icon(
        onPressed: n == 0 ? null : _openLikers,
        icon: const Icon(Icons.favorite_border),
        label: Text(n == 0 ? 'No likes yet' : likesLine(n)),
      );
    }
    final color = Theme.of(context).colorScheme.primary;
    return TextButton.icon(
      onPressed: online && !_busy ? _toggle : null,
      icon: Icon(
        _liked ? Icons.favorite : Icons.favorite_border,
        color: _liked ? color : null,
      ),
      label: Text(_liked ? 'Liked' : 'Like'),
    );
  }
}

/// "1 like", "3 likes".
String likesLine(int n) => n == 1 ? '1 like' : '$n likes';
