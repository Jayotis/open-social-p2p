import 'package:flutter_rust_bridge/flutter_rust_bridge.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/format.dart';

void main() {
  test('bytes', () {
    expect(formatBytes(0), '0 B');
    expect(formatBytes(1536), '2 KB');
    expect(formatBytes(52300000), '49.9 MB');
    expect(formatBytes(5 << 30), '5.0 GB');
  });

  test('durations', () {
    expect(formatDuration(42000), '0:42');
    expect(formatDuration(725000), '12:05');
    expect(formatDuration(3723000), '1:02:03');
  });

  test('ago', () {
    final now = DateTime.utc(2026, 10, 21, 12);
    expect(
      formatAgo(now.subtract(const Duration(seconds: 20)), now: now),
      'just now',
    );
    expect(
      formatAgo(now.subtract(const Duration(minutes: 5)), now: now),
      '5 min ago',
    );
    expect(
      formatAgo(now.subtract(const Duration(hours: 3)), now: now),
      '3 h ago',
    );
    expect(
      formatAgo(now.subtract(const Duration(days: 2)), now: now),
      '2 d ago',
    );
  });

  test('errors from Rust become one line', () {
    final e = AnyhowException(
      "can't reach the home node\n\nCaused by:\n    0: connecting\n    1: timed out\n\nStack backtrace:\n   0: foo",
    );
    expect(
      describeError(e),
      "can't reach the home node: connecting: timed out",
    );
    expect(
      describeError(
        AnyhowException('this device isn\'t paired with this node'),
      ),
      "this device isn't paired with this node",
    );
  });
}
