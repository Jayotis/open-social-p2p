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

  test("a file nobody could send is said in plain words, others aren't", () {
    // As the node words it when Kim's computer is off.
    final off = AnyhowException(
      'download of 073f9bb43c failed\n\nCaused by:\n    no provider could send it',
    );
    expect(
      noCopyWords(off, 'Kim'),
      "Kim's node isn't answering, and nobody else with a copy did. "
      'It may be switched off: try again later.',
    );
    expect(noCopyWords(AnyhowException('disk full'), 'Kim'), isNull);
    expect(threadUnreachableWords(off, 'Kim'), isNull);
  });

  test('scrubAddresses removes addresses and long IDs, nothing else', () {
    expect(
      scrubAddresses(
        'Last error: no route to 192.168.1.20:4433 or [2a01:4f8::1]:7842\n'
        'Problem: abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuv.onion '
        'is down\n'
        'Endpoint 5f2b9c0e4d7a1b3c8e6f0a2d4b6c8e0f1a3c5e7b9d0f2a4c6e8b0d2f4a6c8e0a\n'
        'Path: relay, 80 ms\nRunning for: 3 h 12 min\nNode version: 0.1.0',
      ),
      'Last error: no route to [ip] or [ip]\n'
      'Problem: [onion] is down\n'
      'Endpoint [id]\n'
      'Path: relay, 80 ms\nRunning for: 3 h 12 min\nNode version: 0.1.0',
    );
  });
}
