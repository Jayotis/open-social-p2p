import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';
import 'package:osp_app/waiting_deletes.dart';
import 'package:osp_app/widgets/post_card.dart';

/// The Rust side, as far as deleting goes.
class FakeApp implements OspApp {
  final deleted = <String>[];
  var disconnects = 0;

  /// While set, a delete never reaches the node: as when the phone
  /// suspends the app with it on its way.
  Completer<void>? stuck;

  /// While set, a delete fails with this.
  Object? fails;

  @override
  Future<void> deletePost({required String id}) async {
    if (stuck case final stuck?) await stuck.future;
    if (fails case final fails?) throw fails;
    deleted.add(id);
  }

  @override
  Future<void> disconnect() async => disconnects++;

  @override
  Future<void> resumed() async {}

  @override
  Future<List<Post>> feed({required int limit}) async => [];

  @override
  Future<NodeStatus> status() async => throw Exception('not in this test');

  @override
  SavedFeed? savedFeed() => null;

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

Post post(
  String text, {
  String? deletable,
  String? resharedBy,
  int? resharers,
}) => Post(
  id: 'id-$text',
  author: 'Ann',
  authorIdentity: 'id-Ann',
  published: '2026-01-01T00:00:00Z',
  text: text,
  media: const [],
  resharedBy: resharedBy,
  resharerHosts: false,
  deletable: deletable,
  resharers: resharers,
);

void main() {
  late Directory root;
  late FakeApp app;
  late HomeNode node;

  /// A node on this phone, as when the app starts: kept deletes come back.
  HomeNode phoneNode() => HomeNode.forTests(
    app,
    UploadsFolder(
      Directory('${root.path}/uploads'),
      pickerDir: Directory('${root.path}/cache'),
    ),
    WaitingDeletes(File('${root.path}/waiting-deletes.json')),
  );

  setUp(() {
    root = Directory.systemTemp.createTempSync('delete_test');
    app = FakeApp();
    node = phoneNode();
    node.reachability = Reachability.online;
    node.posts = [
      post('mine', deletable: 'id-mine', resharers: 2),
      post('theirs'),
      post('reshared', deletable: 'my-reshare', resharedBy: 'Ann'),
    ];
  });
  tearDown(() => root.deleteSync(recursive: true));

  Future<void> open(WidgetTester tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ListenableBuilder(
            listenable: node,
            builder: (context, _) => ListView(
              children: [
                for (final p in node.visiblePosts)
                  PostCard(node: node, post: p),
              ],
            ),
          ),
        ),
      ),
    );
  }

  /// Taps Delete on the card saying [text], and confirms with [confirm].
  Future<void> delete(WidgetTester tester, String text, String confirm) async {
    final card = find.ancestor(
      of: find.text(text),
      matching: find.byType(PostCard),
    );
    await tester.tap(find.descendant(of: card, matching: find.text('Delete')));
    await tester.pumpAndSettle();
    await tester.tap(find.text(confirm));
    await tester.pumpAndSettle();
  }

  testWidgets('only your own posts and reshares offer to be deleted', (
    tester,
  ) async {
    await open(tester);
    expect(find.text('Delete'), findsNWidgets(2));
  });

  testWidgets('your own post says how many reshared it', (tester) async {
    await open(tester);
    expect(find.text('Reshared by 2 people'), findsOneWidget);
  });

  testWidgets('a delete waits a minute, and Undo keeps the post', (
    tester,
  ) async {
    await open(tester);
    await delete(tester, 'mine', 'Delete your post');
    expect(find.text('Deleting your post in a minute.'), findsOneWidget);
    expect(find.text('mine'), findsNothing);
    expect(app.deleted, isEmpty);

    await tester.tap(find.text('Undo'));
    await tester.pump();
    expect(find.text('mine'), findsOneWidget);
    await tester.pump(const Duration(minutes: 2));
    expect(app.deleted, isEmpty);
  });

  testWidgets('once its minute is up, the delete is sent', (tester) async {
    await open(tester);
    await delete(tester, 'reshared', 'Delete your reshare');
    await tester.pump(const Duration(seconds: 59));
    expect(app.deleted, isEmpty);
    await tester.pump(const Duration(seconds: 2));
    expect(app.deleted, ['my-reshare']);
    await tester.pumpAndSettle();
    expect(find.text('reshared'), findsNothing);
    expect(find.text('mine'), findsOneWidget);
  });

  testWidgets('leaving the app sends a waiting delete at once', (tester) async {
    node.paired = const NodeInfo(name: 'Ann', identity: 'ann', paired: '');
    await open(tester);
    await delete(tester, 'mine', 'Delete your post');
    node.didChangeAppLifecycleState(AppLifecycleState.paused);
    await tester.pump();
    expect(app.deleted, ['id-mine']);
    expect(app.disconnects, 1, reason: 'then the connection goes');
    // Nothing is left to send when the minute would have been up.
    await tester.pump(const Duration(minutes: 2));
    expect(app.deleted, ['id-mine']);
  });

  const ann = NodeInfo(name: 'Ann', identity: 'ann', paired: '');

  testWidgets(
    'a delete the phone stopped on its way is sent at the next start',
    (tester) async {
      node.paired = ann;
      await open(tester);
      await delete(tester, 'mine', 'Delete your post');
      app.stuck = Completer();
      node.didChangeAppLifecycleState(AppLifecycleState.paused);
      await tester.pump();
      expect(app.deleted, isEmpty);

      // Back before it got there: it isn't sent twice.
      node.didChangeAppLifecycleState(AppLifecycleState.resumed);
      await tester.pump();
      node.didChangeAppLifecycleState(AppLifecycleState.paused);
      await tester.pump();

      // The phone stopped the app. When it starts again, the delete goes.
      final stopped = node;
      app.stuck = null;
      node = phoneNode()..paired = ann;
      node.didChangeAppLifecycleState(AppLifecycleState.resumed);
      await tester.pump(Duration.zero);
      expect(app.deleted, ['id-mine']);
      // And once only.
      node.didChangeAppLifecycleState(AppLifecycleState.paused);
      node.didChangeAppLifecycleState(AppLifecycleState.resumed);
      await tester.pump(const Duration(minutes: 2));
      expect(app.deleted, ['id-mine']);
      stopped.dispose();
      node.dispose();
    },
  );

  testWidgets('a waiting delete is still hidden, and waits out its minute, '
      'after a restart', (tester) async {
    await open(tester);
    await delete(tester, 'mine', 'Delete your post');
    await tester.pump(const Duration(seconds: 20));
    // The phone stopped the app, its timers with it.
    node.dispose();
    node = phoneNode()
      ..paired = ann
      ..posts = node.posts;
    node.didChangeAppLifecycleState(AppLifecycleState.resumed);
    await tester.pump();
    expect(node.visiblePosts.map((p) => p.text), isNot(contains('mine')));
    await tester.pump(const Duration(seconds: 30));
    expect(app.deleted, isEmpty);
    await tester.pump(const Duration(seconds: 15));
    expect(app.deleted, ['id-mine']);
    node.dispose();
  });

  testWidgets('one undone, or one the owner was told failed, is forgotten', (
    tester,
  ) async {
    await open(tester);
    await delete(tester, 'mine', 'Delete your post');
    await tester.tap(find.text('Undo'));
    await tester.pump();
    app.fails = Exception('the node said no');
    await delete(tester, 'reshared', 'Delete your reshare');
    await tester.pump(const Duration(minutes: 2));
    expect(app.deleted, isEmpty);
    app.fails = null;
    expect(
      WaitingDeletes(File('${root.path}/waiting-deletes.json')).read(),
      isEmpty,
    );
  });
}
