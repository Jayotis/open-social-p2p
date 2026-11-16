/// What an Activity entry says (ADR 0038 §3), in a social app's words, not
/// a log's (ADR 0031 §2).
library;

import 'src/rust/api/osp.dart';

String _name(String name) => name.isEmpty ? 'Someone' : name;

/// "Jimmy", "Jimmy and Alice", "Jimmy, Alice and 3 others".
String names(List<ActivityName> people, int count) {
  final named = [for (final p in people) _name(p.name)];
  final others = count - named.length;
  if (named.isEmpty) return count == 1 ? 'Someone' : '$count people';
  if (others <= 0) {
    return named.length == 1
        ? named.first
        : '${named.sublist(0, named.length - 1).join(', ')} and ${named.last}';
  }
  return '${named.join(', ')} and $others ${others == 1 ? 'other' : 'others'}';
}

/// The post an entry is about: "your post “A walk by the river”".
String yourPost(String text) {
  final line = text.split('\n').first.trim();
  return line.isEmpty ? 'your post' : 'your post “$line”';
}

/// The entry's line.
String activityLine(ActivityItem e) {
  final who = e.people.isEmpty ? 'Someone' : _name(e.people.first.name);
  return switch (e.kind) {
    'reply' when e.waiting =>
      "$who's reply to ${yourPost(e.postText)} is waiting for you",
    'reply' => '$who replied to ${yourPost(e.postText)}',
    'liked' => '${names(e.people, e.count)} liked ${yourPost(e.postText)}',
    'reshared' =>
      '${names(e.people, e.count)} reshared ${yourPost(e.postText)}',
    'followed' when e.people.isEmpty => 'Someone new follows you',
    'followed' => '$who follows you',
    'knows_you' => '$who says they know you',
    'reply_published' when e.people.isEmpty || e.people.first.name.isEmpty =>
      'Your reply is in the thread',
    'reply_published' => "Your reply is in $who's thread",
    _ => 'Something happened',
  };
}

/// What they said, if anything: the reply, or the note with "says they
/// know you".
String? activityQuote(ActivityItem e) {
  final text = e.text.trim();
  return text.isEmpty ? null : text;
}
