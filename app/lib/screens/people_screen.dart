import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import 'scan_screen.dart';

/// Following works by exchanging follow codes (ADR 0003): show yours, or
/// scan someone else's.
class PeopleScreen extends StatefulWidget {
  const PeopleScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<PeopleScreen> createState() => _PeopleScreenState();
}

class _PeopleScreenState extends State<PeopleScreen> {
  late final Future<String> _invite = widget.node.app.invite();
  bool _busy = false;

  Future<void> _follow(String? ticket) async {
    if (ticket == null || ticket.isEmpty) return;
    final messenger = ScaffoldMessenger.of(context);
    if (!isFollowTicket(text: ticket)) {
      messenger.showSnackBar(
        const SnackBar(
          content: Text("That isn't a follow code. It starts with OSP1."),
        ),
      );
      return;
    }
    setState(() => _busy = true);
    try {
      await widget.node.app.follow(ticket: ticket.trim());
      messenger.showSnackBar(
        const SnackBar(
          content: Text('Following. Their posts appear in your feed shortly.'),
        ),
      );
      widget.node.refresh(sync: true);
    } catch (e) {
      messenger.showSnackBar(
        SnackBar(content: Text("Couldn't follow: ${describeError(e)}")),
      );
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _scan() async {
    final ticket = await Navigator.of(context).push<String>(
      MaterialPageRoute(
        builder: (_) => ScanScreen(
          title: 'Scan a follow code',
          hint: "Point the camera at someone's follow code.",
          accept: (text) => isFollowTicket(text: text),
          wrongCode: "That's not a follow code.",
        ),
      ),
    );
    await _follow(ticket);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(title: const Text('Follow and be followed')),
      body: ListView(
        padding: const EdgeInsets.all(24),
        children: [
          Text('Your follow code', style: theme.textTheme.titleMedium),
          const SizedBox(height: 8),
          const Text('Anyone who scans this can follow your public posts.'),
          const SizedBox(height: 16),
          FutureBuilder<String>(
            future: _invite,
            builder: (context, ticket) {
              if (ticket.hasError) return Text(describeError(ticket.error!));
              if (!ticket.hasData) {
                return const Center(child: CircularProgressIndicator());
              }
              return Column(
                children: [
                  Center(
                    child: QrImageView(
                      data: ticket.data!,
                      size: 260,
                      backgroundColor: Colors.white,
                    ),
                  ),
                  TextButton.icon(
                    onPressed: () =>
                        Clipboard.setData(ClipboardData(text: ticket.data!)),
                    icon: const Icon(Icons.copy),
                    label: const Text('Copy as text'),
                  ),
                ],
              );
            },
          ),
          const Divider(height: 48),
          Text('Follow someone', style: theme.textTheme.titleMedium),
          const SizedBox(height: 8),
          const Text('Scan their follow code, or paste it.'),
          const SizedBox(height: 16),
          FilledButton.icon(
            onPressed: _busy ? null : _scan,
            icon: const Icon(Icons.qr_code_scanner),
            label: const Text('Scan follow code'),
          ),
          const SizedBox(height: 8),
          OutlinedButton(
            onPressed: _busy
                ? null
                : () async =>
                      _follow(await askForCode(context, 'Paste a follow code')),
            child: const Text('Paste a follow code'),
          ),
          if (_busy)
            const Padding(
              padding: EdgeInsets.only(top: 16),
              child: LinearProgressIndicator(),
            ),
        ],
      ),
    );
  }
}
