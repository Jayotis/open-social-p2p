import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../theme.dart';

/// The question on the node's own address (ADR 0043 §8), in the words
/// setup asks it with.
const hopsQuestion =
    'Your node can fetch and send video through other nodes it picks, which '
    'is much faster than the public fallback. The node next to yours sees '
    "your internet address and which node you're fetching from, as the "
    "fallback does today, but never what's inside.";

/// What each answer does.
const hopsAnswers =
    'With no, or until you answer, your own video goes through the public '
    'fallback, as today. You can change it any time under Your node, '
    'Carrying.';

/// Asks whether the hops the node picks may see its address: on a paired
/// phone whose node hasn't been asked, as one set up before the question.
/// Yes is the highlighted answer, as setup recommends it.
class HopsScreen extends StatefulWidget {
  const HopsScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<HopsScreen> createState() => _HopsScreenState();
}

class _HopsScreenState extends State<HopsScreen> {
  bool _busy = false;
  String? _error;

  Future<void> _answer(bool yes) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.node.answerHops(yes);
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
                      Icons.route_outlined,
                      size: 64,
                      color: theme.colorScheme.primary,
                    ),
                    const SizedBox(height: Space.m),
                    Text(
                      'Faster video through other nodes?',
                      style: theme.textTheme.headlineSmall,
                      textAlign: TextAlign.center,
                    ),
                    const SizedBox(height: Space.m),
                    const Text(hopsQuestion),
                    const SizedBox(height: Space.m),
                    const Text(hopsAnswers),
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
                child: const Text('Yes, go through them'),
              ),
              const SizedBox(height: Space.s),
              TextButton(
                onPressed: _busy ? null : () => _answer(false),
                child: const Text('No, use the fallback'),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
