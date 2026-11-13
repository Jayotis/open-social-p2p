import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/reply_requests_screen.dart';
import 'package:osp_app/screens/thread_screen.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';

/// The Rust side, as far as replies go (ADR 0028).
class FakeApp implements OspApp {
  Thread kept = const Thread(policy: 'network', replies: [], more: false);
  List<SentReply> sent = [];
  List<Reply> requests = [];
  final replied = <(String, String)>[];
  final policies = <(String, String)>[];
  final hidden = <(String, bool)>[];
  final choices = <(String, String?)>[];
  final answers = <(String, String)>[];

  @override
  Future<Thread> thread({required String postId, required int after}) async =>
      kept;

  @override
  Future<List<SentReply>> sentReplies() async => sent;

  @override
  Future<SentReply> reply({
    required String postId,
    required String text,
  }) async {
    replied.add((postId, text));
    final s = sentReply(text, 'awaiting_approval', post: postId);
    sent = [s, ...sent];
    return s;
  }

  @override
  Future<void> setReplyPolicy({
    required String postId,
    required String policy,
  }) async {
    policies.add((postId, policy));
    kept = Thread(policy: policy, replies: kept.replies, more: false);
  }

  @override
  Future<void> hideReply({required String reply, required bool hidden}) async {
    this.hidden.add((reply, hidden));
  }

  @override
  Future<void> setReplier({required String identity, String? choice}) async {
    choices.add((identity, choice));
  }

  @override
  Future<List<Reply>> replyRequests() async => requests;

  @override
  Future<void> answerReply({
    required String reply,
    required String answer,
  }) async {
    answers.add((reply, answer));
    requests = requests.where((r) => r.id != reply).toList();
  }

  @override
  Future<NodeStatus> status() async => NodeStatus(
    name: 'Kim',
    version: '0',
    uptimeSecs: 60,
    follows: 1,
    followers: 1,
    devices: 1,
    relayConnected: true,
    problems: const [],
    path: 'direct',
    trustRequests: 0,
    replyRequests: requests.length,
    overTor: false,
  );

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

Post post(String author, {String text = 'A walk by the river'}) => Post(
  id: 'post-$author',
  author: author,
  authorIdentity: 'id-$author',
  published: '2026-01-01T00:00:00Z',
  text: text,
  media: const [],
  resharerHosts: false,
);

Reply reply(String name, String text, {String state = 'published'}) => Reply(
  id: 'reply-$name',
  post: 'post-Kim',
  identity: 'id-$name',
  name: name,
  text: text,
  published: '2026-01-01T00:00:00Z',
  state: state,
  why: state == 'request' ? "you don't follow them" : null,
  seq: 1,
);

SentReply sentReply(
  String text,
  String status, {
  String post = 'post-Ann',
  String? error,
  int? nextTrySecs,
}) => SentReply(
  id: 'sent-$text',
  post: post,
  creator: 'id-Ann',
  text: text,
  published: '2026-01-01T00:00:00Z',
  status: status,
  error: error,
  nextTrySecs: nextTrySecs,
);

void main() {
  late Directory root;
  late FakeApp app;
  late HomeNode node;

  setUp(() {
    root = Directory.systemTemp.createTempSync('replies_test');
    app = FakeApp();
    final uploads = UploadsFolder(
      Directory('${root.path}/uploads'),
      pickerDir: Directory('${root.path}/cache'),
    );
    node = HomeNode.forTests(app, uploads)
      ..paired = const NodeInfo(
        name: 'Kim',
        identity: 'id-Kim',
        paired: '2026-01-01T00:00:00Z',
      )
      ..reachability = Reachability.online;
  });
  tearDown(() => root.deleteSync(recursive: true));

  Future<void> open(WidgetTester tester, Widget screen) async {
    await tester.binding.setSurfaceSize(const Size(800, 1600));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.pumpWidget(MaterialApp(home: screen));
    await tester.pumpAndSettle();
  }

  testWidgets('on their own post, the owner sets the policy, hides, blocks', (
    tester,
  ) async {
    app.kept = Thread(
      policy: 'network',
      replies: [
        reply('Alice', 'Lovely spot'),
        reply('Jimmy', 'Buy my stuff', state: 'hidden'),
      ],
      more: false,
    );
    await open(tester, ThreadScreen(node: node, post: post('Kim')));

    expect(find.text('Who can reply'), findsOne);
    expect(find.textContaining('Anyone else waits among'), findsOne);
    expect(find.text('Lovely spot'), findsOne);
    expect(find.textContaining('hidden'), findsOne);

    await tester.tap(find.text('Circle'));
    await tester.pumpAndSettle();
    expect(app.policies, [('post-Kim', 'circle')]);
    expect(find.textContaining('Nobody else can.'), findsOne);

    await tester.tap(find.byTooltip('Reply options').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Hide from the thread'));
    await tester.pumpAndSettle();
    expect(app.hidden, [('reply-Alice', true)]);

    await tester.tap(find.byTooltip('Reply options').last);
    await tester.pumpAndSettle();
    expect(find.text('Show again'), findsOne);
    await tester.tap(find.text('Block them'));
    await tester.pumpAndSettle();
    expect(find.text('Block Jimmy?'), findsOne);
    await tester.tap(find.widgetWithText(FilledButton, 'Block'));
    await tester.pumpAndSettle();
    expect(app.choices, [('id-Jimmy', 'block')]);
  });

  testWidgets('turning replies off hides the reply box', (tester) async {
    app.kept = const Thread(policy: 'off', replies: [], more: false);
    await open(tester, ThreadScreen(node: node, post: post('Kim')));
    expect(find.text('No replies yet.'), findsOne);
    expect(find.byType(TextField), findsNothing);
  });

  testWidgets("replying to someone else's post shows where it stands", (
    tester,
  ) async {
    app.kept = Thread(
      policy: 'network',
      replies: [reply('Alice', 'Lovely spot')],
      more: false,
    );
    app.sent = [
      sentReply('Earlier try', 'sending', nextTrySecs: 300),
      sentReply('Bad one', 'refused', error: 'the text is too long'),
      sentReply('Elsewhere', 'sending', post: 'post-Other'),
    ];
    await open(tester, ThreadScreen(node: node, post: post('Ann')));

    // Others' posts don't show a policy.
    expect(find.text('Who can reply'), findsNothing);
    expect(find.text('Your replies'), findsOne);
    expect(
      find.textContaining("Ann's node didn't answer. Trying again in 5 min"),
      findsOne,
    );
    expect(find.text("Refused by Ann's node: the text is too long."), findsOne);
    expect(find.text('Elsewhere'), findsNothing);
    // Not the owner's post, and not their reply: nothing to choose.
    expect(find.byTooltip('Reply options'), findsNothing);

    final send = find.widgetWithIcon(IconButton, Icons.send);
    expect(tester.widget<IconButton>(send).onPressed, isNull);
    await tester.enterText(find.byType(TextField), 'x' * 4001);
    await tester.pump();
    expect(find.text('Too long: 4001 of 4000 bytes'), findsOne);
    expect(tester.widget<IconButton>(send).onPressed, isNull);

    await tester.enterText(find.byType(TextField), '  Thanks for this  ');
    await tester.pump();
    await tester.tap(send);
    await tester.pumpAndSettle();
    expect(app.replied, [('post-Ann', 'Thanks for this')]);
    expect(find.textContaining('Waiting for Ann to look at it'), findsWidgets);
    expect(find.text('Thanks for this'), findsOne);
  });

  testWidgets('a waiting reply says why and is published or blocked', (
    tester,
  ) async {
    node.posts = [post('Kim', text: 'A walk by the river\nmore')];
    app.requests = [
      reply('Jimmy', 'Nice', state: 'request'),
      reply('Sam', 'Spam', state: 'request'),
    ];
    await open(tester, ReplyRequestsScreen(node: node));

    expect(find.textContaining("you don't follow them"), findsNWidgets(2));
    expect(find.text('On your post: "A walk by the river"'), findsNWidgets(2));

    await tester.tap(find.text('Publish and allow').first);
    await tester.pumpAndSettle();
    expect(app.answers, [('reply-Jimmy', 'publish_and_allow')]);
    expect(find.text('Nice'), findsNothing);

    await tester.tap(find.text('Block'));
    await tester.pumpAndSettle();
    expect(find.text('Block Sam?'), findsOne);
    await tester.tap(find.widgetWithText(FilledButton, 'Block'));
    await tester.pumpAndSettle();
    expect(app.answers.last, ('reply-Sam', 'block'));
    expect(find.text('No replies waiting.'), findsOne);
  });
}
