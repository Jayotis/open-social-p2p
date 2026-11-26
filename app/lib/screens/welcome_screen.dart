import 'package:flutter/material.dart';
import 'package:url_launcher/url_launcher.dart';

import '../node.dart';
import '../osp_codes.dart';
import '../terms.dart';
import '../theme.dart';

/// Before pairing (P11, ADR 0042 §3): what the app needs, a node, then the
/// terms of use. A phone already paired that hasn't agreed to the current
/// terms sees only the terms.
class WelcomeScreen extends StatefulWidget {
  const WelcomeScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<WelcomeScreen> createState() => _WelcomeScreenState();
}

class _WelcomeScreenState extends State<WelcomeScreen> {
  late bool _showTerms = widget.node.paired != null;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(Space.l),
          child: _showTerms ? _terms(context) : _welcome(context),
        ),
      ),
    );
  }

  Widget _welcome(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          child: SingleChildScrollView(
            child: Column(
              children: [
                const SizedBox(height: Space.xl),
                Icon(
                  Icons.home_work_outlined,
                  size: 64,
                  color: theme.colorScheme.primary,
                ),
                const SizedBox(height: Space.m),
                Text(
                  'Welcome to OSP',
                  style: theme.textTheme.headlineSmall,
                  textAlign: TextAlign.center,
                ),
                const SizedBox(height: Space.m),
                const Text(
                  'This app is the companion of your node: a program on a '
                  'computer at home that stays on. Your node holds your '
                  'identity, finds people, and keeps your posts available '
                  "while your phone is off. The app doesn't work without one.",
                  textAlign: TextAlign.center,
                ),
                const SizedBox(height: Space.m),
                const Text(
                  'For now the node runs on Linux, on a 64-bit PC or a small '
                  'computer such as a Raspberry Pi.',
                  textAlign: TextAlign.center,
                ),
                const SizedBox(height: Space.l),
                OutlinedButton.icon(
                  onPressed: () => launchUrl(
                    Uri.parse(installGuide),
                    mode: LaunchMode.externalApplication,
                  ),
                  icon: const Icon(Icons.open_in_new),
                  label: const Text('How to set up a node'),
                ),
              ],
            ),
          ),
        ),
        FilledButton(
          onPressed: () => setState(() => _showTerms = true),
          child: const Text('I have a node'),
        ),
      ],
    );
  }

  Widget _terms(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          child: ListView(
            children: [
              const SizedBox(height: Space.l),
              Text('Terms of use', style: theme.textTheme.headlineSmall),
              const SizedBox(height: Space.m),
              for (final term in termsOfUse)
                Padding(
                  padding: const EdgeInsets.only(bottom: Space.m),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      const Text('•  '),
                      Expanded(child: Text(term)),
                    ],
                  ),
                ),
            ],
          ),
        ),
        // Always in view, so the full terms can be read before agreeing.
        TextButton.icon(
          onPressed: () => launchUrl(
            Uri.parse(fullTermsPage),
            mode: LaunchMode.externalApplication,
          ),
          icon: const Icon(Icons.open_in_new),
          label: const Text('Read the full Terms of Use'),
        ),
        const SizedBox(height: Space.s),
        FilledButton(
          onPressed: widget.node.agreeToTerms,
          child: const Text('I agree'),
        ),
      ],
    );
  }
}
