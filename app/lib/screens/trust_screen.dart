import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';

/// Trusted followers (ADR 0013): requests to answer, and who is trusted.
class TrustScreen extends StatefulWidget {
  const TrustScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<TrustScreen> createState() => _TrustScreenState();
}

class _TrustScreenState extends State<TrustScreen> {
  TrustList? _list;
  String? _error;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final list = await widget.node.app.trustList();
      if (!mounted) return;
      setState(() {
        _list = list;
        _error = null;
      });
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    }
    // The badge's count.
    widget.node.checkStatus();
  }

  /// Runs [action], then says [done], or why it failed, and reloads.
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
    await _load();
  }

  Future<void> _allow(TrustEntry r) async {
    if (!await confirmTrust(context, r.name)) return;
    await _do(
      () => widget.node.app.answerTrust(node: r.node, allow: true),
      'Trusted. They aren\'t told.',
    );
  }

  Future<void> _decline(TrustEntry r) => _do(
    () => widget.node.app.answerTrust(node: r.node, allow: false),
    'Declined. They aren\'t told, and their requests are ignored for 30 days.',
  );

  Future<void> _remove(TrustEntry t) async {
    final go = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Stop trusting ${t.name}?'),
        content: const Text(
          'Your node stops naming them as a host more often, on every one '
          'of their nodes. They aren\'t told.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Stop trusting'),
          ),
        ],
      ),
    );
    if (go != true) return;
    await _do(
      () => widget.node.app.untrust(identity: t.identity),
      'No longer trusted.',
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final list = _list;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Trusted followers'),
        actions: [
          IconButton(
            tooltip: 'Refresh',
            onPressed: _busy ? null : _load,
            icon: const Icon(Icons.refresh),
          ),
        ],
      ),
      body: RefreshIndicator(
        onRefresh: _load,
        child: ListView(
          padding: const EdgeInsets.all(16),
          children: [
            const Text(
              'People you know who follow you. For now, trusting someone '
              'changes one thing: your node names them as hosts of your '
              'photos and videos more often. It grants them nothing else.',
            ),
            if (_busy)
              const Padding(
                padding: EdgeInsets.only(top: 16),
                child: LinearProgressIndicator(),
              ),
            if (_error case final error?)
              Padding(
                padding: const EdgeInsets.only(top: 16),
                child: Text(
                  "Couldn't load them: $error",
                  style: TextStyle(color: theme.colorScheme.error),
                ),
              ),
            if (list == null && _error == null)
              const Padding(
                padding: EdgeInsets.all(32),
                child: Center(child: CircularProgressIndicator()),
              ),
            if (list != null) ...[
              const SizedBox(height: 24),
              Text('Asking to be trusted', style: theme.textTheme.titleMedium),
              const SizedBox(height: 8),
              if (list.requests.isEmpty) const Text('No requests waiting.'),
              for (final r in list.requests)
                _RequestCard(
                  request: r,
                  busy: _busy,
                  onAllow: () => _allow(r),
                  onDecline: () => _decline(r),
                ),
              const SizedBox(height: 24),
              Text('Trusted', style: theme.textTheme.titleMedium),
              const SizedBox(height: 8),
              if (list.trusted.isEmpty)
                const Text(
                  'Nobody yet. To trust someone you follow, find them under '
                  'Follow and be followed.',
                ),
              for (final t in list.trusted)
                Card(
                  child: ListTile(
                    title: Text(shownName(t.name)),
                    subtitle: Text(
                      [
                        'Trusted ${_ago(t.secsAgo)}',
                        if (t.followed) 'you follow them',
                        followerLine(t),
                      ].join(' · '),
                    ),
                    trailing: IconButton(
                      tooltip: 'Stop trusting',
                      onPressed: _busy ? null : () => _remove(t),
                      icon: const Icon(Icons.person_remove_outlined),
                    ),
                  ),
                ),
            ],
          ],
        ),
      ),
    );
  }
}

class _RequestCard extends StatelessWidget {
  const _RequestCard({
    required this.request,
    required this.busy,
    required this.onAllow,
    required this.onDecline,
  });

  final TrustEntry request;
  final bool busy;
  final VoidCallback onAllow;
  final VoidCallback onDecline;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final r = request;
    final small = theme.textTheme.bodySmall;
    return Card(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 12, 8, 4),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(shownName(r.name), style: theme.textTheme.titleSmall),
            Text('Asked ${_ago(r.secsAgo)}', style: small),
            if (r.note.isNotEmpty) ...[
              const SizedBox(height: 8),
              // Their words, shown as plain text, never as a link.
              Text('They say: "${r.note}"'),
            ],
            const SizedBox(height: 8),
            if (r.followed)
              const Text('You follow them, so this is the person you added.')
            else
              Text(
                "You don't follow them, so only they say who they are. To be "
                'sure, get their follow code in person or another way you '
                'trust, follow them, then allow.',
                style: TextStyle(color: theme.colorScheme.error),
              ),
            if (r.trustedElsewhere)
              const Text('Already trusted on another of their nodes.'),
            const SizedBox(height: 4),
            Text(followerLine(r), style: small),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                TextButton(
                  onPressed: busy ? null : onDecline,
                  child: const Text('Decline'),
                ),
                TextButton(
                  onPressed: busy ? null : onAllow,
                  child: const Text('Allow'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

/// Asks before trusting [name], saying what it changes.
Future<bool> confirmTrust(BuildContext context, String name) async {
  final go = await showDialog<bool>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text('Trust ${shownName(name)}?'),
      content: const Text(
        'Your node will name them to your other followers as a host of your '
        'photos and videos more often, so more of those followers can see '
        'that this person follows you. It grants them nothing else, and '
        'they aren\'t told.',
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context, false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context, true),
          child: const Text('Trust'),
        ),
      ],
    ),
  );
  return go == true;
}

String shownName(String name) => name.isEmpty ? '(no name)' : name;

/// How long a node has followed, how it reached the owner's lately, and the
/// copies it holds.
String followerLine(TrustEntry t) {
  return [
    switch (t.followerSecs) {
      final secs? => 'follows you for ${formatUptime(secs)}',
      null => 'no longer follows you',
    },
    switch (t.reached) {
      'direct' => 'reached your node directly lately',
      'relay' => 'reached your node through a relay lately',
      'tor' => 'reached your node over Tor lately',
      _ => 'not heard from lately',
    },
    'holds ${t.copies} of your files',
  ].join(' · ');
}

String _ago(int secs) =>
    formatAgo(DateTime.now().subtract(Duration(seconds: secs)));
