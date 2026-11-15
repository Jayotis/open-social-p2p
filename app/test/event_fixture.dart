import 'package:osp_app/src/rust/api/osp.dart';

/// An event of [kind], as the node sends it, last happening [ago] before
/// now and first [lasted] before that.
UniverseEvent anEvent(
  int seq,
  EventKind kind, {
  Duration ago = Duration.zero,
  Duration lasted = Duration.zero,
  int count = 1,
  String? words,
  int people = 0,
  String? from,
  String? author,
  String? post,
  int bytes = 0,
  String? host,
  bool keptByCopy = false,
  bool met = false,
  DateTime? now,
}) {
  final last = (now ?? DateTime.now()).subtract(ago);
  final first = last.subtract(lasted);
  BigInt secs(DateTime t) => BigInt.from(t.millisecondsSinceEpoch ~/ 1000);
  return UniverseEvent(
    seq: BigInt.from(seq),
    first: secs(first),
    last: secs(last),
    count: count,
    kind: kind,
    words: words,
    people: people,
    from: from,
    author: author,
    post: post,
    bytes: BigInt.from(bytes),
    host: host,
    keptByCopy: keptByCopy,
    met: met,
  );
}
