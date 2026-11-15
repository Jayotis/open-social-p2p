import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/network_events.dart';
import 'package:osp_app/screens/universe_tab.dart';
import 'package:osp_app/src/rust/api/osp.dart';

import 'event_fixture.dart';
import 'plain_words.dart';
import 'shell_test.dart' show world;

void main() {
  final now = DateTime(2026, 11, 15, 12);

  group('which events are drawn (ADR 0036 §1)', () {
    test('the first look draws the last hour, and holds the day', () {
      final events = NetworkEvents()
        ..take([
          anEvent(1, EventKind.served, ago: const Duration(hours: 5), now: now),
          anEvent(
            2,
            EventKind.found,
            ago: const Duration(minutes: 20),
            now: now,
          ),
        ], now: now);
      expect(events.since, BigInt.two);
      expect(events.today.map((e) => e.seq.toInt()), [2, 1]);
      expect(events.next?.seq.toInt(), 2);
      events.drew();
      expect(events.next, isNull);
    });

    test('an event summed into comes again, and is not drawn again', () {
      final events = NetworkEvents();
      final first = anEvent(
        1,
        EventKind.served,
        author: 'id-Jimmy',
        post: 'p1',
        now: now,
      );
      events.take([first], now: now);
      events.drew(now: now);
      final later = now.add(const Duration(minutes: 1));
      final summed = anEvent(
        2,
        EventKind.served,
        author: 'id-Jimmy',
        post: 'p1',
        count: 2,
        lasted: const Duration(minutes: 1),
        now: later,
      );
      events.take([summed], now: later);
      expect(events.next, isNull, reason: 'drawn once');
      expect(events.grown?.seq, BigInt.two, reason: 'told again, grown');
      expect(events.latest(first).count, 2);
      expect(events.today.single.count, 2, reason: 'the list has the sum');
      expect(events.since, BigInt.two);
    });

    test('a node restored afresh, sending its day again, replays nothing', () {
      final events = NetworkEvents()
        ..take([anEvent(9, EventKind.served, now: now)], now: now)
        ..drew();
      final later = now.add(const Duration(hours: 3));
      events.take([
        anEvent(1, EventKind.found, ago: const Duration(hours: 2), now: later),
      ], now: later);
      expect(events.next, isNull);
      expect(events.today, hasLength(2), reason: 'both listed');
    });

    test('at most a few wait, the newest kept', () {
      final events = NetworkEvents()
        ..take([
          for (var i = 1; i <= 40; i++)
            anEvent(i, EventKind.served, post: 'p$i', now: now),
        ], now: now);
      expect(events.next?.seq.toInt(), 40 - NetworkEvents.maxWaiting + 1);
    });
  });

  test('every event is told in plain words (ADR 0031 §2)', () {
    final names = EventNames(world(['Jimmy', 'Kim']).people);
    final lines = [
      for (final e in [
        anEvent(1, EventKind.found, words: 'amber-fox', people: 12),
        anEvent(2, EventKind.reshared, from: 'id-Kim', author: 'id-Jimmy'),
        anEvent(
          3,
          EventKind.reshared,
          from: 'id-Kim',
          author: 'id-Alice',
          count: 3,
        ),
        anEvent(4, EventKind.fetched, author: 'id-Jimmy', bytes: 2 << 20),
        anEvent(
          5,
          EventKind.fetched,
          author: 'id-Jimmy',
          host: 'id-Kim',
          keptByCopy: true,
        ),
        anEvent(
          6,
          EventKind.served,
          author: 'id-Jimmy',
          bytes: 4 << 20,
          count: 14,
        ),
        anEvent(7, EventKind.served, author: 'id-nobody'),
        anEvent(8, EventKind.machine, words: 'amber-fox', met: true),
        anEvent(9, EventKind.machine, words: 'amber-fox'),
      ])
        describeEvent(e, names),
    ];
    expect(lines, [
      'Your node found 12 new people, listed by amber-fox',
      'Kim reshared Jimmy\'s post',
      'Kim reshared your posts 3 times',
      'Your node fetched 2.0 MB of Jimmy\'s post from another node',
      'Your node fetched 0 B of Jimmy\'s post from Kim, who keeps it '
          'available. Kept available by a copy while its author\'s node was '
          'away',
      'Your node sent Jimmy\'s post to other nodes 14 times, 4.0 MB',
      'Your node sent 0 B of someone\'s post to another node',
      'Your node met the machine amber-fox',
      'The machine amber-fox stopped answering',
    ]);
    for (final line in lines) {
      expect(protocolWords.hasMatch(line), isFalse, reason: line);
    }
  });

  group('what an event draws: only what it names (ADR 0036 §2)', () {
    final s = world(['Jimmy', 'Kim']);
    test('a reshare between people on the map is a line from the author', () {
      final marks = eventMarks(
        anEvent(1, EventKind.reshared, from: 'id-Kim', author: 'id-Jimmy'),
        s,
      );
      final line = marks.single as EventLine;
      expect(line.from, const Offset(5, 0));
      expect(line.to, const Offset(10, 0));
    });

    test('someone not on the map draws nothing', () {
      expect(
        eventMarks(
          anEvent(1, EventKind.reshared, from: 'id-Zed', author: 'id-Jimmy'),
          s,
        ),
        isEmpty,
      );
    });

    test('a fetch flows in from its host, or from another node', () {
      final hosted =
          eventMarks(
                anEvent(1, EventKind.fetched, host: 'id-Kim', keptByCopy: true),
                s,
              ).single
              as EventFlow;
      expect(
        (hosted.from, hosted.to, hosted.kept),
        (const Offset(10, 0), Offset.zero, true),
      );
      final other =
          eventMarks(anEvent(2, EventKind.fetched, post: 'p'), s).single
              as EventFlow;
      expect((other.from, other.to), (null, Offset.zero));
    });

    test('a copy served flows out from your star', () {
      final flow =
          eventMarks(anEvent(1, EventKind.served, post: 'p'), s).single
              as EventFlow;
      expect((flow.from, flow.to), (Offset.zero, null));
    });

    test('people found ring where they are, or the sky they went to', () {
      expect(
        (eventMarks(anEvent(1, EventKind.found, words: 'amber-fox'), s).single
                as EventRing)
            .at,
        isNull,
      );
    });

    test('a machine is drawn on its chip, not the map', () {
      expect(
        eventMarks(anEvent(1, EventKind.machine, words: 'a', met: true), s),
        isEmpty,
      );
    });
  });
}
