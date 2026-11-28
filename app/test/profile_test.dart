import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/node.dart';
import 'package:osp_app/screens/edit_profile_screen.dart';
import 'package:osp_app/screens/found_people_screen.dart';
import 'package:osp_app/screens/profile_screen.dart';
import 'package:osp_app/src/rust/api/osp.dart';
import 'package:osp_app/uploads.dart';
import 'package:osp_app/widgets/person_avatar.dart';

import 'plain_words.dart';

/// The Rust side, as far as profiles go.
class FakeApp implements OspApp {
  final profiles = <String, Profile>{};
  final pages = <String, List<ProfilePage>>{};
  final asked = <String>[];
  final followed = <String>[];
  final trusted = <String>[];
  final saved = <Map<String, Object?>>[];

  @override
  Future<Profile> profile({required String identity}) async =>
      profiles[identity] ?? (throw Exception('no such person'));

  @override
  Future<ProfilePage> profileItems({
    required String identity,
    required String kind,
    String? before,
    required int limit,
  }) async {
    asked.add('$kind ${before ?? 'newest'}');
    final list = pages['$identity $kind'] ?? const [];
    final at = before == null ? 0 : int.parse(before);
    return at < list.length
        ? list[at]
        : const ProfilePage(posts: [], next: null);
  }

  @override
  Future<void> followPerson({required String identity}) async {
    followed.add(identity);
    profiles[identity] = withFollowed(profiles[identity]!);
  }

  @override
  Future<void> trust({required String identity}) async => trusted.add(identity);

  @override
  Future<Profile> setProfile({
    required String name,
    String? summary,
    required List<ProfileLinkEntry> links,
    String? photo,
    required bool removePicture,
  }) async {
    saved.add({
      'name': name,
      'summary': summary,
      'links': [for (final l in links) '${l.name} ${l.url}'],
      'photo': photo,
      'removePicture': removePicture,
    });
    return profileOf(
      'Me',
      name: name,
      summary: summary,
      links: links,
      own: true,
      picture: photo == null ? null : 'new-picture',
    );
  }

  @override
  Future<String> mediaUrl({required String blob, required String mediaType}) =>
      Future.error(Exception('not in tests'));

  @override
  Future<NodeStatus> status() => Future.error(Exception('not in tests'));

  /// Each "How are we connected?" asked (ADR 0037).
  final connectedAsked = <String>[];

  @override
  Future<Connection> howConnected({required String identity}) async {
    connectedAsked.add(identity);
    return Connection(
      identity: identity,
      paths: const [],
      people: const [],
      side: SideFrom.notAsked,
      vouches: 0,
    );
  }

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}

Profile profileOf(
  String who, {
  String? name,
  String? summary,
  List<ProfileLinkEntry> links = const [],
  bool own = false,
  bool followed = false,
  bool known = false,
  String? picture,
  int posts = 0,
  int media = 0,
}) => Profile(
  identity: 'id-$who',
  name: name ?? who,
  summary: summary,
  links: links,
  picture: picture,
  own: own,
  followed: followed,
  known: known,
  posts: posts,
  media: media,
  reshares: 0,
  followers: own ? 3 : null,
  following: own ? 1 : null,
);

Profile withFollowed(Profile p) => Profile(
  identity: p.identity,
  name: p.name,
  summary: p.summary,
  links: p.links,
  picture: p.picture,
  own: p.own,
  followed: true,
  known: p.known,
  posts: p.posts,
  media: p.media,
  reshares: p.reshares,
  followers: p.followers,
  following: p.following,
);

Post post(String text, {List<Media> media = const []}) => Post(
  regions: const [],
  id: 'id-$text',
  author: 'Jimmy',
  authorIdentity: 'id-Jimmy',
  published: '2026-01-01T00:00:00Z',
  text: text,
  media: media,
  resharerHosts: false,
  liked: false,
  opened: false,
);

const photo = Media(
  isVideo: false,
  blob: 'photo-blob',
  mediaType: 'image/webp',
  size: 1000,
  width: 800,
  height: 600,
  thumbnailBlob: 'thumb-blob',
  onNode: true,
);

void main() {
  late Directory root;
  late FakeApp app;
  late HomeNode node;

  setUp(() {
    root = Directory.systemTemp.createTempSync('profile_test');
    app = FakeApp();
    final uploads = UploadsFolder(
      Directory('${root.path}/uploads'),
      pickerDir: Directory('${root.path}/cache'),
    );
    node = HomeNode.forTests(app, uploads);
    node.paired = const NodeInfo(
      identity: 'id-Me',
      name: 'Me',
      paired: '2026-01-01T00:00:00Z',
    );
  });
  tearDown(() => root.deleteSync(recursive: true));

  Future<void> openProfile(WidgetTester tester, String who) async {
    await tester.pumpWidget(
      MaterialApp(
        home: ProfileScreen(node: node, identity: 'id-$who', name: who),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('found, opened and followed, all in the app (the P2 gate)', (
    tester,
  ) async {
    app.profiles['id-Jimmy'] = profileOf(
      'Jimmy',
      summary: 'I build boats',
      links: const [
        ProfileLinkEntry(name: 'Boats', url: 'https://example.org/boats'),
      ],
      posts: 1,
    );
    app.pages['id-Jimmy posts'] = [
      ProfilePage(posts: [post('A new hull')], next: null),
    ];
    node.people = [
      const FoundPerson(
        regions: [],
        identity: 'id-Jimmy',
        name: 'Jimmy',
        followed: false,
      ),
    ];
    await tester.pumpWidget(MaterialApp(home: FoundPeopleScreen(node: node)));
    await tester.pumpAndSettle();

    await tester.tap(find.text('Jimmy'));
    await tester.pumpAndSettle();
    expect(find.text('I build boats'), findsOneWidget);
    expect(find.text('Boats'), findsOneWidget);
    expect(find.text('https://example.org/boats'), findsOneWidget);
    expect(find.text('A new hull'), findsOneWidget);
    expect(find.textContaining('what your node has found'), findsOneWidget);
    // No follower counts for anyone but you (ADR 0032 §4).
    expect(find.textContaining('follower'), findsNothing);
    expect(find.textContaining('I know'), findsNothing);
    expectPlainWords(tester);

    await tester.tap(find.widgetWithText(FilledButton, 'Follow'));
    await tester.pumpAndSettle();
    expect(app.followed, ['id-Jimmy']);
    expect(find.text('Following'), findsWidgets);
    expect(find.text('I know Jimmy'), findsOneWidget);
    expect(find.text('Ask Jimmy to confirm they know you'), findsOneWidget);
    // Reloading the profile doesn't read a second page of posts.
    expect(app.asked, ['posts newest']);
    expectPlainWords(tester);

    await tester.tap(find.text('I know Jimmy'));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Yes, I know Jimmy'));
    await tester.pumpAndSettle();
    expect(app.trusted, ['id-Jimmy']);
  });

  testWidgets('opening a profile asks nobody; its button asks, for the map', (
    tester,
  ) async {
    app.profiles['id-Jimmy'] = profileOf('Jimmy', posts: 0);
    await openProfile(tester, 'Jimmy');
    expect(app.connectedAsked, isEmpty, reason: 'ADR 0037 §2');
    await tester.tap(find.text('How are we connected?'));
    await tester.pumpAndSettle();
    expect(app.connectedAsked, ['id-Jimmy']);
    expect(node.connection?.identity, 'id-Jimmy');
    expect(node.takeMapWanted(), isTrue, reason: 'shown on the map');
    expect(node.takeMapWanted(), isFalse, reason: 'taken once');
  });

  testWidgets('your own profile has no question of how you are connected', (
    tester,
  ) async {
    app.profiles['id-Me'] = profileOf('Me', own: true);
    await openProfile(tester, 'Me');
    expect(find.text('How are we connected?'), findsNothing);
  });

  testWidgets('lists a page at a time, and media as a grid', (tester) async {
    app.profiles['id-Jimmy'] = profileOf('Jimmy', posts: 2, media: 1);
    app.pages['id-Jimmy posts'] = [
      ProfilePage(posts: [post('second')], next: '1'),
      ProfilePage(posts: [post('first')], next: null),
    ];
    app.pages['id-Jimmy media'] = [
      ProfilePage(
        posts: [
          post('a photo', media: [photo]),
        ],
        next: null,
      ),
    ];
    await openProfile(tester, 'Jimmy');
    expect(find.text('second'), findsOneWidget);
    // The page itself, not the text boxes on it that scroll too.
    final page = find
        .descendant(
          of: find.byType(CustomScrollView),
          matching: find.byType(Scrollable),
        )
        .first;
    expect(find.text('first'), findsNothing);
    await tester.scrollUntilVisible(
      find.text('Show more'),
      200,
      scrollable: page,
    );
    await tester.tap(find.text('Show more'));
    await tester.pumpAndSettle();
    expect(find.text('first'), findsOneWidget);
    expect(find.text('Show more'), findsNothing);

    await tester.scrollUntilVisible(
      find.text('Media 1'),
      -200,
      scrollable: page,
    );
    await tester.tap(find.text('Media 1'));
    await tester.pumpAndSettle();
    expect(app.asked, ['posts newest', 'posts 1', 'media newest']);
    expect(find.byType(GridView), findsNothing);
    expect(find.byType(SliverGrid), findsOneWidget);
    expect(find.text('a photo'), findsNothing);
  });

  testWidgets('your own shows your counts and opens the editor', (
    tester,
  ) async {
    app.profiles['id-Me'] = profileOf('Me', own: true, summary: 'Hello');
    await openProfile(tester, 'Me');
    expect(find.text('3 followers · 1 following'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Follow'), findsNothing);

    await tester.tap(find.text('Edit your profile'));
    await tester.pumpAndSettle();
    expect(find.byType(EditProfileScreen), findsOneWidget);
    expectPlainWords(tester);
  });

  group('editing', () {
    Future<void> edit(WidgetTester tester, Profile profile) async {
      await tester.pumpWidget(
        MaterialApp(
          home: EditProfileScreen(
            node: node,
            profile: profile,
            pick: () async => '${root.path}/picked.jpg',
          ),
        ),
      );
      await tester.pumpAndSettle();
    }

    testWidgets('says the picture is public before one is chosen', (
      tester,
    ) async {
      await edit(tester, profileOf('Me', own: true));
      expect(find.textContaining('picture is public'), findsOneWidget);
      expect(find.text('Choose a photo'), findsOneWidget);
    });

    testWidgets('your letter is your star, as everywhere else', (
      tester,
    ) async {
      await edit(tester, profileOf('Me', own: true));
      InitialAvatar letter() =>
          tester.widget<InitialAvatar>(find.byType(InitialAvatar));
      expect(letter().yours, isTrue);
      expect(letter().seed, 'id-Me');
      // A new name changes the letter, not the colour.
      await tester.enterText(find.byType(TextField).first, 'Robin');
      await tester.pump();
      expect(letter().name, 'Robin');
      expect(letter().seed, 'id-Me');
    });

    testWidgets('saves a name, a summary, a link and a photo', (tester) async {
      File('${root.path}/picked.jpg').writeAsBytesSync([0xff, 0xd8, 0xff]);
      await edit(tester, profileOf('Me', own: true));
      await tester.enterText(find.widgetWithText(TextField, 'Name'), 'Kim K');
      await tester.enterText(
        find.widgetWithText(TextField, 'About you'),
        '  I make tables ',
      );
      await tester.ensureVisible(find.text('Add a link'));
      await tester.tap(find.text('Add a link'));
      await tester.pumpAndSettle();
      final fields = find.byType(TextField);
      await tester.enterText(fields.at(2), 'Site');
      await tester.enterText(fields.at(3), 'http://example.org');
      await tester.ensureVisible(find.text('Choose a photo'));
      await tester.tap(find.text('Choose a photo'));
      await tester.pumpAndSettle();
      expect(find.text('Choose another photo'), findsOneWidget);

      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(app.saved, isEmpty);
      expect(find.text('Links must start with https://'), findsOneWidget);

      await tester.enterText(fields.at(3), 'https://example.org');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(app.saved, [
        {
          'name': 'Kim K',
          'summary': 'I make tables',
          'links': ['Site https://example.org'],
          'photo': '${root.path}/picked.jpg',
          'removePicture': false,
        },
      ]);
      expect(node.me?.name, 'Kim K');
    });

    testWidgets('takes the picture off', (tester) async {
      await edit(tester, profileOf('Me', own: true, picture: 'old'));
      expect(find.text('Choose another photo'), findsOneWidget);
      await tester.tap(find.text('No picture'));
      await tester.pumpAndSettle();
      expect(find.text('Choose a photo'), findsOneWidget);
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(app.saved.single['removePicture'], true);
      expect(app.saved.single['photo'], null);
    });

    testWidgets('needs a name', (tester) async {
      await edit(tester, profileOf('Me', own: true));
      await tester.enterText(find.widgetWithText(TextField, 'Name'), '  ');
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(app.saved, isEmpty);
      expect(find.text('Your profile needs a name.'), findsOneWidget);
    });
  });
}
