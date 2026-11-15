import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/connection_lights.dart';
import 'package:osp_app/src/rust/api/osp.dart';

import 'plain_words.dart';

PathHop hop(String who, HopKind how, {int steps = 1}) =>
    PathHop(identity: who, how: how, steps: steps);

ConnectedPerson named(String who, String? name, {List<String>? regions}) =>
    ConnectedPerson(identity: who, name: name, regions: regions ?? const []);

UniversePerson person(String who, double x, {int? step = 1}) => UniversePerson(
  lit: 0,
  identity: who,
  name: who,
  step: step,
  followed: false,
  known: false,
  posts: 0,
  media: 0,
  reshares: 0,
  x: x,
  y: 0,
  group: step == 0 ? null : BigInt.one,
  bright: 0,
);

UniverseSnapshot shown(
  List<UniversePerson> people, {
  List<UniverseGroup>? groups,
}) => UniverseSnapshot(
  version: 'v',
  level: UniverseLevel.local,
  region: '',
  path: const [],
  groups: groups ?? const [],
  people: [person('kim', 0, step: 0), ...people],
  lines: const [],
  morePeople: 0,
  machines: const [],
  sky: const [],
);

/// ADR 0037's paths: Kim follows Alice, who reshared Carol, who reshared
/// Jimmy; Kim knows Sarah, whose node says Emma is within 2 steps of her,
/// and David credited Jimmy with finding his post early.
ConnectionLights adrPaths() => ConnectionLights(
  'jimmy',
  'Jimmy',
  Connection(
    identity: 'jimmy',
    paths: [
      ConnectionPath(
        hops: [
          hop('alice', HopKind.follows),
          hop('carol', HopKind.reshared),
          hop('jimmy', HopKind.reshared),
        ],
      ),
      ConnectionPath(
        hops: [
          hop('sarah', HopKind.knows),
          hop('emma', HopKind.gap, steps: 2),
          hop('david', HopKind.reshared),
          hop('jimmy', HopKind.creditedEarly),
        ],
      ),
    ],
    people: [
      named('jimmy', 'Jimmy'),
      named('alice', 'Alice'),
      named('carol', 'Carol', regions: ['', 'g:4']),
      named('sarah', 'Sarah'),
      named('emma', null),
      named('david', 'David'),
    ],
    side: SideFrom.asked,
    vouches: 2,
  ),
);

void main() {
  test('each path reads as sentences, in plain words', () {
    final c = adrPaths();
    final words = [for (final p in c.paths) c.words(p)];
    expect(words, [
      'You follow Alice. Alice reshared Carol. Carol reshared Jimmy.',
      'You know Sarah. Sarah’s node says someone is within 2 steps of Sarah. '
          'Someone reshared David. David credited Jimmy with finding their '
          'post early.',
    ]);
    expect(c.source, 'Their node named 2 people who vouched for them');
    for (final w in [...words, c.source]) {
      expect(protocolWords.hasMatch(w), isFalse, reason: w);
    }
  });

  test('a hop is drawn only where the map shows both its ends', () {
    final c = adrPaths();
    // Alice and Jimmy are shown, Carol only as the closed group holding
    // her; Sarah, Emma and David aren't on this map at all.
    final s = shown(
      [person('alice', 5), person('jimmy', 20)],
      groups: [
        UniverseGroup(
          lit: 0,
          id: BigInt.from(4),
          people: 3,
          bright: 0,
          x: 10,
          y: 0,
        ),
      ],
    );
    final lines = [for (final l in c.lines(s)) (l.from, l.to, l.how)];
    expect(lines, [
      (Offset.zero, const Offset(5, 0), HopKind.follows),
      (const Offset(5, 0), const Offset(10, 0), HopKind.reshared),
      (const Offset(10, 0), const Offset(20, 0), HopKind.reshared),
    ]);
    expect(c.holdsPerson(s.people[1]), isTrue);
    expect(c.holdsGroup(s.groups.single), isTrue);
    expect(c.holdsPerson(s.people.first), isFalse, reason: 'her own star');
  });

  test('a gap is drawn as a gap, between the ones shown', () {
    final c = adrPaths();
    final s = shown([person('sarah', 3), person('emma', 9)]);
    final gap = c.lines(s).singleWhere((l) => l.how == HopKind.gap);
    expect((gap.from, gap.to), (const Offset(3, 0), const Offset(9, 0)));
  });

  test('while asking, nothing is drawn and it says it is looking', () {
    final c = ConnectionLights.asking('jimmy', 'Jimmy');
    expect(c.asking, isTrue);
    expect(c.lines(shown([person('jimmy', 4)])), isEmpty);
    expect(c.source, 'Looking for how you’re connected…');
  });
}
