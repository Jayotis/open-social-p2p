import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/activity_notifications.dart';
import 'package:osp_app/src/rust/api/osp.dart';

ActivityItem entry(
  int seq,
  String kind, {
  bool read = false,
  String name = 'Jimmy',
  String text = '',
  int count = 1,
}) => ActivityItem(
  key: '$kind:$seq',
  seq: seq,
  atMs: seq * 1000,
  read: read,
  kind: kind,
  postText: 'A walk by the river',
  people: [ActivityName(identity: 'id-$name', name: name)],
  count: count,
  text: text,
  waiting: false,
);

void main() {
  test('one new entry is told in its own words, with what they said', () {
    final notice = noticeFor([entry(3, 'reply', text: 'Lovely light')], 1, 0)!;
    expect(notice.title, 'Jimmy replied to your post “A walk by the river”');
    expect(notice.body, 'Lovely light');
    expect(notice.lines, isEmpty);
  });

  test('several new entries are counted and listed, newest first', () {
    final notice = noticeFor(
      [
        entry(5, 'liked', name: 'Alice', count: 2),
        entry(4, 'followed'),
        entry(2, 'reply', read: true),
      ],
      2,
      0,
    )!;
    expect(notice.title, '2 new in Activity');
    expect(notice.lines, [
      'Alice and 1 other liked your post “A walk by the river”',
      'Jimmy follows you',
    ]);
    expect(notice.body, notice.lines.first);
  });

  test('what was told already, or read, isn\'t told again', () {
    final entries = [entry(4, 'liked'), entry(3, 'reply', read: true)];
    expect(noticeFor(entries, 1, 4), isNull);
    expect(noticeFor([entry(3, 'reply', read: true)], 0, 0), isNull);
    expect(highestSeq(entries, 2), 4);
    expect(highestSeq(entries, 9), 9);
  });

  test('a change to an entry told before is told again', () {
    // A second like takes a new seq on the node.
    final notice = noticeFor([entry(6, 'liked', count: 2)], 1, 4)!;
    expect(notice.title, contains('liked your post'));
  });

  test('one new among others still unread says how many wait', () {
    final notice = noticeFor([entry(7, 'liked'), entry(4, 'followed')], 2, 4)!;
    expect(notice.title, '2 new in Activity');
    expect(notice.lines, ['Jimmy liked your post “A walk by the river”']);
  });
}
