import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:image_picker/image_picker.dart';

import '../background_upload.dart';
import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';

/// Most photos or videos in one post (the node's limit).
const _maxAttachments = 4;

/// Ask before uploading more than this on mobile data.
const _mobileDataWarnBytes = 20 << 20;

/// Writes a post. Photos and videos go to the home node as they are; the
/// node makes the 720p version and strips their metadata before publishing.
///
/// With [unsent], it carries on sending a post that didn't finish instead.
class ComposeScreen extends StatefulWidget {
  const ComposeScreen({super.key, required this.node, this.unsent});

  final HomeNode node;
  final UnsentPost? unsent;

  @override
  State<ComposeScreen> createState() => _ComposeScreenState();
}

class _Picked {
  _Picked(this.file, this.size);

  final XFile file;
  final int size;

  bool get isVideo {
    final type = file.mimeType;
    if (type != null) return type.startsWith('video/');
    final name = file.name.toLowerCase();
    return ['.mp4', '.mov', '.mkv', '.webm', '.3gp'].any(name.endsWith);
  }
}

class _ComposeScreenState extends State<ComposeScreen> {
  final _text = TextEditingController();
  final _files = <_Picked>[];
  final _background = BackgroundUpload();
  StreamSubscription<PostProgress>? _sending;
  PostProgress? _progress;

  /// Lost contact with the home node; trying again.
  bool _waiting = false;
  bool _discarding = false;
  String? _error;

  /// The post is kept on the phone after failing: try again or discard.
  bool _kept = false;

  int get _totalBytes => _files.fold(0, (sum, f) => sum + f.size);
  bool get _busy => _sending != null || _discarding;

  /// The text and files can't change once the post is on its way.
  bool get _frozen => _busy || _kept;

  @override
  void initState() {
    super.initState();
    final unsent = widget.unsent;
    if (unsent != null) {
      _text.text = unsent.text;
      for (final f in unsent.files) {
        _files.add(_Picked(XFile(f.path), f.size));
      }
      _kept = true;
      WidgetsBinding.instance.addPostFrameCallback(
        (_) => _send(widget.node.app.continuePost()),
      );
    }
  }

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  Future<void> _pick() async {
    final room = _maxAttachments - _files.length;
    if (room <= 0) return;
    final picked = await ImagePicker().pickMultipleMedia(
      limit: room > 1 ? room : null,
      requestFullMetadata: false,
    );
    final added = <_Picked>[];
    for (final file in picked.take(room)) {
      added.add(_Picked(file, await File(file.path).length()));
    }
    if (!mounted) return;
    setState(() => _files.addAll(added));
  }

  Future<void> _post() async {
    if (_text.text.trim().isEmpty && _files.isEmpty) return;
    if (widget.node.onMobileData && _totalBytes > _mobileDataWarnBytes) {
      final go = await _ask(
        title: 'Upload on mobile data?',
        text:
            "You're on mobile data. Posting sends ${formatBytes(_totalBytes)} to your home "
            'node, which then makes a smaller version to publish.',
        no: 'Wait for Wi-Fi',
        yes: 'Upload',
      );
      if (go != true) return;
    }
    if (widget.node.app.pendingPost() != null) {
      final go = await _ask(
        title: 'Discard the unsent post?',
        text:
            "A post that didn't finish sending is waiting. Posting this one "
            'discards it.',
        no: 'Keep it',
        yes: 'Discard it',
      );
      if (go != true) return;
      try {
        await widget.node.discardPost();
      } catch (_) {
        // Too late to stop it: the node publishes it.
      }
    }

    // Out of the picker's folder, which the phone may clear, so the post
    // can carry on later.
    final List<String> files;
    try {
      files = await widget.node.uploads.take([
        for (final f in _files) f.file.path,
      ]);
    } catch (e) {
      setState(() => _error = describeError(e));
      return;
    }
    if (!mounted) return;
    setState(() {
      for (final (i, f) in _files.toList().indexed) {
        _files[i] = _Picked(
          XFile(files[i], name: f.file.name, mimeType: f.file.mimeType),
          f.size,
        );
      }
    });
    _send(widget.node.app.post(text: _text.text.trim(), files: files));
  }

  void _send(Stream<PostProgress> stream) {
    setState(() {
      _error = null;
      _progress = null;
      _waiting = false;
    });
    widget.node.uploading = true;
    _background.start();
    _sending = stream.listen(
      (p) {
        switch (p.stage) {
          case PostStage.done:
            _finish(null);
          case PostStage.failed:
            _finish(_failure(p), kept: p.kept);
          case PostStage.waiting:
            setState(() => _waiting = true);
            _notify();
          default:
            setState(() {
              _progress = p;
              _waiting = false;
            });
            _notify();
        }
      },
      onError: (Object e) => _finish(
        describeError(e),
        kept: widget.node.app.pendingPost() != null,
      ),
    );
  }

  String _failure(PostProgress p) {
    if (!p.noContact) return p.message ?? 'unknown error';
    if (widget.node.phoneOffline) return 'your phone is offline.';
    return "your home computer isn't answering: is it on and online?";
  }

  /// Shows the post's progress in the notification.
  void _notify() {
    final p = _progress;
    final percent = p == null || p.total == 0 ? null : p.sent * 100 ~/ p.total;
    if (p?.stage == PostStage.processing) {
      _background.show('Your home node is preparing your post', '');
    } else if (_waiting) {
      _background.show(
        'Sending your post',
        'Waiting for your home node…',
        percent: percent,
      );
    } else if (p != null) {
      _background.show(
        'Sending your post',
        '$percent% of ${formatBytes(p.total)}',
        percent: percent,
      );
    }
  }

  void _finish(String? error, {bool kept = false}) {
    widget.node.uploading = false;
    _background.stop(sent: error == null);
    _sending?.cancel();
    _sending = null;
    if (error == null) {
      widget.node.uploads.release([for (final f in _files) f.file.path]);
      widget.node.checkUnsent();
      if (!mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('Posted.')));
      widget.node.refresh();
      _close();
      return;
    }
    widget.node.checkUnsent();
    if (!mounted) return;
    setState(() {
      _error = error;
      _kept = kept;
    });
  }

  /// Stops the post and discards it, on the phone and the node.
  Future<void> _discard() async {
    final go = await _ask(
      title: 'Discard this post?',
      text: "It hasn't been published. What was sent so far is deleted.",
      no: 'Keep it',
      yes: 'Discard',
    );
    if (go != true || !mounted) return;
    setState(() => _discarding = true);
    _sending?.cancel();
    _sending = null;
    widget.node.uploading = false;
    _background.stop();
    String message;
    try {
      await widget.node.discardPost();
      message = 'Post discarded.';
    } catch (e) {
      message = 'Too late to discard: ${describeError(e)}';
    }
    if (!mounted) return;
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text(message)));
    _close();
  }

  /// Closes this screen, and a dialog open on it, such as "Discard this
  /// post?" when the post finishes first.
  void _close() {
    final route = ModalRoute.of(context);
    Navigator.of(context)
      ..popUntil((r) => r == route)
      ..pop();
  }

  Future<bool?> _ask({
    required String title,
    required String text,
    required String no,
    required String yes,
  }) {
    return showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(title),
        content: Text(text),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: Text(no),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: Text(yes),
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final canPost =
        !_frozen && (_text.text.trim().isNotEmpty || _files.isNotEmpty);
    final preparing = _progress?.stage == PostStage.processing;
    return PopScope(
      canPop: !_busy,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) {
          ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(
              content: Text('Please wait until your post is sent.'),
            ),
          );
        }
      },
      child: Scaffold(
        appBar: AppBar(
          title: Text(widget.unsent == null ? 'New post' : 'Unsent post'),
          actions: [
            Padding(
              padding: const EdgeInsets.only(right: 12),
              child: FilledButton(
                onPressed: canPost ? _post : null,
                child: const Text('Post'),
              ),
            ),
          ],
        ),
        body: ListView(
          padding: const EdgeInsets.all(16),
          children: [
            TextField(
              controller: _text,
              enabled: !_frozen,
              autofocus: widget.unsent == null,
              minLines: 4,
              maxLines: 12,
              onChanged: (_) => setState(() {}),
              decoration: const InputDecoration(
                hintText: "What's new?",
                border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 12),
            for (final f in _files)
              ListTile(
                contentPadding: EdgeInsets.zero,
                leading: f.isVideo
                    ? const Icon(Icons.movie_outlined, size: 40)
                    : ClipRRect(
                        borderRadius: BorderRadius.circular(4),
                        child: Image.file(
                          File(f.file.path),
                          width: 48,
                          height: 48,
                          fit: BoxFit.cover,
                          errorBuilder: (_, _, _) =>
                              const Icon(Icons.broken_image_outlined, size: 40),
                        ),
                      ),
                title: Text(f.file.name, overflow: TextOverflow.ellipsis),
                subtitle: Text(formatBytes(f.size)),
                trailing: IconButton(
                  icon: const Icon(Icons.close),
                  onPressed: _frozen
                      ? null
                      : () => setState(() => _files.remove(f)),
                ),
              ),
            if (_files.length < _maxAttachments && !_frozen)
              OutlinedButton.icon(
                onPressed: _pick,
                icon: const Icon(Icons.add_photo_alternate_outlined),
                label: const Text('Add photos or videos'),
              ),
            if (_files.isNotEmpty) ...[
              const SizedBox(height: 8),
              Text(
                'Your home node makes a 720p copy and removes location and camera details '
                'before publishing. Videos can be up to 10 minutes.',
                style: theme.textTheme.bodySmall,
              ),
              if (widget.node.onMobileData)
                Text(
                  "You're on mobile data: posting uploads ${formatBytes(_totalBytes)}.",
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: theme.colorScheme.error,
                  ),
                ),
            ],
            const SizedBox(height: 16),
            if (_discarding)
              const _Working(text: 'Discarding…')
            else if (_sending != null) ...[
              _ProgressView(progress: _progress, waiting: _waiting),
              if (!preparing)
                Align(
                  alignment: Alignment.centerRight,
                  child: TextButton(
                    onPressed: _discard,
                    child: const Text('Cancel'),
                  ),
                ),
            ],
            if (_error != null)
              Text(
                "Couldn't post: $_error",
                style: TextStyle(color: theme.colorScheme.error),
              ),
            if (_kept && !_busy) ...[
              const SizedBox(height: 8),
              Text(
                'The post is kept on this phone, to try again later.',
                style: theme.textTheme.bodySmall,
              ),
              const SizedBox(height: 8),
              Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  TextButton(onPressed: _discard, child: const Text('Discard')),
                  const SizedBox(width: 8),
                  FilledButton(
                    onPressed: () => _send(widget.node.app.continuePost()),
                    child: const Text('Try again'),
                  ),
                ],
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _ProgressView extends StatelessWidget {
  const _ProgressView({required this.progress, required this.waiting});

  final PostProgress? progress;
  final bool waiting;

  @override
  Widget build(BuildContext context) {
    final p = progress;
    if (p != null && p.stage == PostStage.processing) {
      return const _Working(text: 'Your home node is preparing your post…');
    }
    if (p == null || p.total == 0) {
      return _Working(
        text: waiting
            ? 'Waiting for your home node…'
            : 'Connecting to your home node…',
      );
    }
    final fraction = p.sent / p.total;
    final sent =
        '${(fraction * 100).round()}% (${formatBytes(p.sent)} of ${formatBytes(p.total)})';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        LinearProgressIndicator(value: fraction),
        const SizedBox(height: 8),
        Text(
          waiting
              ? 'Waiting for your home node… $sent sent so far'
              : 'Sending to your home node: $sent',
        ),
      ],
    );
  }
}

class _Working extends StatelessWidget {
  const _Working({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const LinearProgressIndicator(),
        const SizedBox(height: 8),
        Text(text),
      ],
    );
  }
}
