import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';

import '../format.dart';
import '../node.dart';
import '../osp_codes.dart';
import '../src/rust/api/osp.dart';
import 'scan_screen.dart';
import 'trust_screen.dart';
import '../widgets/sky_bar.dart';

/// Following works by exchanging invite codes (follow codes in ADR 0003):
/// show yours, or scan someone else's. An OSP code (ADR 0039 §4) works
/// wherever an invite code does, and one copied is offered when the screen
/// opens. People you follow can be marked as people you know, or asked to
/// say they know you (ADR 0013).
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

  /// An OSP code found on the clipboard as the screen opened.
  SharedCode? _copied;

  @override
  void initState() {
    super.initState();
    _checkClipboard();
  }

  /// Reads the clipboard once, here where codes are expected: Android says
  /// so each time an app reads it, so the app reads it nowhere else.
  Future<void> _checkClipboard() async {
    if (!await Clipboard.hasStrings()) return;
    final text = (await Clipboard.getData(Clipboard.kTextPlain))?.text;
    if (text == null || !mounted) return;
    final code = findOspCode(text: text);
    if (code == null || code.actor == widget.node.paired?.identity) return;
    setState(() => _copied = code);
  }

  Future<void> _open(SharedCode code) async {
    setState(() {
      _copied = null;
      _busy = true;
    });
    try {
      await openOspCode(context, widget.node, code);
    } finally {
      if (mounted) {
        setState(() {
          _busy = false;
          _following = widget.node.app.following();
        });
      }
    }
  }

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
      'You know ${person.name}.',
    );
  }

  Future<void> _unfollow(Followed person) async {
    await _do(
      () => widget.node.app.unfollow(identity: person.identity),
      'You no longer follow ${person.name}.',
    );
    if (mounted) {
      setState(() {
        _following = widget.node.app.following();
      });
    }
  }

  Future<void> _askTrust(Followed person) async {
    final note = await showDialog<String>(
      context: context,
      builder: (context) => AskTrustDialog(name: person.name),
    );
    if (note == null) return;
    await _do(
      () => widget.node.app.askTrust(identity: person.identity, note: note),
      "Asked. You won't be told their answer.",
    );
  }

  Future<void> _follow(String? ticket) async {
    if (ticket == null || ticket.trim().isEmpty) return;
    final messenger = ScaffoldMessenger.of(context);
    if (!isFollowTicket(text: ticket)) {
      // A whole shared message may be pasted, words and all.
      final SharedCode code;
      try {
        code = findOspCode(text: ticket) ?? readOspCode(text: codePart(ticket));
      } catch (e) {
        messenger.showSnackBar(
          SnackBar(
            content: Text(
              ticket.toLowerCase().contains('osp:')
                  ? describeError(e)
                  : "That isn't an invite code or an OSP code. Invite codes "
                        'start with OSP1, OSP codes with osp:.',
            ),
          ),
        );
        return;
      }
      await _open(code);
      return;
    }
    setState(() => _busy = true);
    try {
      await widget.node.app.follow(ticket: ticket.trim());
      final overTor = widget.node.status?.overTor ?? false;
      messenger.showSnackBar(
        SnackBar(
          content: Text(
            overTor
                ? 'Following. Your node is reaching theirs, which can take a '
                      'few minutes. Their posts appear on Home after that.'
                : 'Following. Their posts appear on Home shortly.',
          ),
        ),
      );
      await widget.node.refresh(sync: true);
      if (mounted) {
        setState(() {
          _following = widget.node.app.following();
        });
      }
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
          title: 'Scan an invite code',
          hint: "Point the camera at someone's invite code.",
          accept: (text) =>
              isFollowTicket(text: text) || findOspCode(text: text) != null,
          wrongCode: "That's not an invite code or an OSP code.",
        ),
      ),
    );
    await _follow(ticket);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      appBar: SkyBar(AppBar(title: const Text('Invite and follow'))),
      body: ListView(
        padding: const EdgeInsets.all(24),
        children: [
          if (_copied case final copied?) ...[
            Card(
              margin: EdgeInsets.zero,
              child: ListTile(
                leading: const Icon(Icons.content_paste),
                title: Text(
                  copied.post == null
                      ? 'You copied an OSP code for someone.'
                      : 'You copied an OSP code for a post.',
                ),
                subtitle: Row(
                  mainAxisAlignment: MainAxisAlignment.end,
                  children: [
                    TextButton(
                      onPressed: () => setState(() => _copied = null),
                      child: const Text('Not now'),
                    ),
                    FilledButton(
                      onPressed: _busy ? null : () => _open(copied),
                      child: const Text('Open it'),
                    ),
                  ],
                ),
              ),
            ),
            const SizedBox(height: 24),
          ],
          Text('Your invite code', style: theme.textTheme.titleMedium),
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
                  Wrap(
                    alignment: WrapAlignment.center,
                    children: [
                      TextButton.icon(
                        onPressed: () => Clipboard.setData(
                          ClipboardData(text: ticket.data!),
                        ),
                        icon: const Icon(Icons.copy),
                        label: const Text('Copy as text'),
                      ),
                      TextButton.icon(
                        onPressed: () => shareProfile(
                          context,
                          widget.node,
                          identity: widget.node.paired?.identity ?? '',
                          name: '',
                        ),
                        icon: const Icon(Icons.share),
                        label: const Text('Share'),
                      ),
                    ],
                  ),
                ],
              );
            },
          ),
          const Divider(height: 48),
          Text('Follow someone', style: theme.textTheme.titleMedium),
          const SizedBox(height: 8),
          const Text(
            'Scan their invite code, or paste it. An OSP code someone shared '
            'works here too.',
          ),
          const SizedBox(height: 16),
          FilledButton.icon(
            onPressed: _busy ? null : _scan,
            icon: const Icon(Icons.qr_code_scanner),
            label: const Text('Scan invite code'),
          ),
          const SizedBox(height: 8),
          OutlinedButton(
            onPressed: _busy
                ? null
                : () async => _follow(
                    await askForCode(context, 'Paste an invite or OSP code'),
                  ),
            child: const Text('Paste a code'),
          ),
          const SizedBox(height: 8),
          TextButton.icon(
            onPressed: () => shareText(aboutOspText()),
            icon: const Icon(Icons.campaign_outlined),
            label: const Text('Tell someone about OSP'),
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
            'Say which of them you know, or ask them to say they know you.',
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
                      subtitle: person.connecting
                          ? Text(
                              widget.node.status?.overTor ?? false
                                  ? 'Connecting, which can take a few minutes…'
                                  : 'Connecting…',
                            )
                          : null,
                      trailing: PopupMenuButton<String>(
                        enabled: !_busy,
                        onSelected: (choice) => switch (choice) {
                          'trust' => _trust(person),
                          'ask' => _askTrust(person),
                          _ => _unfollow(person),
                        },
                        itemBuilder: (_) => const [
                          PopupMenuItem(
                            value: 'trust',
                            child: Text('I know them'),
                          ),
                          PopupMenuItem(
                            value: 'ask',
                            child: Text('Ask if they know you'),
                          ),
                          PopupMenuItem(
                            value: 'unfollow',
                            child: Text('Stop following'),
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
class AskTrustDialog extends StatefulWidget {
  const AskTrustDialog({super.key, required this.name});

  final String name;

  @override
  State<AskTrustDialog> createState() => _AskTrustDialogState();
}

class _AskTrustDialogState extends State<AskTrustDialog> {
  final _note = TextEditingController();

  @override
  void dispose() {
    _note.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text('Ask ${widget.name} if they know you?'),
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
