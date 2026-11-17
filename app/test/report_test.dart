import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/report.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';
import 'package:osp_app/widgets/post_card.dart';

class FakeApp implements OspApp {
  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

Post post(String author) => Post(
  regions: const [],
  id: 'post-1',
  author: author,
  authorIdentity: 'id-$author',
  published: '2026-01-01T00:00:00Z',
  text: 'A walk by the river',
  media: const [],
  resharerHosts: false,
  liked: false,
  opened: true,
);

void main() {
  test('what is reported goes after the #, never to the server', () {
    final page = reportUri(post: 'post-1');
    expect(page.toString(), 'https://osp2p.com/report#post=post-1');
    expect(page.query, isEmpty);
    expect(reportUri(account: 'id-Kim').fragment, 'account=id-Kim');
    expect(reportUri(reply: 'r/1').toString(), endsWith('#reply=r%2F1'));
  });

  group('on a post', () {
    late Directory root;
    late HomeNode node;

    setUp(() {
      root = Directory.systemTemp.createTempSync('report_test');
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
    });
    tearDown(() {
      node.dispose();
      root.deleteSync(recursive: true);
    });

    Future<void> openMenu(WidgetTester tester, Post shown) async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: PostCard(node: node, post: shown),
            ),
          ),
        ),
      );
      await tester.tap(find.byTooltip('More'));
      await tester.pumpAndSettle();
    }

    testWidgets("someone else's, with the node out of reach: Report still "
        'works', (tester) async {
      node.reachability = Reachability.offline;
      await openMenu(tester, post('Kim'));
      final report = find.ancestor(
        of: find.text('Report'),
        matching: find.byType(PopupMenuItem<String>),
      );
      expect(tester.widget<PopupMenuItem<String>>(report).enabled, isTrue);
      final share = find.ancestor(
        of: find.text('Share'),
        matching: find.byType(PopupMenuItem<String>),
      );
      expect(tester.widget<PopupMenuItem<String>>(share).enabled, isFalse);
    });

    testWidgets('their own: no Report', (tester) async {
      await openMenu(tester, post('Alice'));
      expect(find.text('Share'), findsOne);
      expect(find.text('Report'), findsNothing);
    });
  });
}
