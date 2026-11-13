import 'dart:convert';

import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import 'trust_screen.dart' show shownName;

/// The most a reply may be (ADR 0028), in bytes of UTF-8.
const maxReplyBytes = 4000;

/// A post's replies (ADR 0028), and a box to reply in.
///
/// On the owner's own post it shows every reply kept, hidden ones too, and
/// who may reply. On anyone else's it shows the published ones, read from
/// the creator's node without saying who reads, and the owner's own replies
/// with where each stands.
class ThreadScreen extends StatefulWidget {
  const ThreadScreen({super.key, required this.node, required this.post});

  final HomeNode node;
  final Post post;

  @override
  State<ThreadScreen> createState() => _ThreadScreenState();
}

class _ThreadScreenState extends State<ThreadScreen> {
  final _text = TextEditingController();
  String? _policy;
  List<Reply> _replies = [];
  List<SentReply> _sent = [];
  bool _more = false;
  bool _loading = true;
  bool _busy = false;
  String? _error;

  HomeNode get _node => widget.node;
  OspApp get _app => widget.node.app;

  bool get _own => widget.post.authorIdentity == _node.paired?.identity;
  String? get _me => _node.paired?.identity;

  @override
  void initState() {
    super.initState();
    _text.addListener(() => setState(() {}));
    _node.addListener(_changed);
    _load();
  }

  @override
  void dispose() {
    _node.removeListener(_changed);
    _text.dispose();
    super.dispose();
  }

  /// Deletes started here, until they're sent or undone.
  final Set<String> _deletes = {};

  /// A delete waiting: redraw without it. Once sent or undone, load again
  /// to see what the node has.
  void _changed() {
    if (!mounted) return;
    final over = _deletes.where((id) => !_node.isDeleting(id)).toList();
    _deletes.removeAll(over);
    setState(() {});
    if (over.isNotEmpty) _load();
  }

  /// Loads the first page again, and the owner's replies to this post.
  Future<void> _load() async {
    setState(() => _loading = true);
    try {
      final thread = await _app.thread(postId: widget.post.id, after: 0);
      final sent = _own
          ? <SentReply>[]
          : (await _app.sentReplies())
                .where((s) => s.post == widget.post.id)
                .toList();
      if (!mounted) return;
      setState(() {
        _policy = _own ? thread.policy : null;
        _replies = thread.replies;
        _more = thread.more;
        _sent = sent;
        _error = null;
      });
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _loadMore() async {
    if (_replies.isEmpty) return;
    setState(() => _loading = true);
    try {
      final thread = await _app.thread(
        postId: widget.post.id,
        after: _replies.last.seq,
      );
      if (!mounted) return;
      setState(() {
        _replies = [..._replies, ...thread.replies];
        _more = thread.more;
      });
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    } finally {
      if (mounted) setState(() => _loading = false);
    }
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

  Future<void> _send() async {
    final text = _text.text.trim();
    if (text.isEmpty) return;
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _busy = true);
    try {
      final sent = await _app.reply(postId: widget.post.id, text: text);
      _text.clear();
      messenger.showSnackBar(
        SnackBar(content: Text(sentStatusLine(sent, widget.post.author))),
      );
    } catch (e) {
      messenger.showSnackBar(
        SnackBar(content: Text("Couldn't reply: ${describeError(e)}")),
      );
    } finally {
      if (mounted) setState(() => _busy = false);
    }
    await _load();
  }

  Future<void> _setPolicy(String policy) => _do(
    () => _app.setReplyPolicy(postId: widget.post.id, policy: policy),
    switch (policy) {
      'off' => 'Nobody can reply now.',
      'circle' => 'Only your circle can reply now.',
      _ => 'Your circle can reply, and others ask you first.',
    },
  );

  Future<void> _hide(Reply r, bool hidden) => _do(
    () => _app.hideReply(reply: r.id, hidden: hidden),
    hidden ? 'Hidden. Nothing was deleted.' : 'Shown again.',
  );

  Future<void> _block(Reply r) async {
    if (!await confirmBlock(context, r.name)) return;
    await _do(
      () => _app.setReplier(identity: r.identity, choice: 'block'),
      "Blocked. They aren't told.",
    );
  }

  /// Deletes one of the owner's replies after a minute, with an undo
  /// meanwhile, as posts are (ADR 0019).
  void _delete(String id) {
    final messenger = ScaffoldMessenger.of(context);
    _deletes.add(id);
    _node.deleteLater(
      id,
      onFailed: (e) => messenger.showSnackBar(
        SnackBar(content: Text("Couldn't delete it: ${describeError(e)}")),
      ),
    );
    messenger.showSnackBar(
      SnackBar(
        content: const Text('Deleting your reply in a minute.'),
        duration: HomeNode.deleteWait,
        action: SnackBarAction(
          label: 'Undo',
          onPressed: () => _node.undoDelete(id),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final online = _node.reachability == Reachability.online;
    final replies = _replies.where((r) => !_node.isDeleting(r.id)).toList();
    final shown = {for (final r in replies) r.id};
    // The owner's replies not in the thread yet, or no longer: waiting,
    // refused, being sent.
    final waiting = _sent
        .where((s) => !shown.contains(s.id) && !_node.isDeleting(s.id))
        .toList();
    final bytes = utf8.encode(_text.text.trim()).length;
    final tooLong = bytes > maxReplyBytes;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Replies'),
        actions: [
          IconButton(
            tooltip: 'Refresh',
            onPressed: _loading || _busy ? null : _load,
            icon: const Icon(Icons.refresh),
          ),
        ],
      ),
      body: Column(
        children: [
          Expanded(
            child: RefreshIndicator(
              onRefresh: _load,
              child: ListView(
                padding: const EdgeInsets.fromLTRB(16, 8, 16, 16),
                children: [
                  _PostSummary(post: widget.post),
                  if (_policy case final policy?) ...[
                    const SizedBox(height: 16),
                    _PolicyPicker(
                      policy: policy,
                      enabled: online && !_busy,
                      onChanged: _setPolicy,
                    ),
                  ],
                  if (_loading || _busy)
                    const Padding(
                      padding: EdgeInsets.only(top: 16),
                      child: LinearProgressIndicator(),
                    ),
                  if (_error case final error?)
                    Padding(
                      padding: const EdgeInsets.only(top: 16),
                      child: Text(
                        "Couldn't load the replies: $error",
                        style: TextStyle(color: theme.colorScheme.error),
                      ),
                    ),
                  const SizedBox(height: 16),
                  if (!_loading && _error == null && replies.isEmpty)
                    const Text('No replies yet.'),
                  for (final r in replies)
                    _ReplyCard(
                      reply: r,
                      own: _own,
                      mine: r.identity == _me,
                      busy: _busy || !online,
                      onHide: (hidden) => _hide(r, hidden),
                      onBlock: () => _block(r),
                      onDelete: () => _delete(r.id),
                    ),
                  if (_more)
                    Center(
                      child: TextButton(
                        onPressed: _loading ? null : _loadMore,
                        child: const Text('More replies'),
                      ),
                    ),
                  if (waiting.isNotEmpty) ...[
                    const SizedBox(height: 16),
                    Text('Your replies', style: theme.textTheme.titleMedium),
                    const SizedBox(height: 8),
                    for (final s in waiting)
                      _SentCard(
                        sent: s,
                        author: widget.post.author,
                        busy: _busy || !online,
                        onDelete: s.status == 'deleting'
                            ? null
                            : () => _delete(s.id),
                      ),
                  ],
                ],
              ),
            ),
          ),
          if (_policy != 'off')
            SafeArea(
              top: false,
              child: Padding(
                padding: const EdgeInsets.fromLTRB(16, 4, 8, 8),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.end,
                  children: [
                    Expanded(
                      child: TextField(
                        controller: _text,
                        minLines: 1,
                        maxLines: 5,
                        textCapitalization: TextCapitalization.sentences,
                        decoration: InputDecoration(
                          hintText: 'Reply to ${shownName(widget.post.author)}',
                          helperText: _own
                              ? null
                              : 'Signed with your name, and seen by '
                                    'whoever reads the thread.',
                          helperMaxLines: 2,
                          errorText: tooLong
                              ? 'Too long: $bytes of $maxReplyBytes bytes'
                              : null,
                        ),
                      ),
                    ),
                    IconButton(
                      tooltip: 'Send reply',
                      onPressed: online && !_busy && bytes > 0 && !tooLong
                          ? _send
                          : null,
                      icon: const Icon(Icons.send),
                    ),
                  ],
                ),
              ),
            ),
        ],
      ),
    );
  }
}

class _PostSummary extends StatelessWidget {
  const _PostSummary({required this.post});

  final Post post;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final published = DateTime.tryParse(post.published);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          [
            shownName(post.author),
            if (published != null) formatAgo(published),
          ].join(' · '),
          style: theme.textTheme.titleSmall,
        ),
        if (post.text.isNotEmpty) ...[
          const SizedBox(height: 4),
          Text(post.text, maxLines: 6, overflow: TextOverflow.ellipsis),
        ],
        if (post.media.isNotEmpty)
          Text(
            post.media.length == 1
                ? 'With a photo or video'
                : 'With ${post.media.length} photos and videos',
            style: theme.textTheme.bodySmall,
          ),
        const Divider(height: 24),
      ],
    );
  }
}

/// Who may reply to the owner's post.
class _PolicyPicker extends StatelessWidget {
  const _PolicyPicker({
    required this.policy,
    required this.enabled,
    required this.onChanged,
  });

  final String policy;
  final bool enabled;
  final ValueChanged<String> onChanged;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text('Who can reply', style: Theme.of(context).textTheme.titleSmall),
        const SizedBox(height: 8),
        SegmentedButton<String>(
          segments: const [
            ButtonSegment(value: 'off', label: Text('Nobody')),
            ButtonSegment(value: 'circle', label: Text('Circle')),
            ButtonSegment(value: 'network', label: Text('Anyone asks')),
          ],
          selected: {policy},
          onSelectionChanged: enabled ? (s) => onChanged(s.first) : null,
        ),
        const SizedBox(height: 8),
        Text(switch (policy) {
          'off' => 'Nobody can reply. Replies already published stay.',
          'circle' =>
            'People you follow, your trusted followers and those you '
                'allowed. Nobody else can.',
          _ =>
            'Your circle is published at once. Anyone else waits among '
                'your requests until you look.',
        }, style: Theme.of(context).textTheme.bodySmall),
      ],
    );
  }
}

class _ReplyCard extends StatelessWidget {
  const _ReplyCard({
    required this.reply,
    required this.own,
    required this.mine,
    required this.busy,
    required this.onHide,
    required this.onBlock,
    required this.onDelete,
  });

  final Reply reply;

  /// On the owner's own post, so they can hide it or block its replier.
  final bool own;

  /// Written by the owner, so they can delete it.
  final bool mine;
  final bool busy;
  final ValueChanged<bool> onHide;
  final VoidCallback onBlock;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final hidden = reply.state == 'hidden';
    final published = DateTime.tryParse(reply.published);
    final actions = <PopupMenuEntry<String>>[
      if (mine) const PopupMenuItem(value: 'delete', child: Text('Delete')),
      if (own && !mine)
        PopupMenuItem(
          value: hidden ? 'show' : 'hide',
          child: Text(hidden ? 'Show again' : 'Hide from the thread'),
        ),
      if (own && !mine)
        const PopupMenuItem(value: 'block', child: Text('Block them')),
    ];
    return Card(
      child: Opacity(
        opacity: hidden ? 0.6 : 1,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 4, 12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Expanded(
                    child: Text(
                      [
                        shownName(reply.name),
                        if (published != null) formatAgo(published),
                        if (hidden) 'hidden',
                      ].join(' · '),
                      style: theme.textTheme.titleSmall,
                    ),
                  ),
                  if (actions.isNotEmpty)
                    PopupMenuButton<String>(
                      enabled: !busy,
                      tooltip: 'Reply options',
                      onSelected: (choice) => switch (choice) {
                        'delete' => onDelete(),
                        'hide' => onHide(true),
                        'show' => onHide(false),
                        _ => onBlock(),
                      },
                      itemBuilder: (_) => actions,
                    )
                  else
                    const SizedBox(height: 40),
                ],
              ),
              Padding(
                padding: const EdgeInsets.only(right: 12),
                // Their words, as plain text, never as a link.
                child: SelectableText(reply.text),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// One of the owner's replies to this post that isn't in its thread.
class _SentCard extends StatelessWidget {
  const _SentCard({
    required this.sent,
    required this.author,
    required this.busy,
    required this.onDelete,
  });

  final SentReply sent;
  final String author;
  final bool busy;
  final VoidCallback? onDelete;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final trouble = const {
      'refused',
      'gave_up',
      'not_published',
      'closed',
      'deleted',
    }.contains(sent.status);
    return Card(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 12, 4, 8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (sent.text.isNotEmpty) Text(sent.text),
            const SizedBox(height: 4),
            Row(
              children: [
                Expanded(
                  child: Text(
                    sentStatusLine(sent, author),
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: trouble ? theme.colorScheme.error : null,
                    ),
                  ),
                ),
                if (onDelete != null)
                  IconButton(
                    tooltip: 'Delete your reply',
                    onPressed: busy ? null : onDelete,
                    icon: const Icon(Icons.delete_outline),
                  ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

/// Where one of the owner's replies stands, in words.
String sentStatusLine(SentReply s, String author) {
  final who = shownName(author);
  return switch (s.status) {
    'sending' => switch (s.nextTrySecs) {
      final secs? when secs > 0 =>
        "Not delivered yet: $who's node didn't answer. "
            'Trying again in ${formatUptime(secs)}.',
      _ => 'Being sent to $who\'s node.',
    },
    'published' => 'In the thread.',
    'awaiting_approval' =>
      "Waiting for $who to look at it. You'll see it here once it's "
          'published.',
    'not_published' => "$who's node didn't publish it.",
    'closed' => 'This post takes no replies.',
    'deleted' => 'The post was deleted.',
    'refused' => "Refused by $who's node: ${s.error ?? 'no reason given'}.",
    'gave_up' => "$who's node didn't answer in 30 days, so it wasn't sent.",
    'deleting' => 'Being deleted.',
    _ => s.status,
  };
}

/// Asks before blocking [name] from replying, saying what it does.
Future<bool> confirmBlock(BuildContext context, String name) async {
  final go = await showDialog<bool>(
    context: context,
    builder: (context) => AlertDialog(
      title: Text('Block ${shownName(name)}?'),
      content: const Text(
        'Their replies on your posts are hidden, and new ones are refused. '
        "They aren't told: they get the same answer as any refusal.",
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context, false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context, true),
          child: const Text('Block'),
        ),
      ],
    ),
  );
  return go == true;
}
