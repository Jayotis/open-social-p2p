import 'dart:async';

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
Future<void> Function(String text, String subject) shareSheet =
    (text, subject) =>
        SharePlus.instance.share(ShareParams(text: text, subject: subject));

/// Shares [text] through the phone's share sheet. [subject] is the email
/// subject line, where the app the person picks has one.
Future<void> shareText(String text, {String subject = 'Open Social P2P'}) =>
    shareSheet(text, subject);

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
      subject: identity == node.paired?.identity
          ? 'Follow me on Open Social P2P'
          : '${name.isEmpty ? 'Someone' : name} on Open Social P2P',
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
      subject:
          'A post by ${post.author.isEmpty ? 'someone' : post.author} '
          'on Open Social P2P',
    );
  } catch (e) {
    messenger.showSnackBar(
      SnackBar(content: Text("Couldn't make a code: ${describeError(e)}")),
    );
  }
}

/// Opens an OSP code: a person's is followed, once the owner says so; a
/// post's opens its thread, fetched from its author's node if need be.
Future<void> openOspCode(
  BuildContext context,
  HomeNode node,
  SharedCode code,
) async {
  final navigator = Navigator.of(context);
  final messenger = ScaffoldMessenger.of(context);
  void say(String text) =>
      messenger.showSnackBar(SnackBar(content: Text(text)));

  // A post opens as the node holds it, or else is fetched from its
  // author's node and kept as an opened post (ADR 0039 §4).
  if (code.post != null) {
    if (!context.mounted) return;
    final root = Navigator.of(context, rootNavigator: true);
    // Over Tor, its author's node can take a minute or two to answer.
    unawaited(
      showDialog<void>(
        context: context,
        barrierDismissible: false,
        builder: (_) => const AlertDialog(
          content: Row(
            children: [
              CircularProgressIndicator(),
              SizedBox(width: 16),
              Expanded(child: Text("Opening the post from its author's node…")),
            ],
          ),
        ),
      ),
    );
    try {
      final post = await node.app.openPost(code: code.code);
      root.pop();
      navigator.push(
        MaterialPageRoute(
          builder: (_) => ThreadScreen(node: node, post: post),
        ),
      );
    } catch (e) {
      root.pop();
      say("Couldn't open that post: ${describeError(e)}");
    }
    return;
  }

  if (code.actor == node.paired?.identity) {
    say("That's your own code.");
    return;
  }
  final following = await node.app.following();
  final followed = following.where((f) => f.identity == code.actor);
  if (followed.isNotEmpty) {
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
      title: const Text('Follow them?'),
      content: const Text(
        'Your node asks theirs for their profile and public posts, '
        'and keeps getting new ones.',
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
