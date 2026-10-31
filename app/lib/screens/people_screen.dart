import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import 'scan_screen.dart';
import 'trust_screen.dart';

/// Following works by exchanging follow codes (ADR 0003): show yours, or
/// scan someone else's. People you follow can be trusted, or asked to
/// trust you (ADR 0013).
class PeopleScreen extends StatefulWidget {
  const PeopleScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<PeopleScreen> createState() => _PeopleScreenState();
}

class _PeopleScreenState extends State<PeopleScreen> {
  late final Future<String> _invite = widget.node.app.invite();
  late Future<List<Followed>> _following = widget.node.app.following();
  bool _busy = false;

  /// Runs [action], then says [done], or why it failed.
  Future<void> _do(Future<void> Function() action, String done) async {
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _busy = true);
    try {
      await action();
      messenger.showSnackBar(SnackBar(content: Text(done)));
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(describeError(e))));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _trust(Followed person) async {
    if (!await confirmTrust(context, person.name)) return;
    await _do(
      () => widget.node.app.trust(identity: person.identity),
      'You trust ${person.name} now.',
    );
  }

  Future<void> _askTrust(Followed person) async {
    final note = await showDialog<String>(
      context: context,
      builder: (context) => _AskTrustDialog(name: person.name),
    );
    if (note == null) return;
    await _do(
      () => widget.node.app.askTrust(identity: person.identity, note: note),
      "Asked. You won't be told their answer.",
    );
  }

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
      await widget.node.refresh(sync: true);
      if (mounted) setState(() => _following = widget.node.app.following());
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
          const Divider(height: 48),
          Text('People you follow', style: theme.textTheme.titleMedium),
          const SizedBox(height: 8),
          const Text(
            'Someone you know can be a trusted follower: trust them, or ask '
            'them to trust you.',
          ),
          FutureBuilder<List<Followed>>(
            future: _following,
            builder: (context, following) {
              if (following.hasError) {
                return Text(describeError(following.error!));
              }
              final people = following.data;
              if (people == null) {
                return const Center(child: CircularProgressIndicator());
              }
              if (people.isEmpty) {
                return const Padding(
                  padding: EdgeInsets.only(top: 8),
                  child: Text("You don't follow anyone yet."),
                );
              }
              return Column(
                children: [
                  for (final person in people)
                    ListTile(
                      contentPadding: EdgeInsets.zero,
                      title: Text(person.name),
                      trailing: PopupMenuButton<String>(
                        enabled: !_busy,
                        onSelected: (choice) => choice == 'trust'
                            ? _trust(person)
                            : _askTrust(person),
                        itemBuilder: (_) => const [
                          PopupMenuItem(value: 'trust', child: Text('Trust')),
                          PopupMenuItem(
                            value: 'ask',
                            child: Text('Ask to be a trusted follower'),
                          ),
                        ],
                      ),
                    ),
                ],
              );
            },
          ),
        ],
      ),
    );
  }
}

/// Asks for the note sent with a request to be trusted. Pops with the
/// note, or null if cancelled.
class _AskTrustDialog extends StatefulWidget {
  const _AskTrustDialog({required this.name});

  final String name;

  @override
  State<_AskTrustDialog> createState() => _AskTrustDialogState();
}

class _AskTrustDialogState extends State<_AskTrustDialog> {
  final _note = TextEditingController();

  @override
  void dispose() {
    _note.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text('Ask ${widget.name} to trust you?'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            "They see your name and this note, and decide. You aren't told "
            'their answer.',
          ),
          const SizedBox(height: 12),
          TextField(
            controller: _note,
            maxLength: 200,
            maxLines: 3,
            minLines: 1,
            decoration: const InputDecoration(
              labelText: 'Who you are',
              hintText: 'Sam, from the climbing club',
            ),
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context, _note.text.trim()),
          child: const Text('Ask'),
        ),
      ],
    );
  }
}
