import 'package:flutter/material.dart';
import 'package:share_plus/share_plus.dart';

import 'format.dart';
import 'node.dart';
import 'screens/profile_screen.dart';
import 'screens/thread_screen.dart';
import 'src/rust/api/osp.dart';

/// Where someone without OSP learns to install it (ADR 0039 §4). Until
/// there's a project site, the install guide on the repository's host.
const installGuide =
    'https://github.com/Jayotis/open-social-p2p/blob/main/docs/install.md'; // not shown

/// What sharing a profile says, for people who don't have OSP yet, with
/// the code last so it's easy to copy.
String personShareText({
  required String name,
  required String code,
  bool own = false,
}) {
  final who = own
      ? "I'm on Open Social P2P."
      : '${name.isEmpty ? 'Someone' : name} is on Open Social P2P.';
  return '$who To follow ${own ? 'me' : 'them'}, get OSP ($installGuide), '
      'then paste this code into Invite and follow:\n\n$code';
}

/// What sharing a post says: its first lines, who wrote it, how to get
/// OSP, and the code.
String postShareText({
  required String author,
  required String text,
  required String code,
}) {
  final quote = firstLines(text);
  final by = author.isEmpty ? 'Someone' : author;
  return [
    if (quote.isNotEmpty) '“$quote”',
    '$by, on Open Social P2P. To open it, get OSP ($installGuide), then '
        'paste this code into Invite and follow:',
    '',
    code,
  ].join('\n');
}

/// "Tell someone about OSP".
String aboutOspText() =>
    'I use Open Social P2P: a social network that runs on our own '
    'computers, with no company in the middle. How to get it: $installGuide';

/// Up to two lines and 140 characters of [text], marked when cut.
String firstLines(String text, {int lines = 2, int chars = 140}) {
  final kept = text.trim().split('\n').where((l) => l.trim().isNotEmpty);
  var out = kept.take(lines).map((l) => l.trim()).join('\n');
  final cut = kept.length > lines || out.length > chars;
  if (out.length > chars) out = out.substring(0, chars).trimRight();
  return cut ? '$out…' : out;
}

/// The word in [text] that starts with `osp:`, for saying what's wrong
/// with it; or all of [text] if none does.
String codePart(String text) {
  final at = text.toLowerCase().indexOf('osp:');
  if (at < 0) return text;
  return text.substring(at).split(RegExp(r'\s')).first;
}

/// Hands text to the phone's share sheet. Tests replace it.
@visibleForTesting
Future<void> Function(String text) shareSheet = (text) =>
    SharePlus.instance.share(ShareParams(text: text));

/// Shares [text] through the phone's share sheet.
Future<void> shareText(String text) => shareSheet(text);

/// Shares a profile: the owner's, or anyone's the node holds.
Future<void> shareProfile(
  BuildContext context,
  HomeNode node, {
  required String identity,
  required String name,
}) async {
  final messenger = ScaffoldMessenger.of(context);
  try {
    final code = await node.app.codeForPerson(identity: identity);
    await shareText(
      personShareText(
        name: name,
        code: code,
        own: identity == node.paired?.identity,
      ),
    );
  } catch (e) {
    messenger.showSnackBar(
      SnackBar(content: Text("Couldn't make a code: ${describeError(e)}")),
    );
  }
}

/// Shares a post, by its author's invite code.
Future<void> sharePost(BuildContext context, HomeNode node, Post post) async {
  final messenger = ScaffoldMessenger.of(context);
  try {
    final code = await node.app.codeForPost(
      postId: post.id,
      author: post.authorIdentity,
    );
    await shareText(
      postShareText(author: post.author, text: post.text, code: code),
    );
  } catch (e) {
    messenger.showSnackBar(
      SnackBar(content: Text("Couldn't make a code: ${describeError(e)}")),
    );
  }
}

/// Opens an OSP code: a person's is followed, once the owner says so; a
/// post's opens its thread, if the node holds the post.
Future<void> openOspCode(
  BuildContext context,
  HomeNode node,
  SharedCode code,
) async {
  final navigator = Navigator.of(context);
  final messenger = ScaffoldMessenger.of(context);
  void say(String text) =>
      messenger.showSnackBar(SnackBar(content: Text(text)));

  final postId = code.post;
  if (postId != null) {
    try {
      final post = await node.app.showPost(id: postId);
      navigator.push(
        MaterialPageRoute(
          builder: (_) => ThreadScreen(node: node, post: post),
        ),
      );
      return;
    } catch (_) {
      // Not on this node: offer to follow its author, below.
    }
  }

  if (code.actor == node.paired?.identity) {
    say(
      postId == null
          ? "That's your own code."
          : "That post of yours isn't on your node any more.",
    );
    return;
  }
  final following = await node.app.following();
  final followed = following.where((f) => f.identity == code.actor);
  if (followed.isNotEmpty) {
    if (postId != null) {
      say(
        "That post isn't on your node yet. It comes with the next sync with "
        'its author.',
      );
      return;
    }
    final person = followed.first;
    navigator.push(
      MaterialPageRoute(
        builder: (_) => ProfileScreen(
          node: node,
          identity: person.identity,
          name: person.name,
        ),
      ),
    );
    return;
  }
  if (!context.mounted) return;
  final follow = await showDialog<bool>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text(postId == null ? 'Follow them?' : 'Follow its author?'),
      content: Text(
        postId == null
            ? 'Your node asks theirs for their profile and public posts, '
                  'and keeps getting new ones.'
            : "This post is by someone you don't follow, so it isn't on "
                  'your node. Follow them to get their public posts, this '
                  'one among them.',
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context, false),
          child: const Text('Not now'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context, true),
          child: const Text('Follow'),
        ),
      ],
    ),
  );
  if (follow != true) return;
  try {
    await node.app.follow(ticket: code.invite);
    final overTor = node.status?.overTor ?? false;
    say(
      overTor
          ? 'Following. Your node is reaching theirs, which can take a few '
                'minutes. Their posts appear on Home after that.'
          : 'Following. Their posts appear on Home shortly.',
    );
    await node.refresh(sync: true);
  } catch (e) {
    say("Couldn't follow: ${describeError(e)}");
  }
}
