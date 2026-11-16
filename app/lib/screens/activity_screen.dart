import 'dart:async';

import 'package:flutter/material.dart';

import '../activity_notifications.dart';
import '../activity_words.dart';
import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';
import 'profile_screen.dart';
import 'reply_requests_screen.dart';
import 'thread_screen.dart';
import 'trust_screen.dart';
import '../widgets/empty_state.dart';
import '../widgets/person_avatar.dart';

/// Entries asked for at a time.
const activityPage = 50;

/// Activity (ADR 0038 §3): replies, likes, reshares, new followers, people
/// who say they know you, and your replies published, in one place. What's
/// read is kept on the node, so every phone agrees: what's shown here is
/// marked read once it's shown, and stays marked unread on screen until the
/// next time.
class ActivityScreen extends StatefulWidget {
  const ActivityScreen({super.key, required this.node, required this.open});

  final HomeNode node;
  final void Function(Widget screen) open;

  @override
  State<ActivityScreen> createState() => _ActivityScreenState();
}

class _ActivityScreenState extends State<ActivityScreen> {
  List<ActivityItem> _entries = [];
  bool _more = false;
  bool _loading = true;
  String? _error;

  HomeNode get _node => widget.node;

  @override
  void initState() {
    super.initState();
    unawaited(clearActivityNotice());
    _node.checkStatus();
    _load();
  }

  Future<void> _load() async {
    setState(() => _loading = true);
    try {
      final page = await _node.app.activity(limit: activityPage);
      if (!mounted) return;
      setState(() {
        _entries = page.entries;
        _more = page.more;
        _error = null;
      });
      await _markShown(page.entries);
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  /// Older entries; the first page's mark covered them.
  Future<void> _loadMore() async {
    if (_entries.isEmpty) return;
    setState(() => _loading = true);
    try {
      final page = await _node.app.activity(
        before: _entries.last.seq,
        limit: activityPage,
      );
      if (!mounted) return;
      setState(() {
        _entries = [..._entries, ...page.entries];
        _more = page.more;
      });
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  /// Marks read on the node everything up to the newest entry shown. One
  /// that changes after takes a later place, and stays unread.
  Future<void> _markShown(List<ActivityItem> shown) async {
    if (shown.every((e) => e.read)) return;
    try {
      await _node.app.activityRead(keys: const [], through: shown.first.seq);
      await _node.checkStatus();
    } catch (_) {
      // Unread still; it's marked the next time it's shown.
    }
  }

  /// Opens what [e] is about.
  Future<void> _openEntry(ActivityItem e) async {
    final node = _node;
    switch (e.kind) {
      case 'reply' when e.waiting:
        widget.open(ReplyRequestsScreen(node: node));
      case 'knows_you':
        widget.open(TrustScreen(node: node));
      case 'followed' when e.people.isNotEmpty:
        widget.open(_profile(e.people.first));
      case 'followed':
        break;
      default:
        final id = e.post;
        if (id == null) return;
        final messenger = ScaffoldMessenger.of(context);
        try {
          final post = await node.app.showPost(id: id);
          widget.open(ThreadScreen(node: node, post: post));
        } catch (_) {
          messenger.showSnackBar(
            const SnackBar(content: Text("That post isn't on your node.")),
          );
        }
    }
  }

  Widget _profile(ActivityName p) =>
      ProfileScreen(node: _node, identity: p.identity, name: p.name);

  @override
  Widget build(BuildContext context) {
    final node = _node;
    final theme = Theme.of(context);
    return Scaffold(
      appBar: AppBar(title: const Text('Activity')),
      body: RefreshIndicator(
        onRefresh: _load,
        child: ListenableBuilder(
          listenable: node,
          builder: (context, _) {
            final replies = node.status?.replyRequests ?? 0;
            final knows = node.status?.trustRequests ?? 0;
            return ListView(
              children: [
                // What waits for an answer, however long ago it came.
                if (replies > 0)
                  _Waiting(
                    icon: Icons.forum_outlined,
                    title: 'Replies waiting for you',
                    count: replies,
                    onTap: () => widget.open(ReplyRequestsScreen(node: node)),
                  ),
                if (knows > 0)
                  _Waiting(
                    icon: Icons.handshake_outlined,
                    title: 'People who say they know you',
                    count: knows,
                    onTap: () => widget.open(TrustScreen(node: node)),
                  ),
                if (replies + knows > 0) const Divider(),
                if (_loading && _entries.isEmpty)
                  const LinearProgressIndicator(),
                if (_error case final error?)
                  Padding(
                    padding: const EdgeInsets.all(Space.l),
                    child: Text(
                      "Couldn't load your activity: $error",
                      style: TextStyle(color: theme.colorScheme.error),
                    ),
                  ),
                if (!_loading && _error == null && _entries.isEmpty)
                  const EmptyState(
                    icon: Icons.notifications_none,
                    message:
                        'Nothing yet. Replies, likes and new followers show '
                        'up here.',
                  ),
                for (final e in _entries)
                  _EntryTile(entry: e, onTap: () => _openEntry(e)),
                if (_more)
                  Center(
                    child: TextButton(
                      onPressed: _loading ? null : _loadMore,
                      child: const Text('Earlier'),
                    ),
                  ),
              ],
            );
          },
        ),
      ),
    );
  }
}

IconData _icon(ActivityItem e) => switch (e.kind) {
  'reply' || 'reply_published' => Icons.chat_bubble_outline,
  'liked' => Icons.favorite_border,
  'reshared' => Icons.repeat,
  'followed' => Icons.person_add_alt,
  'knows_you' => Icons.handshake_outlined,
  _ => Icons.notifications_outlined,
};

class _EntryTile extends StatelessWidget {
  const _EntryTile({required this.entry, required this.onTap});

  final ActivityItem entry;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final quote = activityQuote(entry);
    final when = formatAgo(
      DateTime.fromMillisecondsSinceEpoch(entry.atMs.toInt()),
    );
    final opens = !(entry.kind == 'followed' && entry.people.isEmpty);
    final scheme = theme.colorScheme;
    final first = entry.people.firstOrNull;
    return ListTile(
      tileColor: entry.read ? null : scheme.primary.withValues(alpha: 0.07),
      leading: SizedBox(
        width: 44,
        height: 44,
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            // Who it's from, as their star; someone unnamed, as a plain one.
            InitialAvatar(
              name: first?.name ?? '',
              seed: first?.identity ?? entry.kind,
              radius: 20,
            ),
            // What happened, small at the corner.
            Positioned(
              right: -2,
              bottom: -2,
              child: Container(
                padding: const EdgeInsets.all(3),
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: scheme.surfaceContainerHighest,
                  border: Border.all(color: scheme.surface, width: 2),
                ),
                child: Icon(_icon(entry), size: 12, color: scheme.primary),
              ),
            ),
            if (!entry.read)
              Positioned(
                left: -2,
                top: -2,
                child: Container(
                  width: 10,
                  height: 10,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: scheme.tertiary,
                    border: Border.all(color: scheme.surface, width: 1.5),
                  ),
                ),
              ),
          ],
        ),
      ),
      title: Text(
        activityLine(entry),
        style: theme.textTheme.bodyLarge?.copyWith(
          fontWeight: entry.read ? FontWeight.w400 : FontWeight.w600,
        ),
      ),
      subtitle: Text(
        [?quote, when].join('\n'),
        maxLines: 3,
        overflow: TextOverflow.ellipsis,
        style: theme.textTheme.bodySmall,
      ),
      onTap: opens ? onTap : null,
    );
  }
}

class _Waiting extends StatelessWidget {
  const _Waiting({
    required this.icon,
    required this.title,
    required this.count,
    required this.onTap,
  });

  final IconData icon;
  final String title;
  final int count;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return ListTile(
      leading: Icon(icon),
      title: Text(title),
      trailing: Badge(label: Text('$count')),
      onTap: onTap,
    );
  }
}
