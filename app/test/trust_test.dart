import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/trust_screen.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';

/// The Rust side, as far as trusted followers go.
class FakeApp implements OspApp {
  TrustList list = const TrustList(requests: [], trusted: []);
  final answers = <(String, bool)>[];
  final untrusted = <String>[];

  @override
  Future<TrustList> trustList() async => list;

  @override
  Future<void> answerTrust({required String node, required bool allow}) async {
    answers.add((node, allow));
    final r = list.requests.firstWhere((r) => r.node == node);
    list = TrustList(
      requests: list.requests.where((r) => r.node != node).toList(),
      trusted: allow ? [r, ...list.trusted] : list.trusted,
    );
  }

  @override
  Future<void> untrust({required String identity}) async {
    untrusted.add(identity);
    list = TrustList(
      requests: list.requests,
      trusted: list.trusted.where((t) => t.identity != identity).toList(),
    );
  }

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

TrustEntry entry(
  String name, {
  String note = '',
  bool followed = false,
  String reached = 'direct',
}) => TrustEntry(
  node: 'node-$name',
  identity: 'id-$name',
  name: name,
  note: note,
  secsAgo: 7200,
  followed: followed,
  trustedElsewhere: false,
  followerSecs: 3 * 86400,
  reached: reached,
  copies: 4,
);

void main() {
  late Directory root;
  late FakeApp app;
  late HomeNode node;

  setUp(() {
    root = Directory.systemTemp.createTempSync('trust_test');
    app = FakeApp();
    final uploads = UploadsFolder(
      Directory('${root.path}/uploads'),
      pickerDir: Directory('${root.path}/cache'),
    );
    node = HomeNode.forTests(app, uploads);
  });
  tearDown(() => root.deleteSync(recursive: true));

  Future<void> open(WidgetTester tester) async {
    // Tall enough to show both lists at once.
    await tester.binding.setSurfaceSize(const Size(800, 2000));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.pumpWidget(MaterialApp(home: TrustScreen(node: node)));
    await tester.pumpAndSettle();
  }

  testWidgets('a request shows its claims and is allowed after a warning', (
    tester,
  ) async {
    app.list = TrustList(
      requests: [
        entry('Sam', note: 'Sam, from the climbing club'),
        entry('Kim', followed: true, reached: 'none'),
      ],
      trusted: const [],
    );
    await open(tester);

    expect(find.text('They say: "Sam, from the climbing club"'), findsOne);
    expect(find.textContaining("You don't follow them"), findsOne);
    expect(find.textContaining('You follow them, so this is'), findsOne);
    expect(
      find.text(
        'follows you for 3 d 0 h · reached your node directly lately · '
        'holds 4 of your files',
      ),
      findsOne,
    );
    expect(find.textContaining('not heard from lately'), findsOne);
    expect(find.textContaining('Nobody yet'), findsOne);

    // Allowing says what it changes first; cancelling does nothing.
    await tester.tap(find.text('Allow').first);
    await tester.pumpAndSettle();
    expect(find.text('Trust Sam?'), findsOne);
    expect(
      find.textContaining('can see that this person follows you'),
      findsOne,
    );
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(app.answers, isEmpty);

    await tester.tap(find.text('Allow').first);
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Trust'));
    await tester.pumpAndSettle();
    expect(app.answers, [('node-Sam', true)]);
    expect(find.textContaining("Trusted. They aren't told."), findsOne);
    expect(find.textContaining('They say:'), findsNothing);
    expect(find.text('Sam'), findsOne);

    // Declining needs no warning.
    await tester.tap(find.text('Decline'));
    await tester.pumpAndSettle();
    expect(app.answers.last, ('node-Kim', false));
    expect(find.text('No requests waiting.'), findsOne);
  });

  testWidgets('trust is taken back after asking', (tester) async {
    app.list = TrustList(requests: const [], trusted: [entry('Sam')]);
    await open(tester);

    await tester.tap(find.byTooltip('Stop trusting'));
    await tester.pumpAndSettle();
    expect(find.text('Stop trusting Sam?'), findsOne);
    await tester.tap(find.widgetWithText(FilledButton, 'Stop trusting'));
    await tester.pumpAndSettle();
    expect(app.untrusted, ['id-Sam']);
    expect(find.textContaining('Nobody yet'), findsOne);
  });
}
