import 'package:flutter/material.dart';

import '../node.dart';
import '../theme.dart';

/// The project's hide list (ADR 0042 §6): nodes use it unless turned off,
/// so this only tells the owner, once, after the terms. The switch is under
/// Advanced details.
class HideListScreen extends StatelessWidget {
  const HideListScreen({super.key, required this.node});

  final HomeNode node;

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
                      'The worst material is hidden',
                      style: theme.textTheme.headlineSmall,
                      textAlign: TextAlign.center,
                    ),
                    const SizedBox(height: Space.m),
                    const Text(
                      'The project keeps a short list of posts and files '
                      'with the worst material, such as sexual material '
                      'involving children, or intimate images shared '
                      "without consent. Your node uses it: you don't see "
                      "them, and your node doesn't download them or pass "
                      'them on.',
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
                  ],
                ),
              ),
              FilledButton(
                onPressed: node.hideListTold,
                child: const Text('Continue'),
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
