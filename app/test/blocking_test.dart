import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/blocked_screen.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';
import 'package:osp_app/widgets/post_card.dart';

import 'plain_words.dart';
import 'shell_test.dart' show nodeStatus;

/// The node, as far as blocking goes.
class FakeApp implements OspApp {
  final blocked = <(String, String)>[];
  final unblocked = <(String, String)>[];
  List<BlockedThing> held = [];

  @override
  Future<void> block({required String kind, required String id}) async =>
      blocked.add((kind, id));

  @override
  Future<void> unblock({required String kind, required String id}) async {
    unblocked.add((kind, id));
    held = held.where((b) => b.id != id).toList();
  }

  @override
  Future<List<BlockedThing>> blocks() async => held;

  @override
  Future<List<Post>> feed({required int limit}) async => [];

  @override
  Future<NodeStatus> status() async => nodeStatus();

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

Post post(String author) => Post(
  regions: const [],
  id: 'post-1',
  author: author,
  authorIdentity: 'id-$author',
  published: '2026-01-01T00:00:00Z',
  text: 'A walk by the river',
  media: const [],
  resharerHosts: false,
  liked: false,
  opened: true,
);

void main() {
  late Directory root;
  late FakeApp app;
  late HomeNode node;

  setUp(() {
    root = Directory.systemTemp.createTempSync('blocking_test');
    app = FakeApp();
    node = HomeNode.forTests(
      app,
      UploadsFolder(
        Directory('${root.path}/uploads'),
        pickerDir: Directory('${root.path}/cache'),
      ),
    );
    node.paired = const NodeInfo(
      name: 'Alice',
      identity: 'id-Alice',
      paired: '2026-11-14T00:00:00Z',
    );
    node.reachability = Reachability.online;
  });
  tearDown(() {
    node.dispose();
    root.deleteSync(recursive: true);
  });

  Future<void> openMenu(WidgetTester tester, Post shown) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: PostCard(node: node, post: shown),
          ),
        ),
      ),
    );
    await tester.tap(find.byTooltip('More'));
    await tester.pumpAndSettle();
  }

  testWidgets('blocking a post\'s author asks first, then tells the node', (
    tester,
  ) async {
    await openMenu(tester, post('Kim'));
    expect(find.text('Hide this post'), findsOne);
    await tester.tap(find.text('Block Kim'));
    await tester.pumpAndSettle();
    expect(find.text('Block Kim?'), findsOne);
    expect(find.textContaining("Kim isn't told"), findsOne);
    expectPlainWords(tester);

    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(app.blocked, isEmpty);

    await tester.tap(find.byTooltip('More'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Block Kim'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Block'));
    await tester.pumpAndSettle();
    expect(app.blocked, [('account', 'id-Kim')]);
    expect(find.text('Kim is blocked.'), findsOne);
  });

  testWidgets('hiding one post blocks that post', (tester) async {
    await openMenu(tester, post('Kim'));
    await tester.tap(find.text('Hide this post'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Hide it'));
    await tester.pumpAndSettle();
    expect(app.blocked, [('post', 'post-1')]);
  });

  testWidgets('your own post has neither', (tester) async {
    await openMenu(tester, post('Alice'));
    expect(find.text('Hide this post'), findsNothing);
    expect(find.textContaining('Block'), findsNothing);
  });

  testWidgets('Blocked lists what was blocked, and unblocks it', (
    tester,
  ) async {
    app.held = [
      BlockedThing(
        kind: 'account',
        id: 'id-Kim',
        name: 'Kim',
        since: DateTime.now()
            .subtract(const Duration(hours: 2))
            .toUtc()
            .toIso8601String(),
      ),
      const BlockedThing(
        kind: 'post',
        id: 'post-1',
        since: '2026-11-01T00:00:00Z',
      ),
    ];
    await tester.pumpWidget(MaterialApp(home: BlockedScreen(node: node)));
    await tester.pumpAndSettle();
    expect(find.text('Kim'), findsOne);
    expect(find.text('A post'), findsOne);
    expect(find.textContaining('Blocked 2 h ago'), findsOne);
    expectPlainWords(tester);

    await tester.tap(find.text('Unblock').first);
    await tester.pumpAndSettle();
    expect(app.unblocked, [('account', 'id-Kim')]);
    expect(find.text('Kim'), findsNothing);
    expect(find.textContaining('Kim is unblocked'), findsOne);
  });
}
