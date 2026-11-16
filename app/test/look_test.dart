/// The look, as pictures (P9, ADR 0040): Home, the Universe and a profile,
/// light and dark, drawn with the app's own fonts. P9's gate is that these
/// are recognisably OSP without the logo; the pictures keep that from
/// slipping unnoticed.
///
/// After a change to the look on purpose:
/// `flutter test --update-goldens test/look_test.dart`, then look at
/// `test/goldens/` before committing.
@Tags(['golden'])
library;

import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/profile_screen.dart';
import 'package:osp_app/shell.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/theme.dart';
import 'package:osp_app/uploads.dart';

/// The Rust side: a node with a few people, posts and a small Universe.
class FakeApp implements OspApp {
  @override
  Future<NodeStatus> status() async => status_;

  @override
  Future<List<Post>> feed({required int limit}) async => posts;

  @override
  Future<UniverseSnapshot?> universe({
    String? region,
    UniverseLayer? lit,
    String? since,
  }) async => since == sky.version ? null : sky;

  @override
  Future<List<UniverseEvent>> universeEvents({BigInt? since}) async => [];

  @override
  Future<Profile> profile({required String identity}) async => kim;

  @override
  Future<ProfilePage> profileItems({
    required String identity,
    required String kind,
    String? before,
    required int limit,
  }) async => ProfilePage(
    posts: before == null && kind == 'posts'
        ? posts.where((p) => p.authorIdentity == kim.identity).toList()
        : const [],
    next: null,
  );

  @override
  Future<ActivityPage> activity({int? before, required int limit}) async =>
      const ActivityPage(entries: [], unread: 0, more: false);

  @override
  Future<List<FoundPerson>> people({required int limit}) async => [];

  @override
  Future<List<Followed>> following() async => [];

  @override
  Future<TrustList> trustList() async =>
      const TrustList(requests: [], trusted: []);

  @override
  Future<String> mediaUrl({required String blob, required String mediaType}) =>
      Future.error(Exception('not in tests'));

  @override
  UnsentPost? pendingPost() => null;

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

final status_ = NodeStatus(
  name: 'Alice',
  version: '0.1.0',
  uptimeSecs: 3600,
  follows: 3,
  followers: 2,
  devices: 1,
  relayConnected: true,
  problems: const [],
  path: 'direct',
  rttMs: 80,
  trustRequests: 0,
  replyRequests: 0,
  activityUnread: 2,
  overTor: true,
  torStarting: false,
  hostedPosts: 4,
  servedLatelyBytes: 0,
  spaceBytes: 0,
);

String ago(Duration d) => DateTime.now().toUtc().subtract(d).toIso8601String();

Post post(
  String who,
  String text,
  Duration age, {
  String? resharedBy,
  int? likes,
  bool liked = false,
  int? resharers,
  List<Media> media = const [],
}) => Post(
  regions: const [],
  id: 'post-$who-${text.length}',
  author: who,
  authorIdentity: 'id-$who',
  published: ago(age),
  text: text,
  media: media,
  resharedBy: resharedBy,
  resharerHosts: false,
  likes: likes,
  liked: liked,
  resharers: resharers,
);

const photo = Media(
  isVideo: false,
  blob: 'photo-blob',
  mediaType: 'image/webp',
  size: 557 * 1024,
  width: 1200,
  height: 800,
  thumbnailBlob: 'thumb-blob',
  onNode: true,
);

final posts = [
  post(
    'Kim',
    'First frost on the allotment this morning. The leeks look happier '
        'about it than I am.',
    const Duration(hours: 2),
    likes: 3,
    media: const [photo],
  ),
  post(
    'Jimmy',
    'Finished wiring the shed node. It hums along at 4 W and nobody in the '
        'house has noticed.',
    const Duration(hours: 5),
    likes: 1,
    liked: true,
    resharedBy: 'Kim',
  ),
  post(
    'Fred',
    'Anyone else reading the river survey? Page 40 has the old mill.',
    const Duration(days: 1),
  ),
];

const kim = Profile(
  identity: 'id-Kim',
  name: 'Kim',
  summary: 'Allotments, bikes and the odd bit of soldering. Leeds.',
  links: [ProfileLinkEntry(name: 'Blog', url: 'https://example.org/kim')],
  picture: null,
  own: false,
  followed: true,
  known: true,
  posts: 12,
  media: 4,
  reshares: 2,
  followers: null,
  following: null,
);

/// Alice at the centre, people at one and two steps, and lines between.
final sky = () {
  UniversePerson person(
    String name,
    int? step,
    double x,
    double y, {
    bool followed = false,
    bool known = false,
    int bright = 0,
  }) => UniversePerson(
    lit: 0,
    identity: 'id-$name',
    name: name,
    step: step,
    followed: followed,
    known: known,
    posts: 0,
    media: 0,
    reshares: 0,
    x: x,
    y: y,
    group: step == 0 ? null : BigInt.one,
    bright: bright,
  );
  UniverseLine line(int from, int to, [LineKind kind = LineKind.reshared]) =>
      UniverseLine(kind: kind, from: from, to: to, count: 1, last: '');
  return UniverseSnapshot(
    version: 'look',
    level: UniverseLevel.local,
    region: '',
    path: const [],
    groups: const [],
    people: [
      person('Alice', 0, 0, 0),
      person('Kim', 1, 6, -8, followed: true, known: true, bright: 40),
      person('Jimmy', 1, -9, -4, followed: true, bright: 20),
      person('Fred', 1, 3, 9, followed: true),
      person('Priya', 2, 14, -14, bright: 60),
      person('Tom', 2, -15, 6),
      person('Ines', 2, 12, 12),
    ],
    lines: [
      line(0, 1),
      line(0, 2),
      line(1, 4),
      line(2, 5),
      line(3, 6, LineKind.creditedEarly),
      line(1, 2),
    ],
    morePeople: 0,
    machines: const [],
    sky: const [],
  );
}();

/// The app's fonts, so the pictures show real text rather than boxes.
Future<void> loadFonts() async {
  final manifest =
      json.decode(await rootBundle.loadString('FontManifest.json')) as List;
  for (final family in manifest.cast<Map<String, dynamic>>()) {
    final loader = FontLoader(family['family'] as String);
    for (final font in (family['fonts'] as List).cast<Map>()) {
      loader.addFont(rootBundle.load(font['asset'] as String));
    }
    await loader.load();
  }
}

void main() {
  late Directory root;
  late HomeNode node;

  setUpAll(loadFonts);

  setUp(() {
    root = Directory.systemTemp.createTempSync('look_test');
    node = HomeNode.forTests(
      FakeApp(),
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
    node.status = status_;
    node.posts = posts;
    node.zoomHintDone();
  });
  tearDown(() {
    node.dispose();
    root.deleteSync(recursive: true);
  });

  Future<void> show(WidgetTester tester, Brightness b, Widget home) async {
    tester.view.physicalSize = const Size(412 * 2, 892 * 2);
    tester.view.devicePixelRatio = 2;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      MaterialApp(
        debugShowCheckedModeBanner: false,
        theme: ospTheme(Brightness.light),
        darkTheme: ospTheme(Brightness.dark),
        themeMode: b == Brightness.dark ? ThemeMode.dark : ThemeMode.light,
        home: home,
      ),
    );
    await tester.pumpAndSettle();
  }

  for (final b in Brightness.values) {
    final name = b.name;

    testWidgets('Home, $name', (tester) async {
      await show(tester, b, AppShell(node: node));
      await expectLater(
        find.byType(MaterialApp),
        matchesGoldenFile('goldens/home_$name.png'),
      );
    });

    testWidgets('the Universe, $name', (tester) async {
      await show(tester, b, AppShell(node: node));
      await tester.tap(find.text('Universe'));
      await tester.pumpAndSettle();
      await expectLater(
        find.byType(MaterialApp),
        matchesGoldenFile('goldens/universe_$name.png'),
      );
    });

    testWidgets('a profile, $name', (tester) async {
      await show(
        tester,
        b,
        ProfileScreen(node: node, identity: 'id-Kim', name: 'Kim'),
      );
      await expectLater(
        find.byType(MaterialApp),
        matchesGoldenFile('goldens/profile_$name.png'),
      );
    });
  }
}
