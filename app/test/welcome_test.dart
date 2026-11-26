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

/// Nothing on the Rust side is needed before pairing. After, only the
/// hide list, as the node has it.
class FakeApp implements OspApp {
  HideListState state = hideState(asked: false, inUse: false);
  final answers = <bool>[];

  @override
  Future<HideListState> hideList() async => state;

  @override
  Future<HideListState> setHideList({required bool inUse}) async {
    answers.add(inUse);
    return state = hideState(asked: true, inUse: inUse);
  }

  // Answering refreshes Home: nothing there.
  @override
  Future<List<Post>> feed({required int limit}) async => [];

  @override
  SavedFeed? savedFeed() => null;

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

HideListState hideState({required bool asked, required bool inUse}) =>
    HideListState(
      asked: asked,
      inUse: inUse,
      hasKey: true,
      checked: inUse,
      exceptions: const [],
    );

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

    // Then whether to use the project's hide list (ADR 0042 §6).
    expect(find.byType(HideListScreen), findsOneWidget);
    expect(find.text('Hide the worst material?'), findsOneWidget);
    // The list is only for the worst kinds, not adult material (ADR 0042 §6).
    expect(find.textContaining('involving children'), findsOneWidget);
    expect(find.text('Use the list'), findsOneWidget);
    expect(find.text('Not now'), findsOneWidget);
    expect(find.byType(PairScreen), findsNothing);
    expectPlainWords(tester);
    await tester.tap(find.text('Use the list'));
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

  test('the answer given before pairing goes to a node not asked yet, '
      'and a node already asked keeps its own', () async {
    await node.answerHideList(true);
    expect(app.answers, isEmpty);
    node.paired = alicesNode;
    await node.loadHideList();
    expect(app.answers, [true]);
    expect(node.hideList!.inUse, isTrue);

    // Asked at its setup already ("not now"): the node's answer stands.
    app.answers.clear();
    app.state = hideState(asked: true, inUse: false);
    await node.loadHideList();
    expect(app.answers, isEmpty);
    expect(node.hideList!.inUse, isFalse);
  });

  testWidgets('a paired phone whose node was never asked asks once', (
    tester,
  ) async {
    node.agreeToTerms();
    node.paired = alicesNode;
    await node.loadHideList();
    expect(node.hideListDue, isTrue);
    await tester.pumpWidget(MaterialApp(home: HideListScreen(node: node)));
    await tester.tap(find.text('Not now'));
    await tester.pumpAndSettle();
    expect(app.answers, [false]);
    expect(node.hideListDue, isFalse);
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
