import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../theme.dart';

/// Whether to use the project's hide list (ADR 0042 §6): asked on first
/// start after the terms, and on a paired phone whose node hasn't been
/// asked. "Use the list" is the highlighted answer.
class HideListScreen extends StatefulWidget {
  const HideListScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<HideListScreen> createState() => _HideListScreenState();
}

class _HideListScreenState extends State<HideListScreen> {
  bool _busy = false;
  String? _error;

  Future<void> _answer(bool use) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.node.answerHideList(use);
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(Space.l),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(
                child: ListView(
                  children: [
                    const SizedBox(height: Space.xl),
                    Icon(
                      Icons.visibility_off_outlined,
                      size: 64,
                      color: theme.colorScheme.primary,
                    ),
                    const SizedBox(height: Space.m),
                    Text(
                      'Hide what breaks the rules?',
                      style: theme.textTheme.headlineSmall,
                      textAlign: TextAlign.center,
                    ),
                    const SizedBox(height: Space.m),
                    const Text(
                      'The project keeps a short list of posts and files '
                      'that break the terms, such as explicit sexual '
                      "material. If you use it, you don't see them, and your "
                      "node doesn't download them or pass them on.",
                    ),
                    const SizedBox(height: Space.m),
                    const _Point(
                      'Your node gets the list once a day without showing '
                      "its address, so the project doesn't learn who asked. "
                      "It's signed, so nobody else can change it.",
                    ),
                    const _Point(
                      'Your own posts are never hidden. You can show a '
                      "person's posts anyway from their profile, and turn "
                      'the list off any time under You, Advanced details.',
                    ),
                    if (_error != null)
                      Padding(
                        padding: const EdgeInsets.only(top: Space.m),
                        child: Text(
                          _error!,
                          style: TextStyle(color: theme.colorScheme.error),
                        ),
                      ),
                  ],
                ),
              ),
              FilledButton(
                onPressed: _busy ? null : () => _answer(true),
                child: const Text('Use the list'),
              ),
              const SizedBox(height: Space.s),
              TextButton(
                onPressed: _busy ? null : () => _answer(false),
                child: const Text('Not now'),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Point extends StatelessWidget {
  const _Point(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.m),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text('•  '),
          Expanded(child: Text(text)),
        ],
      ),
    );
  }
}
