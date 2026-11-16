import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/osp_codes.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';
import 'package:osp_app/widgets/post_card.dart';

import 'plain_words.dart';

const kimCode = 'osp:person/OSP1KIM';

/// [text] without the install guide's link, whose path has "blob" in it.
String words(String text) => text.replaceAll(installGuide, '');
const postCode = 'osp:post/abc?by=OSP1ANN';

/// The node, as far as OSP codes go: it holds one post, and follows nobody.
class FakeApp implements OspApp {
  final followed = <String>[];
  final codesFor = <(String, String?)>[];

  @override
  Future<String> codeForPerson({required String identity}) async {
    codesFor.add((identity, null));
    return kimCode;
  }

  @override
  Future<String> codeForPost({
    required String postId,
    required String author,
  }) async {
    codesFor.add((author, postId));
    return postCode;
  }

  @override
  Future<Post> showPost({required String id}) async =>
      throw Exception('unknown object $id');

  @override
  Future<List<Followed>> following() async => const [];

  @override
  Future<void> follow({required String ticket}) async => followed.add(ticket);

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

Post post(String text) => Post(
  regions: const [],
  id: 'id-post',
  author: 'Ann',
  authorIdentity: 'id-Ann',
  published: '2026-01-01T00:00:00Z',
  text: text,
  media: const [],
  resharerHosts: false,
  liked: false,
);

void main() {
  group('share texts', () {
    test('a profile: who, how to get OSP, and the code last', () {
      final text = personShareText(name: 'Kim', code: kimCode);
      expect(text, startsWith('Kim is on Open Social P2P.'));
      expect(text, contains(installGuide));
      expect(text.split('\n').last, kimCode);
      expect(protocolWords.hasMatch(words(text)), isFalse);
      final own = personShareText(name: '', code: kimCode, own: true);
      expect(own, startsWith("I'm on Open Social P2P. To follow me"));
    });

    test('a post: its first lines, its author, and the code last', () {
      final text = postShareText(
        author: 'Ann',
        text: 'First line\n\nsecond line\nthird line',
        code: postCode,
      );
      expect(text, startsWith('“First line\nsecond line…”\nAnn, on Open'));
      expect(text.split('\n').last, postCode);
      expect(protocolWords.hasMatch(words(text)), isFalse);
      // A post of photos alone has no quote.
      final bare = postShareText(author: '', text: '', code: postCode);
      expect(bare, startsWith('Someone, on Open Social P2P.'));
    });

    test('first lines are cut at 140 characters', () {
      final long = 'a' * 200;
      expect(firstLines(long), '${'a' * 140}…');
      expect(firstLines('short'), 'short');
    });

    test('telling someone about OSP gives the install guide', () {
      expect(aboutOspText(), contains(installGuide));
      expect(protocolWords.hasMatch(words(aboutOspText())), isFalse);
    });

    test('the word that is a code is picked out of a message', () {
      expect(codePart('open this: osp:post/x?by=y please'), 'osp:post/x?by=y');
      expect(codePart('no code'), 'no code');
    });
  });

  group('in the app', () {
    late Directory root;
    late FakeApp app;
    late HomeNode node;
    late List<String> shared;
    late List<String> subjects;

    setUp(() {
      root = Directory.systemTemp.createTempSync('codes_test');
      app = FakeApp();
      node = HomeNode.forTests(
        app,
        UploadsFolder(
          Directory('${root.path}/uploads'),
          pickerDir: Directory('${root.path}/cache'),
        ),
      );
      node.reachability = Reachability.online;
      shared = [];
      subjects = [];
      shareSheet = (text, subject) async {
        shared.add(text);
        subjects.add(subject);
      };
    });
    tearDown(() => root.deleteSync(recursive: true));

    testWidgets("a post's menu shares it by its author's code", (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: PostCard(node: node, post: post('Hello there')),
          ),
        ),
      );
      await tester.tap(find.byTooltip('More'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Share'));
      await tester.pumpAndSettle();
      expect(app.codesFor, [('id-Ann', 'id-post')]);
      expect(shared, hasLength(1));
      expect(shared.single, startsWith('“Hello there”\nAnn, on Open'));
      expect(shared.single, endsWith(postCode));
      expect(subjects, ['A post by Ann on Open Social P2P']);
    });

    /// Opens [code] from a button, as a paste or a link does.
    Future<void> open(WidgetTester tester, SharedCode code) async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) => TextButton(
                onPressed: () => openOspCode(context, node, code),
                child: const Text('open'),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('open'));
      await tester.pumpAndSettle();
    }

    testWidgets(
      "a post not on the node offers to follow its author, and only follows "
      'when asked',
      (tester) async {
        const code = SharedCode(
          code: postCode,
          invite: 'OSP1ANN',
          actor: 'id-Ann',
          post: 'abc',
        );
        await open(tester, code);
        expect(find.text('Follow its author?'), findsOneWidget);
        expectPlainWords(tester);
        await tester.tap(find.text('Not now'));
        await tester.pumpAndSettle();
        expect(app.followed, isEmpty);

        await tester.tap(find.text('open'));
        await tester.pumpAndSettle();
        await tester.tap(find.text('Follow'));
        await tester.pump();
        expect(app.followed, ['OSP1ANN']);
      },
    );

    testWidgets('your own code says so, and follows nobody', (tester) async {
      node.paired = const NodeInfo(
        name: 'Kim',
        identity: 'id-Kim',
        paired: '2026-01-01',
      );
      const code = SharedCode(
        code: kimCode,
        invite: 'OSP1KIM',
        actor: 'id-Kim',
      );
      await open(tester, code);
      expect(find.text("That's your own code."), findsOneWidget);
      expect(app.followed, isEmpty);
    });
  });
}
