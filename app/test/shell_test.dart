import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/activity_screen.dart';
import 'package:osp_app/screens/advanced_screen.dart';
import 'package:osp_app/screens/compose_screen.dart';
import 'package:osp_app/screens/node_summary_screen.dart';
import 'package:osp_app/screens/people_screen.dart';
import 'package:osp_app/screens/search_screen.dart';
import 'package:osp_app/screens/trust_screen.dart';
import 'package:osp_app/shell.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';

import 'plain_words.dart';

/// The Rust side, as far as the app's places go.
class FakeApp implements OspApp {
  NodeStatus status_ = nodeStatus();

  @override
  Future<NodeStatus> status() async => status_;

  @override
  Future<String> invite() async => 'OSP1invite';

  @override
  Future<List<Followed>> following() async => [];

  @override
  Future<TrustList> trustList() async =>
      const TrustList(requests: [], trusted: []);

  @override
  UnsentPost? pendingPost() => null;

  @override
  String deviceId() => 'device-0123456789';

  @override
  String recentLog({required int lines}) => '';

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

NodeStatus nodeStatus({int trustRequests = 0, int replyRequests = 0}) =>
    NodeStatus(
      name: 'Alice',
      version: '0.1.0',
      uptimeSecs: 3600,
      follows: 2,
      followers: 1,
      devices: 1,
      relayConnected: true,
      problems: const ['Tor is still starting'],
      path: 'relay',
      rttMs: 80,
      trustRequests: trustRequests,
      replyRequests: replyRequests,
      overTor: false,
    );

void main() {
  late Directory root;
  late FakeApp app;
  late HomeNode node;

  setUp(() {
    root = Directory.systemTemp.createTempSync('shell_test');
    app = FakeApp();
    node = HomeNode.forTests(
      app,
      UploadsFolder(
        Directory('${root.path}/uploads'),
        pickerDir: Directory('${root.path}/cache'),
      ),
    );
    // Reached, with no posts yet: a fresh node.
    node.paired = const NodeInfo(
      name: 'Alice',
      identity: 'id-Alice',
      paired: '2026-11-14T00:00:00Z',
    );
    node.reachability = Reachability.online;
    node.status = app.status_;
  });
  tearDown(() {
    node.dispose();
    root.deleteSync(recursive: true);
  });

  Future<void> start(WidgetTester tester, Place place) async {
    await tester.pumpWidget(
      MaterialApp(
        key: UniqueKey(),
        home: AppShell(node: node),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(NavigationDestination, _label(place)));
    await tester.pumpAndSettle();
    expectPlainWords(tester);
  }

  /// The way to each destination, as taps: none needs more than two.
  final routes = <String, (List<Finder>, Finder)>{
    'Home': ([_nav('Home')], _title('Home')),
    'Universe': ([_nav('Universe')], _title('Universe')),
    'You': ([_nav('You')], _title('You')),
    'Post': ([find.byTooltip('Post')], find.byType(ComposeScreen)),
    'Activity': ([find.byTooltip('Activity')], find.byType(ActivityScreen)),
    'your node': (
      [_nav('You'), find.text('Your node')],
      find.byType(NodeSummaryScreen),
    ),
  };

  for (final place in Place.values) {
    for (final MapEntry(key: name, value: (taps, lands)) in routes.entries) {
      testWidgets('${_label(place)} reaches $name in ${taps.length} taps', (
        tester,
      ) async {
        expect(taps.length, lessThanOrEqualTo(2));
        await start(tester, place);
        for (final tap in taps) {
          await tester.tap(tap);
          await tester.pumpAndSettle();
        }
        expect(lands, findsOneWidget);
        expectPlainWords(tester);
      });
    }
  }

  testWidgets('your star in the Universe opens your node', (tester) async {
    await start(tester, Place.universe);
    await tester.tap(find.byTooltip('Your node'));
    await tester.pumpAndSettle();
    expect(find.byType(NodeSummaryScreen), findsOneWidget);
    expect(find.text('Your phone reaches it.'), findsOne);
    expect(find.textContaining('reports a problem'), findsOne);
    expectPlainWords(tester);

    // The node's own words for the problem are under Advanced details.
    await tester.tap(find.text('Advanced details'));
    await tester.pumpAndSettle();
    expect(find.byType(AdvancedScreen), findsOneWidget);
    expect(find.textContaining('Tor is still starting'), findsOne);
  });

  testWidgets('search opens from Home and the Universe', (tester) async {
    for (final place in [Place.home, Place.universe]) {
      await start(tester, place);
      await tester.tap(find.byTooltip('Search'));
      await tester.pumpAndSettle();
      expect(find.byType(SearchScreen), findsOneWidget);
    }
  });

  testWidgets('the Activity bell counts what waits, and opens it', (
    tester,
  ) async {
    node.status = nodeStatus(trustRequests: 1, replyRequests: 2);
    app.status_ = node.status!;
    await start(tester, Place.home);
    expect(find.text('3'), findsOne);
    await tester.tap(find.byTooltip('Activity'));
    await tester.pumpAndSettle();
    expect(find.text('Replies waiting for you'), findsOne);
    expect(find.text('People who say they know you'), findsOne);
    await tester.tap(find.text('People who say they know you'));
    await tester.pumpAndSettle();
    expect(find.byType(TrustScreen), findsOneWidget);
    expectPlainWords(tester);
  });

  testWidgets('You leads to invites and people you know', (tester) async {
    await start(tester, Place.you);
    await tester.tap(find.text('Invite and follow'));
    await tester.pumpAndSettle();
    expect(find.byType(PeopleScreen), findsOneWidget);
    expect(find.text('Your invite code'), findsOne);
    expectPlainWords(tester);
  });

  testWidgets('an empty Home points to the Universe', (tester) async {
    await start(tester, Place.home);
    expect(find.textContaining('you follow nobody yet'), findsOne);
    await tester.tap(find.text('Explore the Universe'));
    await tester.pumpAndSettle();
    expect(_title('Universe'), findsOneWidget);
  });

  testWidgets('no posting while the node is unreachable', (tester) async {
    node.reachability = Reachability.offline;
    await start(tester, Place.home);
    await tester.tap(
      find.byTooltip("Can't post while your node is unreachable"),
    );
    await tester.pumpAndSettle();
    expect(find.byType(ComposeScreen), findsNothing);
    expect(find.text("Can't reach your node."), findsOne);
    expectPlainWords(tester);
  });

  testWidgets('the first run: this is you, your node, then the Universe', (
    tester,
  ) async {
    node.firstRun = true;
    await tester.pumpWidget(
      MaterialApp(
        home: ListenableBuilder(
          listenable: node,
          builder: (_, _) => AppShell(node: node),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('This is you'), findsOne);
    expect(find.text('Alice'), findsOne);
    expectPlainWords(tester);
    await tester.tap(find.text('Next'));
    await tester.pumpAndSettle();
    expect(find.text('Your home computer is your node'), findsOne);
    expectPlainWords(tester);
    await tester.tap(find.text('Open the Universe'));
    await tester.pumpAndSettle();
    expect(node.firstRun, isFalse);
    expect(_title('Universe'), findsOneWidget);
  });
}

String _label(Place place) => switch (place) {
  Place.home => 'Home',
  Place.universe => 'Universe',
  Place.you => 'You',
};

Finder _nav(String label) => find.widgetWithText(NavigationDestination, label);

Finder _title(String title) =>
    find.descendant(of: find.byType(AppBar), matching: find.text(title));
