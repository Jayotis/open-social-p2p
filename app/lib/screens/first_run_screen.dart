import 'package:flutter/material.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../format.dart';
import '../node.dart';
import '../theme.dart';

/// The first run after pairing (ADR 0031 §3): this is you, your home
/// computer is your node, then the Universe.
class FirstRunScreen extends StatefulWidget {
  const FirstRunScreen({super.key, required this.node, required this.onDone});

  final HomeNode node;

  /// Called when the person is through: the shell opens the Universe.
  final VoidCallback onDone;

  @override
  State<FirstRunScreen> createState() => _FirstRunScreenState();
}

class _FirstRunScreenState extends State<FirstRunScreen> {
  late final Future<String> _invite = widget.node.app.invite();
  int _page = 0;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final name = widget.node.paired?.name ?? '';
    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(Space.l),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(
                child: SingleChildScrollView(
                  child: _page == 0
                      ? Column(
                          children: [
                            const SizedBox(height: Space.xl),
                            Text(
                              'This is you',
                              style: theme.textTheme.headlineSmall,
                            ),
                            const SizedBox(height: Space.s),
                            Text(
                              name.isEmpty ? '(no name yet)' : name,
                              style: theme.textTheme.titleLarge,
                            ),
                            const SizedBox(height: Space.l),
                            FutureBuilder<String>(
                              future: _invite,
                              builder: (context, code) {
                                if (code.hasError) {
                                  return Text(describeError(code.error!));
                                }
                                if (!code.hasData) {
                                  return const CircularProgressIndicator();
                                }
                                return QrImageView(
                                  data: code.data!,
                                  size: 220,
                                  backgroundColor: Colors.white,
                                );
                              },
                            ),
                            const SizedBox(height: Space.m),
                            const Text(
                              'Your invite code. Anyone who scans it can '
                              'follow what you post publicly. You can find it '
                              'again under You.',
                              textAlign: TextAlign.center,
                            ),
                          ],
                        )
                      : Column(
                          children: [
                            const SizedBox(height: Space.xl),
                            const Icon(Icons.computer, size: 64),
                            const SizedBox(height: Space.m),
                            Text(
                              'Your home computer is your node',
                              style: theme.textTheme.headlineSmall,
                              textAlign: TextAlign.center,
                            ),
                            const SizedBox(height: Space.m),
                            const Text(
                              'It keeps working while your phone is off. It '
                              'finds people, gathers the posts of those you '
                              'follow, and keeps yours available to others. '
                              'This app is how you use it.',
                              textAlign: TextAlign.center,
                            ),
                          ],
                        ),
                ),
              ),
              FilledButton(
                onPressed: _page == 0
                    ? () => setState(() => _page = 1)
                    : widget.onDone,
                child: Text(_page == 0 ? 'Next' : 'Open the Universe'),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
