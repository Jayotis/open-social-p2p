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
  final followedPeople = <String>[];
  final codesFor = <(String, String?)>[];
  final openedCodes = <String>[];
  final forgotten = <String>[];

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

  /// Ann's post, fetched from her node as an opened post.
  @override
  Future<Post> openPost({required String code}) async {
    openedCodes.add(code);
    return post('From Ann', opened: true);
  }

  @override
  Future<void> forgetOpened({required String postId}) async =>
      forgotten.add(postId);

  @override
  Future<void> followPerson({required String identity}) async =>
      followedPeople.add(identity);

  @override
  Future<Thread> thread({required String postId, required int after}) async =>
      const Thread(
        policy: 'network',
        replies: [],
        more: false,
        likes: 0,
        liked: false,
      );

  @override
  Future<List<SentReply>> sentReplies() async => const [];

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

Post post(String text, {bool opened = false}) => Post(
  regions: const [],
  id: 'id-post',
  author: 'Ann',
  authorIdentity: 'id-Ann',
  published: '2026-01-01T00:00:00Z',
  text: text,
  media: const [],
  resharerHosts: false,
  liked: false,
  opened: opened,
);

void main() {
  group('share texts', () {
    test("a profile: who, how to get OSP, and the code's link last", () {
      final text = personShareText(name: 'Kim', code: kimCode);
      expect(text, startsWith('Kim is on Open Social P2P.'));
      expect(text, contains(installGuide));
      expect(text.split('\n').last, 'https://osp2p.com/o#$kimCode');
      expect(protocolWords.hasMatch(words(text)), isFalse);
      final own = personShareText(name: '', code: kimCode, own: true);
      expect(own, startsWith("I'm on Open Social P2P. To follow me"));
    });

    test("a post: its first lines, its author, and the code's link last", () {
      final text = postShareText(
        author: 'Ann',
        text: 'First line\n\nsecond line\nthird line',
        code: postCode,
      );
      expect(text, startsWith('“First line\nsecond line…”\nAnn, on Open'));
      expect(text.split('\n').last, codeLink(postCode));
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

    test('a tapped link is read with its escapes undone', () {
      final link = Uri.parse(codeLink(postCode));
      expect(linkText(link), codeLink(postCode));
      final escaped = Uri.parse(
        'https://osp2p.com/o#osp%3Apost%2Fabc%3Fby%3DOSP1ANN',
      );
      expect(linkText(escaped), codeLink(postCode));
      expect(linkText(Uri.parse('osp:person/OSP1KIM')), kimCode);
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
      expect(shared.single, endsWith(codeLink(postCode)));
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
      "a post not on the node opens from its author's node, labelled, is "
      'gone when Kim leaves it, and stays once she follows',
      (tester) async {
        const code = SharedCode(
          code: postCode,
          invite: 'OSP1ANN',
          actor: 'id-Ann',
          post: 'abc',
        );
        await open(tester, code);
        expect(app.openedCodes, [postCode]);
        expect(find.text('From Ann'), findsOneWidget);
        expect(find.text('Opened from a shared OSP post'), findsOneWidget);
        expectPlainWords(tester);
        // Shown once: no liking or replying, and no question first.
        expect(find.byTooltip('Send reply'), findsNothing);
        expect(find.byIcon(Icons.favorite_border), findsNothing);
        expect(app.followed, isEmpty);

        // Leaving drops it from the node.
        await tester.pageBack();
        await tester.pumpAndSettle();
        expect(app.forgotten, ['id-post']);

        // Opened again and followed: it's an ordinary post, and stays.
        await tester.tap(find.text('open'));
        await tester.pumpAndSettle();
        await tester.tap(find.text('Follow Ann'));
        await tester.pumpAndSettle();
        expect(app.followedPeople, ['id-Ann']);
        expect(find.text('Opened from a shared OSP post'), findsNothing);
        expect(find.byTooltip('Send reply'), findsOneWidget);
        await tester.pageBack();
        await tester.pumpAndSettle();
        expect(app.forgotten, ['id-post']);
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
