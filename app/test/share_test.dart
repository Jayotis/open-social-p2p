import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/share_universe_screen.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';
import 'package:osp_app/widgets/share_card_image.dart';

/// The node, as far as "Share my Universe" goes: it names on the card only
/// the people asked for, as the node's allowlist does.
class FakeApp implements OspApp {
  final asked = <List<String>>[];

  @override
  Future<ShareCard> shareCard({required List<String> named}) async {
    asked.add(named);
    return card(named: named.contains('id-Jimmy') ? 'Jimmy' : null);
  }

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

ShareCard card({String? named}) => ShareCard(
  people: 68,
  direct: 14,
  paths: 6,
  bytesServed: 1800000000,
  points: [
    const CardPoint(x: 0, y: 0, people: 1, you: true),
    CardPoint(x: 64, y: -32, people: 1, you: false, name: named),
    const CardPoint(x: -96, y: 48, people: 12, you: false),
  ],
  lines: const [CardLine(from: 0, to: 1), CardLine(from: 1, to: 2)],
);

UniversePerson person(
  String name,
  int? step, {
  bool followed = false,
  bool known = false,
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
  x: 0,
  y: 0,
  bright: 0,
);

void main() {
  late Directory root;
  late FakeApp app;
  late HomeNode node;

  setUp(() {
    root = Directory.systemTemp.createTempSync('share_test');
    app = FakeApp();
    node = HomeNode.forTests(
      app,
      UploadsFolder(
        Directory('${root.path}/uploads'),
        pickerDir: Directory('${root.path}/cache'),
      ),
    );
    node.universe = UniverseSnapshot(
      version: 'v',
      level: UniverseLevel.local,
      region: '',
      path: const [],
      groups: const [],
      people: [
        person('Kim', 0),
        person('Jimmy', 1, followed: true),
        person('Tom', 1, known: true),
        person('Fred', null),
      ],
      lines: const [],
      morePeople: 0,
      machines: const [],
      sky: const [],
    );
  });
  tearDown(() => root.deleteSync(recursive: true));

  /// Lets the card be made and drawn, which takes real time, until
  /// [shown] is on the screen.
  Future<void> settle(WidgetTester tester, Finder shown) async {
    for (var i = 0; i < 100 && shown.evaluate().isEmpty; i++) {
      await tester.runAsync(
        () => Future.delayed(const Duration(milliseconds: 50)),
      );
      await tester.pump();
    }
  }

  Uint8List preview(WidgetTester tester) {
    final image = tester.widget<Image>(find.byKey(const Key('share-preview')));
    return (image.image as MemoryImage).bytes;
  }

  testWidgets('the card names only whom you tick, and shares what it shows', (
    tester,
  ) async {
    // A tall phone's screen, so what's under the card is laid out.
    tester.view.physicalSize = const Size(1080, 3600);
    tester.view.devicePixelRatio = 3;
    addTearDown(tester.view.reset);
    final shared = <Uint8List>[];
    await tester.runAsync(
      () => tester.pumpWidget(
        MaterialApp(
          home: ShareUniverseScreen(
            node: node,
            share: (png) async => shared.add(png),
          ),
        ),
      ),
    );
    await settle(tester, find.textContaining('Nobody is named on it'));

    // Nobody ticked: the node is asked to name nobody.
    expect(app.asked, [<String>[]]);
    expect(find.textContaining('Nobody is named on it'), findsOneWidget);
    // Only the people you follow or know can be ticked: not you, not Fred.
    expect(find.byType(CheckboxListTile), findsNWidgets(2));
    expect(find.text('Jimmy'), findsOneWidget);
    expect(find.text('Tom'), findsOneWidget);
    expect(find.text('Fred'), findsNothing);

    // What's shared is the preview, byte for byte.
    final first = preview(tester);
    expect(first.take(4), [0x89, 0x50, 0x4E, 0x47], reason: 'a PNG');
    await tester.tap(find.text('Share'));
    await tester.pump();
    expect(shared.single, same(first));

    // Ticking Jimmy makes a new card, naming him.
    app.asked.clear();
    await tester.tap(find.text('Jimmy'));
    await settle(tester, find.textContaining('Named on it: Jimmy.'));
    expect(app.asked, [
      ['id-Jimmy'],
    ]);
    expect(find.textContaining('Named on it: Jimmy.'), findsOneWidget);
    final second = preview(tester);
    expect(second, isNot(equals(first)));
    await tester.ensureVisible(find.text('Share'));
    await tester.tap(find.text('Share'));
    await tester.pump();
    expect(shared.last, same(second));
  });

  test("the card's words", () {
    expect(shareCardLines(card()), [
      '68 people discovered · 14 direct connections',
      '6 separate paths into the wider network',
      '1.7 GB served peer to peer',
    ]);
    final quiet = ShareCard(
      people: 1,
      direct: 0,
      paths: 0,
      bytesServed: 0,
      points: const [],
      lines: const [],
    );
    expect(shareCardLines(quiet), ['1 person discovered']);
    expect(shareCardNames(card(named: 'Jimmy')), ['Jimmy']);
    expect(shareCardNames(card()), isEmpty);
  });
}
