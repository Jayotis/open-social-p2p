import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/main.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/hide_list_screen.dart';
import 'package:osp_app/screens/pair_screen.dart';
import 'package:osp_app/screens/welcome_screen.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/terms.dart';
import 'package:osp_app/uploads.dart';

import 'plain_words.dart';

/// Nothing on the Rust side is needed before pairing, the hide list
/// included: the phone only tells its owner about it.
class FakeApp implements OspApp {
  // Pairing refreshes Home: nothing there.
  @override
  Future<List<Post>> feed({required int limit}) async => [];

  @override
  SavedFeed? savedFeed() => null;

  /// The node a pairing code pairs with.
  NodeInfo pairsWith = alicesNode;

  @override
  Future<NodeInfo> pair({
    required String ticket,
    required String deviceName,
  }) async => pairsWith;

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

const alicesNode = NodeInfo(
  name: 'Alice',
  identity: 'id-Alice',
  paired: '2026-11-14T00:00:00Z',
);

void main() {
  late Directory root;
  late HomeNode node;
  late FakeApp app;

  setUp(() {
    app = FakeApp();
    root = Directory.systemTemp.createTempSync('welcome_test');
    node = HomeNode.forTests(
      app,
      UploadsFolder(
        Directory('${root.path}/uploads'),
        pickerDir: Directory('${root.path}/cache'),
      ),
    );
  });
  tearDown(() {
    node.dispose();
    root.deleteSync(recursive: true);
  });

  testWidgets('first start: the node is required, then the terms, then '
      'pairing (ADR 0042 §3)', (tester) async {
    await tester.pumpWidget(PhoneApp(node: node));
    expect(find.text('Welcome to OSP'), findsOneWidget);
    expect(find.textContaining("doesn't work without one"), findsOneWidget);
    expect(find.text('How to set up a node'), findsOneWidget);
    expect(find.byType(PairScreen), findsNothing);
    expectPlainWords(tester);

    await tester.tap(find.text('I have a node'));
    await tester.pumpAndSettle();
    expect(find.text('Terms of use'), findsOneWidget);
    expect(find.textContaining('explicit sexual material'), findsOneWidget);
    // The full terms are a tap away before agreeing.
    expect(find.text('Read the full Terms of Use'), findsOneWidget);
    expect(find.byType(PairScreen), findsNothing);
    expectPlainWords(tester);

    await tester.tap(find.text('I agree'));
    await tester.pumpAndSettle();
    expect(node.termsDue, isFalse);

    // Then the project's hide list, which nodes use unless turned off: told,
    // not asked (ADR 0042 §6).
    expect(find.byType(HideListScreen), findsOneWidget);
    expect(find.text('The worst material is hidden'), findsOneWidget);
    // The list is only for the worst kinds, not adult material (ADR 0042 §6).
    expect(find.textContaining('involving children'), findsOneWidget);
    expect(find.textContaining('turn the list off'), findsOneWidget);
    expect(find.text('Not now'), findsNothing);
    expect(find.byType(PairScreen), findsNothing);
    expectPlainWords(tester);
    await tester.tap(find.text('Continue'));
    await tester.pumpAndSettle();
    expect(node.hideListDue, isFalse);
    expect(find.byType(PairScreen), findsOneWidget);
  });

  testWidgets('a paired phone that has not agreed to these terms sees only '
      'the terms', (tester) async {
    node.paired = const NodeInfo(
      name: 'Alice',
      identity: 'id-Alice',
      paired: '2026-11-14T00:00:00Z',
    );
    await tester.pumpWidget(MaterialApp(home: WelcomeScreen(node: node)));
    expect(find.text('Welcome to OSP'), findsNothing);
    expect(find.text('Terms of use'), findsOneWidget);
    expect(find.text('Read the full Terms of Use'), findsOneWidget);
    await tester.tap(find.text('I agree'));
    expect(node.termsDue, isFalse);
  });

  test('pairing again with the same node is no first run; with another '
      "owner's node it is", () async {
    await node.pair('OSPPAIR-first');
    expect(node.firstRun, isTrue);
    node.finishFirstRun();
    // The node forgot this phone, and it pairs again.
    await node.pair('OSPPAIR-again');
    expect(node.firstRun, isFalse);
    app.pairsWith = const NodeInfo(
      name: 'Kim',
      identity: 'id-Kim',
      paired: '2026-11-26T00:00:00Z',
    );
    await node.pair('OSPPAIR-kims');
    expect(node.firstRun, isTrue);
    // The refreshes each pairing started end before the node is disposed.
    await pumpEventQueue();
  });

  test('the app shows the same terms as docs/terms-short.md', () {
    final doc = File('../docs/terms-short.md').readAsStringSync();
    final bullets = [
      for (final line in doc.split('\n'))
        if (line.startsWith('- ')) line.substring(2),
    ];
    expect(bullets, termsOfUse);
    expect(doc, contains('Version $termsVersion,'));
  });
}
