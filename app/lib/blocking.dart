/// Blocking (ADR 0042 §5): the owner's node drops a person or a post and
/// won't fetch, serve or show it again. Nobody is told; it can be undone
/// under You, Blocked.
library;

import 'package:flutter/material.dart';

import 'format.dart';
import 'node.dart';
import 'screens/trust_screen.dart' show shownName;

Future<bool> _confirm(
  BuildContext context, {
  required String title,
  required String body,
  required String yes,
}) async {
  final go = await showDialog<bool>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text(title),
      content: Text(body),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context, false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context, true),
          child: Text(yes),
        ),
      ],
    ),
  );
  return go == true;
}

/// Asks, then blocks a person. Returns whether they were blocked.
Future<bool> blockPerson(
  BuildContext context,
  HomeNode node, {
  required String identity,
  required String name,
}) async {
  final who = shownName(name);
  if (!await _confirm(
    context,
    title: 'Block $who?',
    body:
        "Your node removes $who's posts and stops following them, and won't "
        "show you anything of theirs again. Their replies to your posts are "
        "hidden. $who isn't told. You can undo it under You, Blocked.",
    yes: 'Block',
  )) {
    return false;
  }
  if (!context.mounted) return false;
  return _run(
    context,
    node,
    () => node.app.block(kind: 'account', id: identity),
    '$who is blocked.',
  );
}

/// Asks, then blocks one post. Returns whether it was blocked.
Future<bool> blockPost(BuildContext context, HomeNode node, String id) async {
  if (!await _confirm(
    context,
    title: 'Hide this post?',
    body:
        "Your node removes it, and reshares of it, and won't show it to you "
        "again. Its author isn't told. You can undo it under You, Blocked.",
    yes: 'Hide it',
  )) {
    return false;
  }
  if (!context.mounted) return false;
  return _run(
    context,
    node,
    () => node.app.block(kind: 'post', id: id),
    'Post hidden.',
  );
}

Future<bool> _run(
  BuildContext context,
  HomeNode node,
  Future<void> Function() block,
  String done,
) async {
  final messenger = ScaffoldMessenger.of(context);
  try {
    await block();
  } catch (e) {
    messenger.showSnackBar(SnackBar(content: Text(describeError(e))));
    return false;
  }
  messenger.showSnackBar(SnackBar(content: Text(done)));
  await node.refresh();
  return true;
}
