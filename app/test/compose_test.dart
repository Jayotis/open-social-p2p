import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/compose_screen.dart';
import 'package:osp_app/screens/feed_screen.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';

/// The Rust side, as far as posting goes. Each post or continue gets a
/// stream the test feeds.
class FakeApp implements OspApp {
  UnsentPost? pending;
  final sends = <StreamController<PostProgress>>[];
  var discards = 0;
  Object? discardError;

  StreamController<PostProgress> get sending => sends.last;

  @override
  UnsentPost? pendingPost() => pending;

  @override
  Stream<PostProgress> continuePost() {
    sends.add(StreamController());
    return sending.stream;
  }

  @override
  Stream<PostProgress> post({
    required String text,
    required List<String> files,
  }) => continuePost();

  @override
  Future<void> discardPost() async {
    discards++;
    pending = null;
    if (discardError case final e?) throw e;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

PostProgress progress(
  PostStage stage, {
  int sent = 0,
  int total = 0,
  String? message,
  bool kept = false,
  bool noContact = false,
}) => PostProgress(
  stage: stage,
  sent: sent,
  total: total,
  message: message,
  kept: kept,
  noContact: noContact,
);

void main() {
  late Directory root;
  late FakeApp app;
  late HomeNode node;
  late UnsentPost unsent;

  setUp(() {
    root = Directory.systemTemp.createTempSync('compose_test');
    final uploads = UploadsFolder(
      Directory('${root.path}/uploads'),
      pickerDir: Directory('${root.path}/cache'),
    );
    final clip = File('${root.path}/uploads/post1/0/clip.mp4')
      ..createSync(recursive: true)
      ..writeAsBytesSync(List.filled(1000, 0));
    unsent = UnsentPost(
      text: 'Our furnace',
      files: [UnsentFile(path: clip.path, size: 1000)],
      started: DateTime.now().toUtc().toIso8601String(),
    );
    app = FakeApp()..pending = unsent;
    node = HomeNode.forTests(app, uploads);
  });
  tearDown(() => root.deleteSync(recursive: true));

  /// Opens the compose screen from a screen below it, carrying on with the
  /// unsent post.
  Future<void> openUnsent(WidgetTester tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Builder(
            builder: (context) => TextButton(
              onPressed: () => Navigator.of(context).push(
                MaterialPageRoute(
                  builder: (_) => ComposeScreen(node: node, unsent: unsent),
                ),
              ),
              child: const Text('below'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('below'));
    await settle(tester);
  }

  testWidgets('an unsent post carries on, waits, and is posted', (
    tester,
  ) async {
    await openUnsent(tester);
    expect(app.sends, hasLength(1), reason: 'carried on at once');
    expect(find.text('Our furnace'), findsOneWidget);

    app.sending.add(progress(PostStage.sending, sent: 500, total: 1000));
    await settle(tester);
    expect(find.textContaining('Sending to your home node: 50%'), findsOne);
    expect(find.text('Cancel'), findsOneWidget);

    app.sending.add(progress(PostStage.waiting));
    await settle(tester);
    expect(find.textContaining('Waiting for your home node… 50%'), findsOne);

    app.sending.add(progress(PostStage.processing, sent: 1000, total: 1000));
    await settle(tester);
    expect(find.textContaining('is preparing your post'), findsOneWidget);
    expect(find.text('Cancel'), findsNothing, reason: 'too late to cancel');

    app.pending = null;
    app.sending.add(progress(PostStage.done, message: 'post-id'));
    await settle(tester, files: true);
    expect(find.text('Posted.'), findsOneWidget);
    expect(find.byType(ComposeScreen), findsNothing);
    expect(
      Directory('${root.path}/uploads/post1').existsSync(),
      isFalse,
      reason: "the post's files are deleted",
    );
  });

  testWidgets('a post that gave up says why, is kept, and can try again', (
    tester,
  ) async {
    await openUnsent(tester);
    app.sending.add(
      progress(
        PostStage.failed,
        message: 'no contact',
        kept: true,
        noContact: true,
      ),
    );
    await settle(tester);
    expect(
      find.text(
        "Couldn't post: your home computer isn't answering: is it on and online?",
      ),
      findsOneWidget,
    );
    expect(find.textContaining('kept on this phone'), findsOneWidget);

    await tester.tap(find.text('Try again'));
    await settle(tester);
    expect(app.sends, hasLength(2));
    expect(find.text('Try again'), findsNothing);
    expect(find.text('Cancel'), findsOneWidget);
  });

  testWidgets('Cancel discards the post after asking', (tester) async {
    await openUnsent(tester);
    app.sending.add(progress(PostStage.sending, sent: 100, total: 1000));
    await settle(tester);

    await tester.tap(find.text('Cancel'));
    await settle(tester);
    await tester.tap(find.text('Keep it'));
    await settle(tester);
    expect(app.discards, 0);

    await tester.tap(find.text('Cancel'));
    await settle(tester);
    await tester.tap(find.text('Discard'));
    await settle(tester, files: true);
    expect(app.discards, 1);
    expect(find.text('Post discarded.'), findsOneWidget);
    expect(find.byType(ComposeScreen), findsNothing);
  });

  testWidgets('a post finishing while "Discard?" is open closes both', (
    tester,
  ) async {
    await openUnsent(tester);
    app.sending.add(progress(PostStage.sending, sent: 100, total: 1000));
    await settle(tester);
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(find.text('Discard this post?'), findsOneWidget);

    app.sending.add(progress(PostStage.done, message: 'post-id'));
    await settle(tester);
    expect(find.text('Discard this post?'), findsNothing);
    expect(find.byType(ComposeScreen), findsNothing);
    expect(find.text('below'), findsOneWidget);
    expect(app.discards, 0);
  });

  testWidgets('too late to discard says so', (tester) async {
    await openUnsent(tester);
    app.sending.add(progress(PostStage.failed, message: 'busy', kept: true));
    await settle(tester);
    app.discardError = Exception(
      'your home node is already preparing this post',
    );

    await tester.tap(find.text('Discard'));
    await settle(tester);
    await tester.tap(find.widgetWithText(FilledButton, 'Discard'));
    await settle(tester, files: true);
    expect(find.textContaining('Too late to discard'), findsOneWidget);
    expect(find.byType(ComposeScreen), findsNothing);
  });

  testWidgets('the feed offers to carry on with an unsent post', (
    tester,
  ) async {
    node.unsent = unsent;
    await tester.pumpWidget(MaterialApp(home: FeedScreen(node: node)));
    await settle(tester);
    expect(find.text("A post didn't finish sending"), findsOneWidget);
    expect(find.textContaining('"Our furnace"'), findsOneWidget);

    // Not while it's being sent.
    node.uploading = true;
    await tester.pumpWidget(MaterialApp(home: FeedScreen(node: node)));
    expect(find.text("A post didn't finish sending"), findsNothing);
    node.uploading = false;
    await tester.pumpWidget(MaterialApp(home: FeedScreen(node: node)));

    await tester.tap(find.text('Continue'));
    await settle(tester);
    expect(find.byType(ComposeScreen), findsOneWidget);
    expect(app.sends, hasLength(1));
  });
}

/// Lets screens and dialogs come and go. (Progress bars that never stop
/// moving rule out `pumpAndSettle`.) With [files], first lets file work
/// finish, which takes real time.
Future<void> settle(WidgetTester tester, {bool files = false}) async {
  if (files) {
    await tester.runAsync(() => Future.delayed(Duration(milliseconds: 100)));
  }
  for (var i = 0; i < 5; i++) {
    await tester.pump(const Duration(milliseconds: 200));
  }
}
