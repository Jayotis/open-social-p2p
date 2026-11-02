import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/feed_screen.dart';
import 'package:osp_app/screens/search_screen.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';

/// The Rust side, as far as searching goes.
class FakeApp implements OspApp {
  final asked = <String>[];

  /// Answers each search, by what was asked for.
  Future<Found> Function(String words) answer = (_) async =>
      const Found(people: [], posts: []);

  @override
  Future<Found> search({required String words, required int limit}) {
    asked.add(words);
    return answer(words);
  }

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

Post post(String text) => Post(
  id: 'id-$text',
  author: 'Bob',
  published: '2026-01-01T00:00:00Z',
  text: text,
  media: const [],
  resharerHosts: false,
);

FoundPerson person(String name, {String? summary, bool followed = false}) =>
    FoundPerson(
      identity: 'id-$name',
      name: name,
      summary: summary,
      followed: followed,
    );

void main() {
  late Directory root;
  late FakeApp app;
  late HomeNode node;

  setUp(() {
    root = Directory.systemTemp.createTempSync('search_test');
    app = FakeApp();
    final uploads = UploadsFolder(
      Directory('${root.path}/uploads'),
      pickerDir: Directory('${root.path}/cache'),
    );
    node = HomeNode.forTests(app, uploads);
  });
  tearDown(() => root.deleteSync(recursive: true));

  Future<void> open(WidgetTester tester) =>
      tester.pumpWidget(MaterialApp(home: SearchScreen(node: node)));

  Future<void> searchFor(WidgetTester tester, String words) async {
    await tester.enterText(find.byType(TextField), words);
    await tester.testTextInput.receiveAction(TextInputAction.search);
    await tester.pump();
  }

  testWidgets('says what it looks through, before anything is typed', (
    tester,
  ) async {
    await open(tester);
    expect(find.textContaining('nobody sees'), findsOneWidget);
    // Nothing to look for: nothing asked.
    await searchFor(tester, '   ');
    expect(app.asked, isEmpty);
  });

  testWidgets('shows the people and posts found, in order', (tester) async {
    app.answer = (_) async => Found(
      people: [
        person('Bob Baker', summary: 'Makes bread', followed: true),
        person('Bob Stranger'),
      ],
      posts: [post('Bread at last'), post('More bread')],
    );
    await open(tester);
    await searchFor(tester, ' bread ');
    await tester.pumpAndSettle();

    expect(app.asked, ['bread']);
    expect(find.text('People'), findsOneWidget);
    expect(find.text('Bob Baker'), findsOneWidget);
    expect(find.text('Makes bread'), findsOneWidget);
    expect(find.text('Following'), findsOneWidget);
    expect(find.text('Bob Stranger'), findsOneWidget);
    expect(find.text('Posts'), findsOneWidget);
    final first = tester.getTopLeft(find.text('Bread at last'));
    final second = tester.getTopLeft(find.text('More bread'));
    expect(first.dy, lessThan(second.dy));
  });

  testWidgets('leaves out a heading with nothing under it', (tester) async {
    app.answer = (_) async => Found(people: const [], posts: [post('Only me')]);
    await open(tester);
    await searchFor(tester, 'only');
    await tester.pumpAndSettle();
    expect(find.text('People'), findsNothing);
    expect(find.text('Posts'), findsOneWidget);
  });

  testWidgets('marks your own profile', (tester) async {
    node.paired = const NodeInfo(
      identity: 'id-Me',
      name: 'Me',
      paired: '2026-01-01T00:00:00Z',
    );
    app.answer = (_) async => Found(people: [person('Me')], posts: const []);
    await open(tester);
    await searchFor(tester, 'me');
    await tester.pumpAndSettle();
    expect(find.text('You'), findsOneWidget);
  });

  testWidgets('says when nothing matches', (tester) async {
    await open(tester);
    await searchFor(tester, 'nothing');
    await tester.pumpAndSettle();
    expect(find.text('Nothing on your node matches.'), findsOneWidget);
  });

  testWidgets('says why, when the node cannot be asked', (tester) async {
    app.answer = (_) async =>
        throw AnyhowException("Couldn't reach your home node");
    await open(tester);
    await searchFor(tester, 'bread');
    await tester.pumpAndSettle();
    expect(find.textContaining("Couldn't reach"), findsOneWidget);
  });

  testWidgets('an answer to an earlier search never replaces a later one', (
    tester,
  ) async {
    final slow = Completer<Found>();
    app.answer = (words) async => words == 'first'
        ? await slow.future
        : Found(people: const [], posts: [post('Second answer')]);
    await open(tester);
    await searchFor(tester, 'first');
    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    await searchFor(tester, 'second');
    await tester.pumpAndSettle();
    expect(find.text('Second answer'), findsOneWidget);

    slow.complete(Found(people: const [], posts: [post('First answer')]));
    await tester.pumpAndSettle();
    expect(find.text('First answer'), findsNothing);
    expect(find.text('Second answer'), findsOneWidget);
  });

  testWidgets('the search button works like the keyboard does', (tester) async {
    await open(tester);
    await tester.enterText(find.byType(TextField), 'bread');
    await tester.tap(find.byTooltip('Search'));
    await tester.pumpAndSettle();
    expect(app.asked, ['bread']);
  });

  testWidgets('opens from the feed', (tester) async {
    await tester.pumpWidget(MaterialApp(home: FeedScreen(node: node)));
    await tester.tap(find.byTooltip('Search your node'));
    await tester.pumpAndSettle();
    expect(find.byType(SearchScreen), findsOneWidget);
  });
}
