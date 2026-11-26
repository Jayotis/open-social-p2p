import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/carrying_screen.dart';
import 'package:osp_app/screens/hops_screen.dart';
import 'package:osp_app/screens/node_summary_screen.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';

import 'plain_words.dart';

/// The Rust side: Jimmy's node, which carries for others and keeps what
/// the owner changes, as the node does.
class FakeApp implements OspApp {
  bool hasCarrier = true;
  bool paused = false;
  bool metered = false;
  int jobs = 2;
  int share = 50;
  int dataCapGb = 0;
  int? monthlyGb;
  bool? hops;
  int seeds = 1;
  int bytes = 0;

  /// Set, the node refuses every change with it.
  String? refuse;
  final changes = <String>[];

  CarrierState get carrier => CarrierState(
    carrying: !paused && !metered,
    paused: paused,
    metered: metered,
    seed: false,
    jobs: jobs,
    uploadShare: share,
    uploadMbps: 20,
    uploadFrom: 'set',
    circuits: 0,
    bytes: bytes,
    hops: hops,
    monthBytes: 0,
    monthBudget: monthlyGb != null
        ? (monthlyGb == 0 ? null : monthlyGb! * 1000000000)
        : (dataCapGb == 0 ? null : dataCapGb ~/ 2 * 1000000000),
    overBudget: false,
    dataCapGb: dataCapGb,
    monthlyGb: monthlyGb,
    seeds: seeds,
    standing: 0,
    probation: 0,
  );

  @override
  Future<NodeStatus> status() async => NodeStatus(
    name: 'Jimmy',
    version: '0.1.0',
    uptimeSecs: 3600,
    follows: 2,
    followers: 1,
    devices: 1,
    relayConnected: true,
    problems: const [],
    path: 'direct',
    trustRequests: 0,
    replyRequests: 0,
    activityUnread: 0,
    overTor: false,
    torStarting: false,
    hostedPosts: 0,
    servedLatelyBytes: 0,
    spaceBytes: 0,
    lookedForPeople: true,
    carrier: hasCarrier ? carrier : null,
  );

  @override
  Future<void> setCarrying({
    bool? paused,
    bool? metered,
    int? jobs,
    int? uploadShare,
    int? uploadMbps,
    int? monthlyGb,
    int? dataCapGb,
  }) async {
    if (refuse != null) throw Exception(refuse);
    if (paused != null) {
      this.paused = paused;
      changes.add('paused $paused');
    }
    if (metered != null) {
      this.metered = metered;
      changes.add('metered $metered');
    }
    if (jobs != null) {
      this.jobs = jobs;
      changes.add('jobs $jobs');
    }
    if (uploadShare != null) {
      share = uploadShare;
      changes.add('share $uploadShare');
    }
    if (monthlyGb != null) {
      this.monthlyGb = monthlyGb;
      changes.add('monthly $monthlyGb');
    }
    if (dataCapGb != null) {
      this.dataCapGb = dataCapGb;
      changes.add('cap $dataCapGb');
    }
  }

  @override
  Future<void> setHops({required bool yes}) async {
    if (refuse != null) throw Exception(refuse);
    hops = yes;
    changes.add('hops $yes');
  }

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

void main() {
  late Directory root;
  late FakeApp app;
  late HomeNode node;

  setUp(() async {
    root = Directory.systemTemp.createTempSync('carrying_test');
    app = FakeApp();
    node = HomeNode.forTests(
      app,
      UploadsFolder(
        Directory('${root.path}/uploads'),
        pickerDir: Directory('${root.path}/cache'),
      ),
    );
    node.paired = const NodeInfo(
      name: 'Jimmy',
      identity: 'id-Jimmy',
      paired: '2026-11-21T00:00:00Z',
    );
    await node.checkStatus();
  });
  tearDown(() {
    node.dispose();
    root.deleteSync(recursive: true);
  });

  Future<void> show(WidgetTester tester, Widget screen) async {
    await tester.pumpWidget(MaterialApp(home: screen));
    await tester.pumpAndSettle();
  }

  test(
    'the question is due on a node not asked that knows seed carriers',
    () async {
      expect(node.hopsDue, isTrue);

      // A node that knows none has nothing to go through yet.
      app.seeds = 0;
      await node.checkStatus();
      expect(node.hopsDue, isFalse);

      // One from before carrying's controls doesn't say.
      app
        ..seeds = 1
        ..hasCarrier = false;
      await node.checkStatus();
      expect(node.hopsDue, isFalse);

      app.hasCarrier = true;
      await node.answerHops(false);
      expect(node.hopsDue, isFalse);
    },
  );

  testWidgets('Jimmy answers yes, and his node has the answer', (tester) async {
    await show(tester, HopsScreen(node: node));
    expect(find.text(hopsQuestion), findsOne);
    expectPlainWords(tester);

    await tester.tap(find.text('Yes, go through them'));
    await tester.pumpAndSettle();
    expect(app.changes, ['hops true']);
    expect(node.hopsDue, isFalse);
  });

  testWidgets('Your node has a Carrying line that opens Carrying', (
    tester,
  ) async {
    app.bytes = 3 * 1000 * 1000 * 1000;
    await node.checkStatus();
    await show(tester, NodeSummaryScreen(node: node, open: (_) {}));
    expect(
      find.textContaining('carried 2.8 GB of encrypted video for other nodes'),
      findsOne,
    );
    expectPlainWords(tester);

    Widget? opened;
    await show(
      tester,
      NodeSummaryScreen(node: node, open: (screen) => opened = screen),
    );
    await tester.tap(find.text('Carrying: limits and your own video'));
    expect(opened, isA<CarryingScreen>());
  });

  testWidgets('each control on Carrying changes the node', (tester) async {
    tester.view.physicalSize = const Size(1080, 6000);
    addTearDown(tester.view.reset);
    await show(tester, CarryingScreen(node: node));
    expect(find.text('Ready to carry. Nothing right now.'), findsOne);
    expectPlainWords(tester);

    await tester.tap(find.text('Pause carrying'));
    await tester.pumpAndSettle();
    expect(find.text('Carrying is paused.'), findsOne);

    await tester.tap(find.text('My connection is metered'));
    await tester.pumpAndSettle();

    await tester.tap(find.byTooltip('More'));
    await tester.pumpAndSettle();
    expect(find.text('3 videos at a time'), findsOne);

    // The plan's cap sets carrying's month to half of it.
    expect(find.textContaining('No limit'), findsOne);
    await tester.tap(find.text("Your plan's monthly data cap"));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), '200');
    await tester.pump();
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    expect(find.text('200 GB a month'), findsOne);
    expect(find.textContaining("Up to 100 GB, half your plan's cap"), findsOne);

    // Its own budget instead.
    await tester.tap(find.text('Carrying a month'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), '30');
    await tester.pump();
    await tester.tap(find.text('Save'));
    await tester.pumpAndSettle();
    expect(find.textContaining('Up to 30 GB, as you set'), findsOne);

    await tester.drag(find.byType(Slider), const Offset(-2000, 0));
    await tester.pumpAndSettle();

    await tester.tap(find.text('No, use the fallback'));
    await tester.pumpAndSettle();

    expect(app.changes, [
      'paused true',
      'metered true',
      'jobs 3',
      'cap 200',
      'monthly 30',
      'share 5',
      'hops false',
    ]);
    expectPlainWords(tester);
  });

  testWidgets("a change the node refuses says so, and shows what it has", (
    tester,
  ) async {
    await show(tester, CarryingScreen(node: node));
    app.refuse = 'jobs at once go from 1 to 100';
    await tester.tap(find.byTooltip('More'));
    await tester.pumpAndSettle();
    expect(find.textContaining('Not changed'), findsOne);
    expect(find.text('2 videos at a time'), findsOne);
  });
}
