import 'dart:convert';
import 'dart:io';

import 'package:clock/clock.dart';

/// Deletes waiting to be sent, and when each is due, kept in a file so one
/// isn't lost when the phone stops the app before it went: an iPhone may
/// suspend it while the request is on its way (ADR 0019).
///
/// Written at once and in full on every change: there are only ever a few,
/// and the app may be stopped right after.
class WaitingDeletes {
  WaitingDeletes(this.file);

  final File file;

  /// One the node still refuses after this long is given up.
  static const giveUpAfter = Duration(days: 1);

  /// What it deletes, and when it's due.
  Map<String, DateTime> read() {
    try {
      final json = jsonDecode(file.readAsStringSync()) as Map<String, dynamic>;
      final oldest = clock.now().subtract(giveUpAfter);
      return {
        for (final MapEntry(:key, :value) in json.entries)
          if (DateTime.tryParse(value as String) case final due?
              when due.isAfter(oldest))
            key: due,
      };
    } catch (_) {
      // None yet, or a file that can't be read: nothing to send.
      return {};
    }
  }

  void add(String id, DateTime due) => _write(read()..[id] = due);

  void remove(String id) {
    final waiting = read();
    if (waiting.remove(id) != null) _write(waiting);
  }

  void _write(Map<String, DateTime> waiting) {
    try {
      final json = jsonEncode({
        for (final MapEntry(:key, :value) in waiting.entries)
          key: value.toUtc().toIso8601String(),
      });
      // Whole or not at all, even if the app is stopped halfway.
      final tmp = File('${file.path}.tmp');
      tmp.writeAsStringSync(json, flush: true);
      tmp.renameSync(file.path);
    } catch (_) {
      // The delete still goes while the app runs; it just isn't kept.
    }
  }
}
