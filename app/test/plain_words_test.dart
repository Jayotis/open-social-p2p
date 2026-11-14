import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

import 'plain_words.dart';

/// Files allowed to use protocol words: Advanced details, and the
/// generated Rust bindings.
bool _exempt(String path) =>
    path.endsWith('/advanced_screen.dart') || path.contains('/src/rust/');

/// Every string literal in [source], less comments and interpolations.
/// A line ending in `// not shown` holds a value used in code, such as a
/// pattern or a value compared, and is skipped.
Iterable<String> literals(String source) sync* {
  final literal = RegExp(
    r"'(?:[^'\\\n]|\\.)*'"
    r'|"(?:[^"\\\n]|\\.)*"',
  );
  for (final line in source.split('\n')) {
    final code = line.trimLeft();
    if (code.startsWith('//') ||
        code.startsWith('import ') ||
        code.startsWith('export ') ||
        line.endsWith('// not shown')) {
      continue;
    }
    for (final m in literal.allMatches(line)) {
      yield m
          .group(0)!
          .replaceAll(RegExp(r'\$\{[^}]*\}'), '')
          .replaceAll(RegExp(r'\$\w+'), '');
    }
  }
}

void main() {
  test('ordinary screens name no mechanism (ADR 0031)', () {
    final found = <String>[];
    final files = Directory('lib')
        .listSync(recursive: true)
        .whereType<File>()
        .where((f) => f.path.endsWith('.dart') && !_exempt(f.path));
    for (final file in files) {
      for (final text in literals(file.readAsStringSync())) {
        if (protocolWords.hasMatch(text)) found.add('${file.path}: $text');
      }
    }
    expect(found, isEmpty);
  });

  test('the check finds the words it should', () {
    final caught = literals('''
      Text('Connecting through Tor…'),
      Text("From directory \$name"),
      // A comment about the relay.
      if (path == 'relay') x, // not shown
      Text('Restore your notes'),
    ''').where(protocolWords.hasMatch).toList();
    expect(caught, ["'Connecting through Tor…'", '"From directory "']);
  });
}
