import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/main.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/pair_screen.dart';
import 'package:osp_app/screens/welcome_screen.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/terms.dart';
import 'package:osp_app/uploads.dart';

import 'plain_words.dart';

/// Nothing on the Rust side is needed before pairing.
class FakeApp implements OspApp {
  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

void main() {
  late Directory root;
  late HomeNode node;

  setUp(() {
    root = Directory.systemTemp.createTempSync('welcome_test');
    node = HomeNode.forTests(
      FakeApp(),
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
    expect(find.byType(PairScreen), findsNothing);
    expectPlainWords(tester);

    await tester.tap(find.text('I agree'));
    await tester.pumpAndSettle();
    expect(node.termsDue, isFalse);
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
    await tester.tap(find.text('I agree'));
    expect(node.termsDue, isFalse);
  });

  test('the app shows the same terms as docs/terms.md', () {
    final doc = File('../docs/terms.md').readAsStringSync();
    final bullets = [
      for (final line in doc.split('\n'))
        if (line.startsWith('- ')) line.substring(2),
    ];
    expect(bullets, termsOfUse);
    expect(doc, contains('Version $termsVersion,'));
  });
}
