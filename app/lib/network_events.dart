/// What the owner's node did or saw lately (ADR 0036 §1), as the phone
/// holds it: the day's events, which of them to draw on the map, and each
/// one in plain words.
library;

import 'package:clock/clock.dart';

import 'format.dart';
import 'src/rust/api/osp.dart';

/// The day's events from the node, and the ones still to be drawn.
///
/// Each is drawn once (ADR 0036 §1): an event summed into on the node
/// comes again with a new `seq` but the same thing and start, so it isn't
/// drawn again. Only those from the last hour are drawn at all: the first
/// look shows what just happened, and a node restored afresh, which sends
/// its whole day again, doesn't replay it.
class NetworkEvents {
  /// How recent an event must be to be drawn.
  static const recent = Duration(hours: 1);

  /// At most this many wait to be drawn; the oldest go first.
  static const maxWaiting = 12;

  /// How long after it's drawn an event that grows is told again.
  static const growing = Duration(minutes: 2);

  final Map<String, UniverseEvent> _byKey = {};
  final Set<String> _drawn = {};
  final Map<String, DateTime> _drawnAt = {};

  /// An event drawn lately that has grown since: its line says it again,
  /// with the new total, but it isn't drawn again.
  UniverseEvent? grown;
  final List<UniverseEvent> _waiting = [];

  /// The highest `seq` held: what to ask `since`.
  BigInt? since;

  /// The day's events, newest first.
  List<UniverseEvent> get today {
    final all = _byKey.values.toList()
      ..sort((a, b) => b.last.compareTo(a.last));
    return all;
  }

  /// The next event to draw, if any.
  UniverseEvent? get next => _waiting.firstOrNull;

  /// The newest the phone holds of what [e] is about: summed into since it
  /// was drawn, it says the total so far.
  UniverseEvent latest(UniverseEvent e) => _byKey[keyOf(e)] ?? e;

  /// [next] was drawn.
  void drew({DateTime? now}) {
    if (_waiting.isEmpty) return;
    _drawnAt[keyOf(_waiting.removeAt(0))] = now ?? clock.now();
  }

  /// Takes what the node answered, oldest first.
  void take(List<UniverseEvent> fresh, {DateTime? now}) {
    final at = now ?? clock.now();
    final cutoff = at.subtract(const Duration(hours: 24));
    final soon = at.subtract(recent);
    for (final e in fresh) {
      if (since == null || e.seq > since!) since = e.seq;
      final key = keyOf(e);
      _byKey[key] = e;
      final last = _time(e.last);
      final drawnAt = _drawnAt[key];
      if (drawnAt != null && at.difference(drawnAt) < growing) grown = e;
      if (last.isBefore(soon) || !_drawn.add(key)) continue;
      _waiting.add(e);
    }
    _byKey.removeWhere((_, e) => _time(e.last).isBefore(cutoff));
    while (_waiting.length > maxWaiting) {
      _waiting.removeAt(0);
    }
  }

  void clear() {
    _byKey.clear();
    _drawn.clear();
    _drawnAt.clear();
    grown = null;
    _waiting.clear();
    since = null;
  }

  /// What an event is about and when that began, as the node sums them.
  static String keyOf(UniverseEvent e) => [
    e.kind.name,
    e.first,
    e.words,
    e.from,
    e.author,
    if (e.kind != EventKind.reshared) e.post,
    if (e.kind == EventKind.machine) e.met,
  ].join('|');
}

DateTime _time(BigInt secs) =>
    DateTime.fromMillisecondsSinceEpoch(secs.toInt() * 1000);

/// When [e] last happened.
DateTime eventTime(UniverseEvent e) => _time(e.last);

/// Names people by their identity, from what the map holds; the owner is
/// "you".
class EventNames {
  EventNames(Iterable<UniversePerson> people) {
    for (final p in people) {
      if (p.step == 0) owner = p.identity;
      if (p.name.isNotEmpty) _names[p.identity] = p.name;
    }
  }

  final _names = <String, String>{};
  String? owner;

  /// `Jimmy`, `You`, or `Someone`.
  String who(String? identity) {
    if (identity != null && identity == owner) return 'You';
    return _names[identity] ?? 'Someone';
  }

  /// `Jimmy's`, `your`, `someone's`, or `a` with no author.
  String whose(String? identity) {
    if (identity == null) return 'a';
    if (identity == owner) return 'your';
    final name = _names[identity];
    return name == null ? "someone's" : "$name's";
  }
}

/// [e] in plain words (ADR 0031 §2), as one line.
String describeEvent(UniverseEvent e, EventNames names) {
  final n = e.count;
  final size = formatBytes(e.bytes.toInt());
  switch (e.kind) {
    case EventKind.found:
      final people = e.people == 1 ? '1 new person' : '${e.people} new people';
      return 'Your node found $people, listed by ${e.words ?? 'a machine'}';
    case EventKind.reshared:
      final whose = names.whose(e.author);
      final who = names.who(e.from);
      return n == 1
          ? '$who reshared $whose post'
          : '$who reshared $whose posts $n times';
    case EventKind.fetched:
      final from = e.fromAuthor
          ? switch (names.whose(e.author)) {
              'your' => 'your own node',
              'a' || "someone's" => "its author's node",
              final whose => '$whose node',
            }
          : e.host == null
          ? 'another node'
          : '${names.who(e.host)}, who keeps it available';
      final kept = e.keptByCopy
          ? ". Kept available by a copy while its author's node was away"
          : '';
      return 'Your node fetched $size of ${names.whose(e.author)} post '
          'from $from$kept';
    case EventKind.served:
      final whose = names.whose(e.author);
      return n == 1
          ? 'Your node sent $size of $whose post to another node'
          : 'Your node sent $whose post to other nodes $n times, $size';
    case EventKind.machine:
      final words = e.words ?? 'a machine';
      return e.met
          ? 'Your node met the machine $words'
          : 'The machine $words stopped answering';
  }
}
