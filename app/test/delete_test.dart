import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';
import 'package:osp_app/widgets/post_card.dart';

/// The Rust side, as far as deleting goes.
class FakeApp implements OspApp {
  final deleted = <String>[];
  var disconnects = 0;

  @override
  Future<void> deletePost({required String id}) async => deleted.add(id);

  @override
  Future<void> disconnect() async => disconnects++;

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

Post post(String text, {String? deletable, String? resharedBy}) => Post(
  id: 'id-$text',
  author: 'Ann',
  published: '2026-01-01T00:00:00Z',
  text: text,
  media: const [],
  resharedBy: resharedBy,
  resharerHosts: false,
  deletable: deletable,
);

void main() {
  late Directory root;
  late FakeApp app;
  late HomeNode node;

  setUp(() {
    root = Directory.systemTemp.createTempSync('delete_test');
    app = FakeApp();
    node = HomeNode.forTests(
      app,
      UploadsFolder(
        Directory('${root.path}/uploads'),
        pickerDir: Directory('${root.path}/cache'),
      ),
    );
    node.reachability = Reachability.online;
    node.posts = [
      post('mine', deletable: 'id-mine'),
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
}
