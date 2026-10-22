import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import 'scan_screen.dart';

/// First run: pair this phone with its home node (ADR 0008).
class PairScreen extends StatefulWidget {
  const PairScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<PairScreen> createState() => _PairScreenState();
}

class _PairScreenState extends State<PairScreen> {
  bool _busy = false;
  String? _error;

  Future<void> _pair(String? ticket) async {
    if (ticket == null || ticket.isEmpty) return;
    if (!isPairingTicket(text: ticket)) {
      setState(
        () => _error =
            "That isn't a pairing code. It should start with OSPPAIR1.",
      );
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.node.pair(ticket);
      // The app switches to the feed by itself once paired.
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _scan() async {
    final ticket = await Navigator.of(context).push<String>(
      MaterialPageRoute(
        builder: (_) => ScanScreen(
          title: 'Scan pairing code',
          hint: 'Point the camera at the QR code from `osp pair`.',
          accept: (text) => isPairingTicket(text: text),
          wrongCode: "That's not a pairing code.",
        ),
      ),
    );
    await _pair(ticket);
  }

  Future<void> _paste() async {
    await _pair(await askForCode(context, 'Paste the pairing code'));
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(24),
          children: [
            const SizedBox(height: 24),
            Icon(
              Icons.home_work_outlined,
              size: 64,
              color: theme.colorScheme.primary,
            ),
            const SizedBox(height: 16),
            Text(
              'Pair with your home node',
              style: theme.textTheme.headlineSmall,
              textAlign: TextAlign.center,
            ),
            const SizedBox(height: 16),
            const Text(
              'This app is a remote control for your home node: the computer '
              'that holds your identity and does the networking. Your phone '
              'talks only to it.',
            ),
            const SizedBox(height: 16),
            const Text('On that computer, run:'),
            const SizedBox(height: 8),
            Container(
              padding: const EdgeInsets.all(12),
              color: theme.colorScheme.surfaceContainerHighest,
              child: const SelectableText(
                'osp pair',
                style: TextStyle(fontFamily: 'monospace', fontSize: 16),
              ),
            ),
            const SizedBox(height: 8),
            const Text(
              'Then scan the QR code it shows. The code works once, for 10 minutes.',
            ),
            const SizedBox(height: 24),
            FilledButton.icon(
              onPressed: _busy ? null : _scan,
              icon: const Icon(Icons.qr_code_scanner),
              label: const Text('Scan QR code'),
            ),
            const SizedBox(height: 8),
            OutlinedButton(
              onPressed: _busy ? null : _paste,
              child: const Text('Paste the code instead'),
            ),
            const SizedBox(height: 24),
            if (_busy) ...[
              const LinearProgressIndicator(),
              const SizedBox(height: 8),
              const Text(
                'Connecting to your home node…',
                textAlign: TextAlign.center,
              ),
            ],
            if (_error != null)
              Text(_error!, style: TextStyle(color: theme.colorScheme.error)),
          ],
        ),
      ),
    );
  }
}
