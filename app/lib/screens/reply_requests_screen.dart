import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import 'thread_screen.dart' show confirmBlock;
import 'trust_screen.dart' show shownName;

/// Replies to the owner's posts from people outside their circle, waiting
/// for an answer (ADR 0028).
class ReplyRequestsScreen extends StatefulWidget {
  const ReplyRequestsScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<ReplyRequestsScreen> createState() => _ReplyRequestsScreenState();
}

class _ReplyRequestsScreenState extends State<ReplyRequestsScreen> {
  List<Reply>? _requests;
  String? _error;
  bool _busy = false;

  OspApp get _app => widget.node.app;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final requests = await _app.replyRequests();
      if (!mounted) return;
      setState(() {
        _requests = requests;
        _error = null;
      });
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    }
    // The badge's count.
    widget.node.checkStatus();
  }

  Future<void> _answer(Reply r, String answer) async {
    if (answer == 'block' && !await confirmBlock(context, r.name)) return;
    if (!mounted) return;
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _busy = true);
    try {
      await _app.answerReply(reply: r.id, answer: answer);
      messenger.showSnackBar(
        SnackBar(
          content: Text(switch (answer) {
            'publish' => 'Published.',
            'publish_and_allow' =>
              'Published. Their replies to your posts are published from now '
                  'on.',
            'decline' => "Declined. They aren't told.",
            _ => "Blocked. They aren't told.",
          }),
        ),
      );
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(describeError(e))));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
    await _load();
  }

  /// The post a request answers, if it's in the feed.
  Post? _post(String id) {
    for (final p in widget.node.posts) {
      if (p.id == id) return p;
    }
    return null;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final requests = _requests;
    final online = widget.node.reachability == Reachability.online;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Replies waiting'),
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
              "Replies to your posts from people outside your circle. Nobody "
              'sees them until you publish them. They go after 30 days.',
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
            if (requests == null && _error == null)
              const Padding(
                padding: EdgeInsets.all(32),
                child: Center(child: CircularProgressIndicator()),
              ),
            if (requests != null) ...[
              const SizedBox(height: 16),
              if (requests.isEmpty) const Text('No replies waiting.'),
              for (final r in requests)
                _RequestCard(
                  request: r,
                  post: _post(r.post),
                  busy: _busy || !online,
                  onAnswer: (answer) => _answer(r, answer),
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
    required this.post,
    required this.busy,
    required this.onAnswer,
  });

  final Reply request;
  final Post? post;
  final bool busy;
  final ValueChanged<String> onAnswer;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final r = request;
    final small = theme.textTheme.bodySmall;
    final published = DateTime.tryParse(r.published);
    return Card(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 12, 8, 4),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(shownName(r.name), style: theme.textTheme.titleSmall),
            Text(
              [
                if (published != null) 'Wrote ${formatAgo(published)}',
                ?r.why,
              ].join(' · '),
              style: small,
            ),
            if (post case final post? when post.text.isNotEmpty) ...[
              const SizedBox(height: 4),
              Text(
                'On your post: "${post.text.split('\n').first}"',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: small,
              ),
            ],
            const SizedBox(height: 8),
            // Their words, as plain text, never as a link.
            SelectableText(r.text),
            const SizedBox(height: 4),
            Text(
              'The name is the one their profile gives. Your node checked '
              'that the reply is signed by that profile, not who is behind it.',
              style: small,
            ),
            OverflowBar(
              alignment: MainAxisAlignment.end,
              children: [
                TextButton(
                  onPressed: busy ? null : () => onAnswer('block'),
                  child: const Text('Block'),
                ),
                TextButton(
                  onPressed: busy ? null : () => onAnswer('decline'),
                  child: const Text('Decline'),
                ),
                TextButton(
                  onPressed: busy ? null : () => onAnswer('publish'),
                  child: const Text('Publish'),
                ),
                TextButton(
                  onPressed: busy ? null : () => onAnswer('publish_and_allow'),
                  child: const Text('Publish and allow'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}
