/// Small formatting helpers shared by the screens.
library;

import 'package:flutter_rust_bridge/flutter_rust_bridge.dart';

/// Why someone the owner doesn't follow is on the node: they asked to be
/// found, and the machine named (by its name in words) lists them. The
/// Universe's machine list says what such a machine is.
String listedBy(String machine) => 'Asked to be found, listed by $machine';

/// `1.2 MB`, `340 KB`.
String formatBytes(int bytes) {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  var value = bytes.toDouble();
  var unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  final digits = unit >= 2 && value < 100 ? 1 : 0;
  return '${value.toStringAsFixed(digits)} ${units[unit]}';
}

/// `0:42`, `12:05`, `1:02:03`.
String formatDuration(int ms) {
  final total = (ms / 1000).round();
  final h = total ~/ 3600;
  final m = (total % 3600) ~/ 60;
  final s = (total % 60).toString().padLeft(2, '0');
  if (h > 0) return '$h:${m.toString().padLeft(2, '0')}:$s';
  return '$m:$s';
}

/// `just now`, `5 min ago`, `3 h ago`, `2 d ago`, then the date.
String formatAgo(DateTime time, {DateTime? now}) {
  final diff = (now ?? DateTime.now()).difference(time);
  if (diff.inMinutes < 1) return 'just now';
  if (diff.inHours < 1) return '${diff.inMinutes} min ago';
  if (diff.inDays < 1) return '${diff.inHours} h ago';
  if (diff.inDays < 7) return '${diff.inDays} d ago';
  final local = time.toLocal();
  return '${local.year}-${_two(local.month)}-${_two(local.day)}';
}

/// `14:05`.
String formatClock(DateTime time) {
  final local = time.toLocal();
  return '${_two(local.hour)}:${_two(local.minute)}';
}

/// `3 h 12 min`, `2 d 4 h`.
String formatUptime(int secs) {
  if (secs >= 86400) return '${secs ~/ 86400} d ${secs % 86400 ~/ 3600} h';
  if (secs >= 3600) return '${secs ~/ 3600} h ${secs % 3600 ~/ 60} min';
  if (secs >= 60) return '${secs ~/ 60} min';
  return '$secs s';
}

String _two(int n) => n.toString().padLeft(2, '0');

/// One line for an error from Rust: its message and causes, without the
/// "Caused by:" layout or a backtrace.
String describeError(Object error) {
  final text = error is AnyhowException ? error.message : error.toString();
  final parts = <String>[];
  for (final raw in text.split('\n')) {
    final line = raw.trim().replaceFirst(RegExp(r'^\d+: '), '');
    if (line.isEmpty || line == 'Caused by:') continue;
    if (line.startsWith('Stack backtrace') || line.startsWith('Backtrace')) {
      break;
    }
    if (!parts.contains(line)) parts.add(line);
  }
  return parts.isEmpty ? text : parts.join(': ');
}

/// Plain words for a file the node couldn't get because neither [who]'s
/// node nor anyone with a copy answered, as when their computer is off.
/// Null for any other error, which is shown as it is.
String? noCopyWords(Object error, String who) =>
    describeError(error).contains('no provider could send it')
    ? "$who's node isn't answering, and nobody else with a copy did. "
          'It may be switched off: try again later.'
    : null;

/// Plain words for a thread the node couldn't read from [who]'s node,
/// which didn't answer. Null for any other error.
String? threadUnreachableWords(Object error, String who) =>
    describeError(
      error,
    ).contains("couldn't be read without showing who you are")
    ? "$who's node isn't answering, so the replies can't be shown now. "
          'Try again later.'
    : null;

/// [text] without network addresses or long IDs: IPv4 and IPv6 addresses,
/// onion addresses, and runs of 20 or more letters and digits such as
/// endpoint IDs and keys. For text a person may paste somewhere public.
String scrubAddresses(String text) => text
    .replaceAll(RegExp(r'\b[a-z2-7]{16,56}\.onion\b'), '[onion]') // not shown
    .replaceAll(
      RegExp(r'\[?\b(?:[0-9a-fA-F]{0,4}:){2,7}[0-9a-fA-F]{0,4}\b\]?(?::\d+)?'),
      '[ip]',
    )
    .replaceAll(RegExp(r'\b(?:\d{1,3}\.){3}\d{1,3}(?::\d+)?\b'), '[ip]')
    .replaceAll(RegExp(r'\b[A-Za-z0-9]{20,}\b'), '[id]');
