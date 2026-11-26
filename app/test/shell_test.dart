import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:flutter/material.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/activity_screen.dart';
import 'package:osp_app/screens/advanced_screen.dart';
import 'package:osp_app/screens/compose_screen.dart';
import 'package:osp_app/screens/events_screen.dart';
import 'package:osp_app/screens/found_people_screen.dart';
import 'package:osp_app/screens/node_summary_screen.dart';
import 'package:osp_app/screens/pair_screen.dart';
import 'package:osp_app/screens/people_screen.dart';
import 'package:osp_app/screens/search_screen.dart';
import 'package:osp_app/screens/trust_screen.dart';
import 'package:osp_app/screens/universe_tab.dart';
import 'package:osp_app/shell.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';

import 'event_fixture.dart';
import 'plain_words.dart';

/// The Rust side, as far as the app's places go.
class FakeApp implements OspApp {
  NodeStatus status_ = nodeStatus();

  /// What the node answers instead, if set.
  Object? statusError;

  @override
  Future<NodeStatus> status() async =>
      statusError == null ? status_ : throw statusError!;

  @override
  Future<String> invite() async => 'OSP1invite';

  @override
  Future<List<Followed>> following() async => [];

  @override
  Future<TrustList> trustList() async =>
      const TrustList(requests: [], trusted: []);

  @override
  UnsentPost? pendingPost() => null;

  /// Who the node has found.
  List<FoundPerson> found = [];
  final followed = <String>[];

  @override
  Future<List<FoundPerson>> people({required int limit}) async => found;

  /// The node's Universe now: the top, and other regions by name. Each
  /// `since` it was asked with, and each region.
  UniverseSnapshot snapshot = world([]);
  final regions = <String, UniverseSnapshot>{};
  final asked = <String?>[];
  final askedFor = <String>[];
  bool unreachable = false;

  /// The top with a layer lit, by layer, and each layer asked with.
  final litTops = <UniverseLayer, UniverseSnapshot>{};
  final layersAsked = <UniverseLayer?>[];

  @override
  Future<UniverseSnapshot?> universe({
    String? region,
    UniverseLayer? lit,
    String? since,
  }) async {
    asked.add(since);
    askedFor.add(region ?? '');
    layersAsked.add(lit);
    if (unreachable) throw Exception('no contact');
    final there = region == null
        ? (lit == null ? snapshot : litTops[lit] ?? snapshot)
        : regions[region] ?? world(const [], region: region, path: const ['']);
    return since == there.version ? null : there;
  }

  /// What the node did, oldest first, and each `since` it was asked.
  List<UniverseEvent> events = [];
  final eventsAsked = <BigInt?>[];

  @override
  Future<List<UniverseEvent>> universeEvents({BigInt? since}) async {
    eventsAsked.add(since);
    if (unreachable) throw Exception('no contact');
    return [
      for (final e in events)
        if (since == null || e.seq > since) e,
    ];
  }

  /// What a search finds, and each one asked for.
  Found searchAnswer = const Found(people: [], posts: []);
  final searched = <String>[];

  @override
  Future<Found> search({required String words, required int limit}) async {
    searched.add(words);
    return searchAnswer;
  }

  /// What a tap on a group or cell opens, by region, and each one asked.
  final sheets = <String, Found>{};
  final sheetsAsked = <String>[];

  @override
  Future<Found> regionItems({
    required String region,
    required int limit,
  }) async {
    sheetsAsked.add(region);
    return sheets[region] ?? const Found(people: [], posts: []);
  }

  /// What "How are we connected?" answers, and each one asked.
  Connection? connectionAnswer;
  final connectedAsked = <String>[];

  /// When set, the answer waits for it.
  Completer<void>? holdConnection;

  @override
  Future<Connection> howConnected({required String identity}) async {
    connectedAsked.add(identity);
    await holdConnection?.future;
    return connectionAnswer ??
        Connection(
          identity: identity,
          paths: const [],
          people: const [],
          side: SideFrom.asked,
          vouches: 0,
        );
  }

  @override
  Future<void> followPerson({required String identity}) async =>
      followed.add(identity);

  /// The node's Activity, and each `through` it was marked read with.
  List<ActivityItem> activityEntries = [];
  final readThrough = <int?>[];

  @override
  Future<ActivityPage> activity({int? before, required int limit}) async =>
      ActivityPage(
        entries: activityEntries,
        unread: activityEntries.where((e) => !e.read).length,
        more: false,
      );

  @override
  Future<void> activityRead({required List<String> keys, int? through}) async {
    readThrough.add(through);
    activityEntries = [
      for (final e in activityEntries)
        e.seq <= (through ?? -1) ? entry(e, read: true) : e,
    ];
  }

  @override
  String deviceId() => 'device-0123456789';

  @override
  String recentLog({required int lines}) => '';

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

/// An Activity entry of [kind] about [names], with [seq].
ActivityItem activityItem(
  String kind,
  List<String> names, {
  int seq = 1,
  int count = 1,
  bool read = false,
  bool waiting = false,
  String text = '',
}) => ActivityItem(
  key: '$kind:$seq',
  seq: seq,
  atMs: DateTime.now().millisecondsSinceEpoch,
  read: read,
  kind: kind,
  post: 'post-$seq',
  postText: 'A walk by the river',
  people: [for (final n in names) ActivityName(identity: 'id-$n', name: n)],
  count: count,
  text: text,
  waiting: waiting,
  node: 'node-$seq',
);

ActivityItem entry(ActivityItem e, {required bool read}) => ActivityItem(
  key: e.key,
  seq: e.seq,
  atMs: e.atMs,
  read: read,
  kind: e.kind,
  post: e.post,
  postText: e.postText,
  reply: e.reply,
  people: e.people,
  count: e.count,
  text: e.text,
  waiting: e.waiting,
  node: e.node,
);

NodeStatus nodeStatus({
  int trustRequests = 0,
  int replyRequests = 0,
  int activityUnread = 0,
  int hostedPosts = 0,
  int servedLatelyBytes = 0,
  bool torStarting = false,
  bool lookedForPeople = false,
}) => NodeStatus(
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
  activityUnread: activityUnread,
  overTor: false,
  torStarting: torStarting,
  hostedPosts: hostedPosts,
  servedLatelyBytes: servedLatelyBytes,
  spaceBytes: 1400 * 1000 * 1000,
  lookedForPeople: lookedForPeople,
);

/// A snapshot with the owner, Alice, at the centre and [names] after her,
/// in a row to her right, five units apart, all in group 1.
UniverseSnapshot world(
  List<String> names, {
  String? version,
  String region = '',
  List<String> path = const [],
  List<UniverseMachine> machines = const [],
}) {
  UniversePerson person(String name, int? step, double x) => UniversePerson(
    lit: 0,
    identity: 'id-$name',
    name: name,
    step: step,
    followed: false,
    known: false,
    posts: 0,
    media: 0,
    reshares: 0,
    x: x,
    y: 0,
    group: step == 0 ? null : BigInt.one,
    bright: 0,
  );
  return UniverseSnapshot(
    version: version ?? 'v-$region-${names.join('-')}',
    level: UniverseLevel.local,
    region: region,
    path: path,
    groups: const [],
    people: [
      person('Alice', 0, 0),
      for (final (i, n) in names.indexed) person(n, 1, 5.0 * (i + 1)),
    ],
    lines: [
      for (var i = 1; i <= names.length; i++)
        UniverseLine(
          kind: LineKind.reshared,
          from: 0,
          to: i,
          count: 1,
          last: '2026-11-14',
        ),
    ],
    morePeople: 0,
    machines: machines,
    sky: const [],
  );
}

void main() {
  late Directory root;
  late FakeApp app;
  late HomeNode node;

  setUp(() {
    debugDrawnEvents.clear();
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

  /// Opens the app at [place]; [still] as on a phone asking for reduced
  /// motion.
  Future<void> start(
    WidgetTester tester,
    Place place, {
    bool still = false,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        key: UniqueKey(),
        builder: (context, child) => MediaQuery(
          data: MediaQuery.of(context).copyWith(disableAnimations: still),
          child: child!,
        ),
        // As the app does: places redraw as the node changes.
        home: ListenableBuilder(
          listenable: node,
          builder: (_, _) => AppShell(node: node),
        ),
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
    // The app's icon in the top bar, lit as the node is (ADR 0040 §7).
    'your node': (
      [find.byKey(const Key('node-mark'))],
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

  testWidgets('your node says what it did, from its own counts', (
    tester,
  ) async {
    app.status_ = nodeStatus(
      hostedPosts: 11,
      servedLatelyBytes: 324 * 1024 * 1024,
    );
    node.status = app.status_;
    await start(tester, Place.universe);
    await tester.tap(find.byTooltip('Your node'));
    await tester.pumpAndSettle();
    expect(
      find.text('It keeps 11 posts you shared available to others.'),
      findsOne,
    );
    expect(
      find.text(
        'It sent 324 MB of photos and videos to others in the last 30 days.',
      ),
      findsOne,
    );
    expect(
      find.text('You follow 2 people, and 1 person follows you.'),
      findsOne,
    );
    expect(find.text('It uses 1.3 GB of space for OSP.'), findsOne);
    expectPlainWords(tester);
  });

  testWidgets("a node that doesn't know this phone any more is paired with "
      'again, and the pairing stays until then', (tester) async {
    await start(tester, Place.home);
    // Removed from the node's devices: it answers, but not this phone.
    app.statusError = Exception("this device isn't paired with this node");
    await node.checkStatus();
    await tester.pumpAndSettle();
    expect(find.text("Your node doesn't know this phone any more."), findsOne);
    expect(find.text('Retry'), findsNothing);
    expectPlainWords(tester);

    await tester.tap(find.text('Pair again'));
    await tester.pumpAndSettle();
    expect(find.byType(PairScreen), findsOne);
    expect(find.text('Pair this phone again'), findsOne);
    expect(find.textContaining('the phone keeps what it has'), findsOne);
    expectPlainWords(tester);
    expect(node.paired?.identity, 'id-Alice');

    // Some other failure is the usual "can't reach it".
    await tester.pageBack();
    await tester.pumpAndSettle();
    app.statusError = Exception('no answer in time');
    await node.checkStatus();
    await tester.pumpAndSettle();
    expect(find.textContaining("Can't reach your node."), findsOne);
    expect(find.text('Pair again'), findsNothing);
  });

  testWidgets('the icon in the top bar says how your node is', (tester) async {
    await start(tester, Place.home);
    expect(find.byTooltip('Your node: running'), findsOne);

    node.reachability = Reachability.offline;
    node.notifyListeners();
    await tester.pumpAndSettle();
    expect(find.byTooltip("Your node: can't reach it"), findsOne);

    // Reaching it breathes, but holds still when asked for less motion.
    node.reachability = Reachability.connecting;
    node.notifyListeners();
    await tester.pump();
    expect(find.byTooltip('Your node: reaching it'), findsOne);
    expect(tester.binding.transientCallbackCount, greaterThan(0));
    await start(tester, Place.home, still: true);
    await tester.pump();
    expect(find.byTooltip('Your node: reaching it'), findsOne);
    expect(tester.binding.transientCallbackCount, 0, reason: 'still');
  });

  testWidgets('out of reach, the map says it is the last one seen', (
    tester,
  ) async {
    app.snapshot = world(['Kim']);
    await start(tester, Place.universe);
    expect(find.textContaining('As last seen'), findsNothing);
    node.reachability = Reachability.offline;
    node.lastContact = DateTime.now().subtract(const Duration(hours: 3));
    await tester.tap(find.widgetWithText(NavigationDestination, 'Home'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(NavigationDestination, 'Universe'));
    await tester.pumpAndSettle();
    expect(find.text('As last seen, 3 h ago'), findsOne);
  });

  testWidgets('a node that served nothing says so, and no hosting', (
    tester,
  ) async {
    await start(tester, Place.universe);
    await tester.tap(find.byTooltip('Your node'));
    await tester.pumpAndSettle();
    expect(find.textContaining('available to others'), findsNothing);
    expect(
      find.text('It sent no photos or videos to others in the last 30 days.'),
      findsOne,
    );
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
    app.activityEntries = [
      activityItem('knows_you', ['Jimmy'], seq: 3, text: 'from the gym'),
      activityItem('liked', ['Alice', 'Jimmy'], seq: 2, count: 5),
      activityItem('followed', [], seq: 1, read: true),
    ];
    node.status = nodeStatus(trustRequests: 1, activityUnread: 2);
    app.status_ = node.status!;
    await start(tester, Place.home);
    expect(find.text('2'), findsOne);

    // Read on one phone, it's read for every phone: the node keeps it.
    app.status_ = nodeStatus(trustRequests: 1);
    await tester.tap(find.byTooltip('Activity'));
    await tester.pumpAndSettle();
    expect(find.text('People who say they know you'), findsOne);
    expect(find.text('Jimmy says they know you'), findsOne);
    expect(find.textContaining('from the gym'), findsOne);
    expect(
      find.text(
        'Alice, Jimmy and 3 others liked your post “A walk by the river”',
      ),
      findsOne,
    );
    expect(find.text('Someone new follows you'), findsOne);
    expect(app.readThrough, [3]);
    expect(app.activityEntries.every((e) => e.read), isTrue);
    expectPlainWords(tester);

    await tester.tap(find.text('Jimmy says they know you'));
    await tester.pumpAndSettle();
    expect(find.byType(TrustScreen), findsOneWidget);
    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(find.text('2'), findsNothing);
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

  testWidgets('the Universe lists who the node found, to follow', (
    tester,
  ) async {
    final posted = DateTime.now()
        .subtract(const Duration(hours: 2))
        .toUtc()
        .toIso8601String();
    app.found = [
      FoundPerson(
        regions: const [],
        identity: 'id-Kim',
        name: 'Kim',
        summary: 'Plays the cello',
        followed: false,
        foundThrough: 'upset-slot-only',
        newest: posted,
      ),
      const FoundPerson(
        regions: [],
        identity: 'id-Sam',
        name: 'Sam',
        followed: true,
      ),
    ];
    app.snapshot = world(['Kim', 'Sam']);
    await start(tester, Place.universe);
    expect(find.text('Your node found 2 people'), findsOne);
    expect(find.text('Kim, Sam'), findsOne);

    await tester.tap(find.text('Your node found 2 people'));
    await tester.pumpAndSettle();
    expect(find.byType(FoundPeopleScreen), findsOneWidget);
    expect(find.textContaining('Posted 2 h ago'), findsOne);
    expect(find.text('Following'), findsOne);
    expectPlainWords(tester);

    await tester.tap(find.widgetWithText(TextButton, 'Follow'));
    await tester.pumpAndSettle();
    expect(app.followed, ['id-Kim']);
    expect(find.text('Following'), findsNWidgets(2));
  });

  testWidgets('a node that found nobody says it is looking', (tester) async {
    await start(tester, Place.universe);
    expect(find.text('Your node is looking for people'), findsOne);
    expectPlainWords(tester);
  });

  testWidgets('a node that looked and found nobody says so, not that it is '
      'still looking', (tester) async {
    app.status_ = nodeStatus(lookedForPeople: true);
    node.status = app.status_;
    await start(tester, Place.universe);
    expect(find.text('Your node found nobody yet'), findsOne);
    expect(find.text('Your node is looking for people'), findsNothing);
    expectPlainWords(tester);
    await tester.tap(find.text('Your node found nobody yet'));
    await tester.pumpAndSettle();
    expect(find.text('Invite and follow'), findsOne);
  });

  testWidgets('a node that found nobody offers an invite code', (tester) async {
    await start(tester, Place.universe);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Your node is looking for people'));
    await tester.pumpAndSettle();
    expect(find.text('Invite and follow'), findsOne);
  });

  testWidgets('the Universe shows the one it holds, then asks since it', (
    tester,
  ) async {
    // Kept from last time, and the node can't be reached yet.
    node.universe = world(['Kim'], version: 'old');
    app.unreachable = true;
    await start(tester, Place.universe);
    expect(find.text('Your node found 1 person'), findsOne);
    expect(find.text('Kim'), findsOne);
    expect(app.asked, ['old']);

    // The node answers: a newer one replaces it.
    app.unreachable = false;
    app.snapshot = world(['Kim', 'Sam', 'Lee', 'Ana', 'Bo']);
    await node.checkStatus();
    await tester.pumpAndSettle();
    expect(find.text('Your node found 5 people'), findsOne);
    expect(find.text('Kim, Sam, Lee and 2 more'), findsOne);

    // Unchanged: asked since the one held, which stays.
    await node.checkStatus();
    await tester.pumpAndSettle();
    expect(app.asked.last, app.snapshot.version);
    expect(find.text('Your node found 5 people'), findsOne);
  });

  testWidgets('the Universe counts those who did not fit', (tester) async {
    final some = world(['Kim', 'Sam']);
    app.snapshot = UniverseSnapshot(
      version: 'v',
      level: UniverseLevel.local,
      region: '',
      path: const [],
      groups: const [],
      people: some.people,
      lines: const [],
      morePeople: 40,
      machines: const [],
      sky: const [],
    );
    await start(tester, Place.universe);
    expect(find.text('Your node found 42 people'), findsOne);
    expect(find.text('Kim, Sam and 40 more'), findsOne);
  });

  /// Where [world]'s person [n] (1 the first after Alice) is drawn, with
  /// the top framed: they're in a row to the right of her star, the
  /// farthest of [count] at 0.4 of the room the controls leave clear.
  Offset drawnAt(WidgetTester tester, int n, int count) {
    final map = tester.getRect(find.byKey(const Key('universe-map')));
    final room = min(map.width, max(map.height - 254, map.height / 3));
    // The button over her star is centred on it.
    final star = tester.getCenter(find.byTooltip('Your node'));
    return star + Offset(room * 0.4 * n / count, 0);
  }

  testWidgets('tapping a group opens its people, then the map', (tester) async {
    final top = world([]);
    app.snapshot = UniverseSnapshot(
      version: 'g',
      level: UniverseLevel.universe,
      region: '',
      path: const [],
      groups: [
        UniverseGroup(
          lit: 0,
          id: BigInt.from(7),
          people: 12,
          bright: 40,
          x: 5,
          y: 0,
        ),
      ],
      people: top.people,
      lines: const [],
      morePeople: 0,
      machines: const [],
      sky: const [],
    );
    app.sheets['g:7'] = const Found(
      people: [
        FoundPerson(
          regions: [],
          identity: 'id-Kim',
          name: 'Kim',
          followed: false,
        ),
      ],
      posts: [],
    );
    await start(tester, Place.universe);
    await tester.pumpAndSettle();

    await tester.tapAt(drawnAt(tester, 1, 1));
    await tester.pumpAndSettle();
    expect(app.sheetsAsked, ['g:7']);
    expect(find.text('A group of 12 people'), findsOne);
    expect(find.text('Kim'), findsOne);
    expect(node.universeRegion, '', reason: 'the map stays where it was');
    expectPlainWords(tester);

    await tester.tap(find.byKey(const Key('region-open-map')));
    await tester.pumpAndSettle();
    expect(find.text('A group of 12 people'), findsNothing);
    expect(app.askedFor.last, 'g:7');
    expect(node.universeRegion, 'g:7');
  });

  testWidgets('tapping a star stands there, and stepping back keeps going', (
    tester,
  ) async {
    app.snapshot = world(['Kim']);
    app.regions['p:id-Kim'] = world(['Kim'], region: 'p:id-Kim', path: ['']);
    await start(tester, Place.universe);
    expect(find.byTooltip('Zoom out'), findsNothing, reason: 'at the top');
    final home = tester.getCenter(find.byTooltip('Your node'));

    await tester.tapAt(drawnAt(tester, 1, 1));
    await tester.pumpAndSettle();
    expect(app.askedFor.last, 'p:id-Kim');
    expect(node.universeRegion, 'p:id-Kim');
    expect(find.text('Kim'), findsOne);
    expect(find.textContaining('1 public line shown'), findsOne);
    expectPlainWords(tester);

    // Stepping back goes up the path, the top as held, and shows all of
    // it again rather than only the one stood at.
    await tester.tap(find.byTooltip('Step back'));
    await tester.pumpAndSettle();
    expect(node.universeRegion, '');
    expect(find.text('Your node found 1 person'), findsOne);
    final back = tester.getCenter(find.byTooltip('Your node'));
    expect((back - home).distance, lessThan(1), reason: 'framed as before');
  });

  testWidgets('a screen reader finds each star, stands there and zooms', (
    tester,
  ) async {
    final reader = tester.ensureSemantics();
    app.snapshot = world(['Kim', 'Jimmy']);
    app.regions['p:id-Kim'] = world(
      ['Kim', 'Jimmy'],
      region: 'p:id-Kim',
      path: [''],
    );
    await start(tester, Place.universe);
    expect(find.semantics.byLabel(RegExp(r'^Jimmy\. 1 step')), findsOne);

    // The map's own zoom, as a screen reader can't pinch: her star twice as
    // far from the middle.
    final map = find.semantics.byLabel(RegExp('^The map of your Universe'));
    final middle = tester.getCenter(find.byKey(const Key('universe-map')));
    final before = tester.getCenter(find.byTooltip('Your node')) - middle;
    tester.semantics.customAction(
      map,
      const CustomSemanticsAction(label: 'Zoom in'),
    );
    await tester.pumpAndSettle();
    final after = tester.getCenter(find.byTooltip('Your node')) - middle;
    expect(after.distance, closeTo(before.distance * 2, 1));

    tester.semantics.tap(find.semantics.byLabel(RegExp(r'^Kim\. 1 step')));
    await tester.pumpAndSettle();
    expect(node.universeRegion, 'p:id-Kim');
    expect(
      find.semantics.byLabel(RegExp(r'^Kim\. You are standing here')),
      findsOne,
    );

    // Out far enough, it goes up the path, as a pinch does.
    for (var i = 0; i < 2; i++) {
      tester.semantics.customAction(
        map,
        const CustomSemanticsAction(label: 'Zoom out'),
      );
      await tester.pumpAndSettle();
    }
    expect(node.universeRegion, '');
    reader.dispose();
  });

  testWidgets('asked for less motion, the map jumps rather than flies', (
    tester,
  ) async {
    app.snapshot = world(['Kim']);
    app.regions['p:id-Kim'] = world(['Kim'], region: 'p:id-Kim', path: ['']);
    await start(tester, Place.universe, still: true);

    await tester.tapAt(drawnAt(tester, 1, 1));
    // The tap, then the region from the node: nothing left to move.
    await tester.pump();
    await tester.pump();
    expect(node.universeRegion, 'p:id-Kim');
    expect(find.textContaining('1 public line shown'), findsOne);
    expect(tester.binding.transientCallbackCount, 0, reason: 'nothing moves');
  });

  testWidgets('asked for less motion, what the node did is told, not drawn', (
    tester,
  ) async {
    app.snapshot = world(['Jimmy']);
    app.events = [
      anEvent(2, EventKind.served, author: 'id-Jimmy', post: 'p1', bytes: 1),
    ];
    await start(tester, Place.universe, still: true);
    expect(find.byKey(const Key('event-line')), findsOne);
    expect(debugDrawnEvents, isEmpty);
  });

  testWidgets('only a tap asks how you are connected, and the paths show', (
    tester,
  ) async {
    app.snapshot = world(['Kim', 'Jimmy']);
    app.regions['p:id-Kim'] = world(
      ['Kim', 'Jimmy'],
      region: 'p:id-Kim',
      path: [''],
    );
    app.connectionAnswer = const Connection(
      identity: 'id-Kim',
      paths: [
        ConnectionPath(
          hops: [
            PathHop(identity: 'id-Jimmy', how: HopKind.follows, steps: 1),
            PathHop(identity: 'id-Kim', how: HopKind.reshared, steps: 1),
          ],
        ),
      ],
      people: [
        ConnectedPerson(identity: 'id-Kim', name: 'Kim', regions: []),
        ConnectedPerson(identity: 'id-Jimmy', name: 'Jimmy', regions: []),
      ],
      side: SideFrom.asked,
      vouches: 1,
    );
    await start(tester, Place.universe);
    await tester.tapAt(drawnAt(tester, 1, 2));
    await tester.pumpAndSettle();
    expect(node.universeRegion, 'p:id-Kim');
    expect(
      app.connectedAsked,
      isEmpty,
      reason: 'standing at someone asks nothing (ADR 0037 §2)',
    );

    await tester.tap(find.text('How are we connected?'));
    await tester.pumpAndSettle();
    expect(app.connectedAsked, ['id-Kim']);
    expect(node.universeRegion, '', reason: 'the paths start at your star');
    final card = find.byKey(const Key('paths-card'));
    expect(
      find.descendant(of: card, matching: find.text('Kim')),
      findsOne,
      reason: 'the name is the title, not cut off after a sentence',
    );
    expect(
      find.text('Their node named 1 person who vouched for them'),
      findsOne,
    );
    expect(find.text('1. You follow Jimmy. Jimmy reshared Kim.'), findsOne);
    expectPlainWords(tester);

    await tester.tap(find.byTooltip('Put away'));
    await tester.pumpAndSettle();
    expect(node.connection, isNull);
    expect(find.byKey(const Key('paths-card')), findsNothing);
    expect(app.connectedAsked, hasLength(1));
  });

  testWidgets('no path found says so, not that you are not connected', (
    tester,
  ) async {
    app.snapshot = world(['Kim']);
    app.regions['p:id-Kim'] = world(['Kim'], region: 'p:id-Kim', path: ['']);
    await start(tester, Place.universe);
    await tester.tapAt(drawnAt(tester, 1, 1));
    await tester.pumpAndSettle();
    await tester.tap(find.text('How are we connected?'));
    await tester.pumpAndSettle();
    expect(find.text('Your node hasn’t found a connection.'), findsOne);
    expect(find.textContaining('not connected'), findsNothing);
    expect(find.text('Their node named nobody who vouched for them'), findsOne);
  });

  testWidgets('an ask while Tor starts says it waits for Tor', (tester) async {
    app.snapshot = world(['Kim']);
    app.regions['p:id-Kim'] = world(['Kim'], region: 'p:id-Kim', path: ['']);
    app.holdConnection = Completer();
    await start(tester, Place.universe);
    node.status = nodeStatus(torStarting: true);
    await tester.tapAt(drawnAt(tester, 1, 1));
    await tester.pumpAndSettle();
    await tester.tap(find.text('How are we connected?'));
    await tester.pump();
    expect(
      find.text(
        'Your node is still connecting; it asks once it can reach others…',
      ),
      findsOne,
    );
    expect(find.textContaining('try again'), findsNothing);

    app.holdConnection!.complete();
    await tester.pumpAndSettle();
    expect(find.text('Their node named nobody who vouched for them'), findsOne);
  });

  testWidgets('a newcomer with only sky people sees them, not the whole sky', (
    tester,
  ) async {
    final top = world(['Kim', 'Sam']);
    UniversePerson inSky(UniversePerson p, double x) => UniversePerson(
      lit: 0,
      identity: p.identity,
      name: p.name,
      step: null,
      followed: false,
      known: false,
      posts: 0,
      media: 0,
      reshares: 0,
      x: x,
      y: 0,
      group: null,
      bright: 0,
    );
    app.snapshot = UniverseSnapshot(
      version: 'sky-only',
      level: UniverseLevel.universe,
      region: '',
      path: const [],
      groups: const [],
      people: [
        top.people[0],
        inSky(top.people[1], 80),
        inSky(top.people[2], 160),
      ],
      lines: const [],
      morePeople: 0,
      machines: const [],
      sky: const [],
    );
    app.regions['p:id-Sam'] = world(['Sam'], region: 'p:id-Sam', path: ['']);
    await start(tester, Place.universe);
    // The farthest of them is at the edge of the framing.
    await tester.tapAt(drawnAt(tester, 1, 1));
    await tester.pumpAndSettle();
    expect(node.universeRegion, 'p:id-Sam');
  });

  testWidgets('her star stays when her node can\'t say who it found', (
    tester,
  ) async {
    app.unreachable = true;
    await start(tester, Place.universe);
    expect(find.text("Couldn't ask your node who it found"), findsOne);
    final map = tester.renderObject(
      find
          .descendant(
            of: find.byKey(const Key('universe-map')),
            matching: find.byType(CustomPaint),
          )
          .first,
    );
    final star = tester.getCenter(find.byTooltip('Your node'));
    final mapAt = tester.getTopLeft(find.byKey(const Key('universe-map')));
    final at = star - mapAt;
    expect(
      map,
      paints..something(
        (call, args) => call == #drawCircle && args[0] == at && args[1] == 11.0,
      ),
    );
  });

  /// Opens the Universe with the zoom hint due, stopping while it plays.
  Future<void> startHinting(WidgetTester tester) async {
    node.zoomHintDue = true;
    app.snapshot = world(['Kim']);
    await start(tester, Place.home);
    // The map is there behind Home, as the app keeps the last one.
    await node.loadUniverse();
    await tester.pumpAndSettle();
    expect(node.universe, isNotNull);
    // Not played out unseen behind Home.
    expect(node.zoomHintDue, isTrue);
    await tester.tap(find.widgetWithText(NavigationDestination, 'Universe'));
    for (var i = 0; i < 5; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    expect(find.text('Pinch to zoom'), findsOne);
  }

  testWidgets('the map shows once that it zooms', (tester) async {
    await startHinting(tester);
    // Clear of her star, and the name under it.
    final star = tester.getRect(find.byTooltip('Your node'));
    final hint = tester.getRect(find.text('Pinch to zoom'));
    expect(hint.top, greaterThan(star.bottom + 30));
    // It lets a tap through to the map meanwhile.
    expect(find.byTooltip('My star').hitTestable(), findsOne);
    await tester.pumpAndSettle();
    expect(find.text('Pinch to zoom'), findsNothing);
    expect(node.zoomHintDue, isFalse);
  });

  testWidgets('a pinch ends the zoom hint early', (tester) async {
    await startHinting(tester);
    final mid = tester.getCenter(find.byKey(const Key('universe-map')));
    final a = await tester.startGesture(mid - const Offset(20, 0), pointer: 1);
    final b = await tester.startGesture(mid + const Offset(20, 0), pointer: 2);
    await a.moveBy(const Offset(-30, 0));
    await b.moveBy(const Offset(30, 0));
    await tester.pump();
    expect(find.text('Pinch to zoom'), findsNothing);
    expect(node.zoomHintDue, isFalse);
    await a.up();
    await b.up();
  });

  testWidgets('on its side, the card leaves the buttons clear', (tester) async {
    await tester.binding.setSurfaceSize(const Size(846, 387));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    app.snapshot = world(['Kim']);
    await start(tester, Place.universe);
    final card = tester.getRect(
      find.ancestor(
        of: find.text('Your node found 1 person'),
        matching: find.byType(Card),
      ),
    );
    for (final button in ['My star', 'Layers', 'Your node']) {
      final rect = tester.getRect(find.byTooltip(button));
      expect(rect.overlaps(card), isFalse, reason: button);
    }
  });

  testWidgets('My star flies home from anywhere', (tester) async {
    app.snapshot = world(['Kim', 'Sam']);
    app.regions['p:id-Sam'] = world(
      ['Kim', 'Sam'],
      region: 'p:id-Sam',
      path: [''],
    );
    await start(tester, Place.universe);
    final home = tester.getCenter(find.byTooltip('Your node'));
    await tester.tapAt(drawnAt(tester, 2, 2));
    await tester.pumpAndSettle();
    expect(node.universeRegion, 'p:id-Sam');
    expect(tester.getCenter(find.byTooltip('Your node')), isNot(home));
    await tester.tap(find.byTooltip('My star'));
    await tester.pumpAndSettle();
    expect(node.universeRegion, '');
    expect(app.askedFor.last, '');
    // Her star is back where it was.
    final star = tester.getCenter(find.byTooltip('Your node'));
    expect((star - home).distance, lessThan(1));
  });

  testWidgets('machines have their own list, in plain words', (tester) async {
    app.snapshot = world(
      ['Kim'],
      machines: const [
        UniverseMachine(
          words: 'able-baker-charlie',
          state: MachineState.answeredLately,
          directory: true,
          activity: ActivityBand.aLot,
        ),
        UniverseMachine(
          words: 'dog-easy-fox',
          state: MachineState.heardOf,
          directory: false,
          activity: ActivityBand.none,
        ),
      ],
    );
    await start(tester, Place.universe);
    await tester.tap(find.text('2 machines'));
    await tester.pumpAndSettle();
    // What machines are, for a newcomer, first.
    expect(find.textContaining('other people’s nodes'), findsOne);
    expectPlainWords(tester);
    // Then the machines, below it.
    final sheet = find.descendant(
      of: find.byType(BottomSheet),
      matching: find.byType(Scrollable),
    );
    await tester.scrollUntilVisible(
      find.text('dog-easy-fox'),
      100,
      scrollable: sheet.first,
    );
    expect(find.text('able-baker-charlie'), findsOne);
    expect(find.textContaining('lists people who asked to be found'), findsOne);
    expect(find.text('Heard of, hasn’t answered yet'), findsOne);
    expectPlainWords(tester);
    await tester.scrollUntilVisible(
      find.text('What your node did'),
      -100,
      scrollable: sheet.first,
    );
    // And from there, what her own node did.
    await tester.ensureVisible(find.text('What your node did'));
    await tester.tap(find.text('What your node did'));
    await tester.pumpAndSettle();
    expect(find.byType(NodeSummaryScreen), findsOneWidget);
    expect(find.text('able-baker-charlie'), findsNothing);
  });

  testWidgets('an idle world draws nothing on the map (ADR 0036 §2)', (
    tester,
  ) async {
    app.snapshot = world(['Jimmy', 'Kim']);
    await start(tester, Place.universe);
    await node.checkStatus();
    await tester.pump(const Duration(seconds: 10));
    expect(app.eventsAsked, isNotEmpty);
    expect(debugDrawnEvents, isEmpty);
    expect(find.byKey(const Key('event-line')), findsNothing);
    expect(tester.binding.hasScheduledFrame, isFalse, reason: 'still');
  });

  testWidgets('what the node did is drawn once, in words, and listed', (
    tester,
  ) async {
    app.snapshot = world(['Jimmy', 'Kim']);
    app.events = [
      // From this morning: listed, not drawn.
      anEvent(
        1,
        EventKind.found,
        words: 'amber-fox',
        people: 3,
        ago: const Duration(hours: 4),
      ),
      anEvent(
        2,
        EventKind.served,
        author: 'id-Jimmy',
        post: 'p1',
        bytes: 2 << 20,
      ),
    ];
    await start(tester, Place.universe);
    expect(debugDrawnEvents.toSet(), {BigInt.two});
    const served = "Your node sent 2.0 MB of Jimmy's post to another node";
    expect(find.text(served), findsOne);
    expectPlainWords(tester);

    // Summed into on the node: the same start, a new seq. Not drawn again.
    // Its line is gone by then.
    await tester.pump(const Duration(seconds: 9));
    expect(find.text(served), findsNothing);
    final earlier = app.events.last;
    app.events = [
      app.events.first,
      UniverseEvent(
        seq: BigInt.from(3),
        first: earlier.first,
        last: earlier.last,
        count: 2,
        kind: EventKind.served,
        people: 0,
        author: 'id-Jimmy',
        post: 'p1',
        bytes: BigInt.from(3 << 20),
        keptByCopy: false,
        fromAuthor: false,
        met: false,
      ),
    ];
    await node.checkStatus();
    // The events come, then the line is shown after the next frame.
    await tester.pump();
    await tester.pump();
    // Its line again, with the total: the first part was small.
    expect(
      find.text("Your node sent Jimmy's post to other nodes 2 times, 3.0 MB"),
      findsOne,
    );
    await tester.pump(const Duration(seconds: 10));
    expect(app.eventsAsked.last, BigInt.two, reason: 'since the newest held');
    expect(debugDrawnEvents.toSet(), {BigInt.two});
    expect(node.events.today.first.count, 2);

    // A copy keeping Jimmy's post available: drawn, in its own words.
    app.events = [
      anEvent(
        4,
        EventKind.fetched,
        author: 'id-Jimmy',
        host: 'id-Kim',
        keptByCopy: true,
        bytes: 1 << 20,
      ),
    ];
    await node.checkStatus();
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    expect(find.textContaining('Kept available by a copy'), findsOne);
    expectPlainWords(tester);

    await tester.tap(find.byKey(const Key('event-line')));
    await tester.pumpAndSettle();
    expect(find.byType(EventsScreen), findsOneWidget);
    expect(
      find.text('Your node found 3 new people, listed by amber-fox'),
      findsOne,
    );
    expect(find.textContaining('Kept available by a copy'), findsOne);
    expectPlainWords(tester);
    await tester.pump(const Duration(seconds: 10));
    // Each drawn once, only what was sent.
    expect(debugDrawnEvents.toSet(), {BigInt.two, BigInt.from(4)});
  });

  testWidgets('your node opens the list of what it did today', (tester) async {
    await start(tester, Place.universe);
    await tester.tap(find.byTooltip('Your node'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('What it did today'));
    await tester.pumpAndSettle();
    expect(find.text('Nothing yet today'), findsOne);
    expectPlainWords(tester);
  });

  testWidgets('pinching anyhow never loses the map', (tester) async {
    app.snapshot = world(['Kim', 'Sam', 'Lee']);
    await start(tester, Place.universe);
    final home = tester.getCenter(find.byTooltip('Your node'));
    final map = tester.getRect(find.byKey(const Key('universe-map')));
    final r = Random(1);
    Offset any() => Offset(
      map.left + r.nextDouble() * map.width,
      map.top + r.nextDouble() * map.height,
    );
    // Two or three fingers landing one by one, moving anywhere, sometimes
    // all to one point, and lifting one by one.
    for (var round = 0; round < 60; round++) {
      final n = 2 + r.nextInt(2);
      final fingers = <TestGesture>[];
      final at = <Offset>[];
      for (var i = 0; i < n; i++) {
        at.add(any());
        fingers.add(await tester.startGesture(at[i], pointer: 10 + i));
        await tester.pump(const Duration(milliseconds: 8));
      }
      for (var step = 0; step < 8; step++) {
        for (var i = 0; i < n; i++) {
          // Sometimes all fingers meet at one point.
          final to = r.nextInt(6) == 0 ? map.center : any();
          await fingers[i].moveTo(to);
        }
        await tester.pump(const Duration(milliseconds: 16));
      }
      for (final f in fingers) {
        await f.up();
        await tester.pump(const Duration(milliseconds: 8));
      }
      await tester.pump(const Duration(milliseconds: 500));
      expect(tester.takeException(), isNull, reason: 'round $round');
    }
    // A stray tap may have opened a screen over the map.
    tester
        .state<NavigatorState>(find.byType(Navigator))
        .popUntil((route) => route.isFirst);
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('My star'));
    await tester.pumpAndSettle();
    final star = tester.getCenter(find.byTooltip('Your node'));
    expect((star - home).distance, lessThan(1));
  });

  testWidgets('a layer lights up the map where it is, one at a time', (
    tester,
  ) async {
    app.snapshot = world(['Kim']);
    app.litTops[UniverseLayer.posts] = world(['Kim'], version: 'lit');
    await start(tester, Place.universe);

    await tester.tap(find.byTooltip('Layers'));
    await tester.pumpAndSettle();
    expect(find.text('What lights up'), findsOne);
    final network = tester.widget<ListTile>(
      find.widgetWithText(ListTile, 'Network'),
    );
    expect(network.enabled, isFalse, reason: 'Network comes with P6');
    expectPlainWords(tester);

    await tester.tap(find.text('Posts'));
    await tester.pumpAndSettle();
    expect(app.layersAsked.last, UniverseLayer.posts);
    expect(node.universeRegion, '', reason: 'the map stays where it was');
    expect(node.universe?.version, 'lit');
    expect(find.text('Posts, last 90 days'), findsOne);
    expectPlainWords(tester);

    await tester.tap(find.byTooltip('Back to people'));
    await tester.pumpAndSettle();
    expect(app.layersAsked.last, isNull);
    expect(find.text('Posts, last 90 days'), findsNothing);
    expect(node.universe?.version, isNot('lit'), reason: 'held by layer');
  });

  testWidgets('a search lights up its results, flies there, and lists them', (
    tester,
  ) async {
    final top = world([]);
    app.snapshot = UniverseSnapshot(
      version: 'g',
      level: UniverseLevel.universe,
      region: '',
      path: const [],
      groups: [
        UniverseGroup(
          lit: 0,
          id: BigInt.from(7),
          people: 12,
          bright: 40,
          x: 5,
          y: 0,
        ),
      ],
      people: top.people,
      lines: const [],
      morePeople: 0,
      machines: const [],
      sky: const [],
    );
    app.regions['g:7'] = world(['Kim'], region: 'g:7', path: ['']);
    app.searchAnswer = const Found(
      people: [
        FoundPerson(
          regions: ['', 'g:7'],
          identity: 'id-Kim',
          name: 'Kim',
          followed: false,
        ),
      ],
      posts: [
        Post(
          regions: ['', 'g:7'],
          id: 'cello-post',
          author: 'Kim',
          authorIdentity: 'id-Kim',
          published: '2026-11-14T00:00:00Z',
          text: 'Cello at noon',
          media: [],
          resharerHosts: false,
          liked: false,
          opened: false,
        ),
      ],
    );
    await start(tester, Place.home);

    await tester.tap(find.byTooltip('Search'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'cello');
    await tester.testTextInput.receiveAction(TextInputAction.search);
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Show on the map'));
    await tester.pumpAndSettle();
    expect(find.byType(SearchScreen), findsNothing);
    expect(_title('Universe'), findsOneWidget);
    expect(
      node.universeRegion,
      'g:7',
      reason: 'the deepest region holding them all',
    );
    expect(find.text('1 person and 1 post, lit up'), findsOne);
    expectPlainWords(tester);

    await tester.tap(find.text('List'));
    await tester.pumpAndSettle();
    expect(find.byType(SearchScreen), findsOneWidget);
    expect(find.text('Cello at noon'), findsOne);
    expect(app.searched, ['cello', 'cello']);

    await tester.tap(find.byTooltip('Show on the map'));
    await tester.pumpAndSettle();
    expect(find.byType(SearchScreen), findsNothing);
    expect(find.text('1 person and 1 post, lit up'), findsOne);

    await tester.tap(find.byTooltip('Clear the search'));
    await tester.pumpAndSettle();
    expect(node.searchLights, isNull);
    expect(find.textContaining('Your node found'), findsOne);
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
