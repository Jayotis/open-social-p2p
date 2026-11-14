/// The words the normal journey doesn't use (ADR 0031 §2). They belong on
/// the Advanced details screen only.
library;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

final protocolWords = RegExp(
  r'\b(tor|iroh|endpoints?|onions?|relays?|rtt|actors?|blobs?|director(y|ies)|tickets?)\b',
  caseSensitive: false,
);

/// Fails if any text or tooltip on screen now uses a protocol word.
void expectPlainWords(WidgetTester tester) {
  final shown = <String>[
    for (final w in tester.widgetList<RichText>(find.byType(RichText)))
      w.text.toPlainText(),
    for (final w in tester.widgetList<Tooltip>(find.byType(Tooltip)))
      ?w.message,
  ];
  final found = [
    for (final text in shown)
      if (protocolWords.hasMatch(text)) text,
  ];
  expect(found, isEmpty, reason: 'protocol words on an ordinary screen');
}
