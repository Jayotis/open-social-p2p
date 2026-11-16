import 'package:flutter/material.dart';

import '../widgets/person_avatar.dart';
import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import 'profile_screen.dart';
import '../widgets/sky_bar.dart';

/// Who likes one of the owner's posts (ADR 0038), newest first. Only the
/// owner's node knows; anyone else reading the post sees how many.
class LikersScreen extends StatefulWidget {
  const LikersScreen({super.key, required this.node, required this.post});

  final HomeNode node;
  final Post post;

  @override
  State<LikersScreen> createState() => _LikersScreenState();
}

class _LikersScreenState extends State<LikersScreen> {
  List<Liker>? _likers;
  String? _error;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final likers = await widget.node.app.likes(postId: widget.post.id);
      if (mounted) setState(() => _likers = likers);
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    }
  }

  @override
  Widget build(BuildContext context) {
    final likers = _likers;
    return Scaffold(
      appBar: SkyBar(AppBar(title: const Text('Likes'))),
      body: switch ((likers, _error)) {
        (_, final String error) => Padding(
          padding: const EdgeInsets.all(16),
          child: Text("Couldn't load who likes it: $error"),
        ),
        (null, _) => const LinearProgressIndicator(),
        (final List<Liker> l, _) when l.isEmpty => const Padding(
          padding: EdgeInsets.all(16),
          child: Text('Nobody has liked it yet.'),
        ),
        (final List<Liker> l, _) => ListView(
          children: [
            for (final liker in l)
              ListTile(
                leading: InitialAvatar(name: liker.name, seed: liker.identity),
                title: Text(liker.name.isEmpty ? 'Someone' : liker.name),
                subtitle: switch (DateTime.tryParse(liker.published)) {
                  final t? => Text(formatAgo(t)),
                  null => null,
                },
                onTap: () => Navigator.of(context).push(
                  MaterialPageRoute(
                    builder: (_) => ProfileScreen(
                      node: widget.node,
                      identity: liker.identity,
                      name: liker.name,
                    ),
                  ),
                ),
              ),
          ],
        ),
      },
    );
  }
}
