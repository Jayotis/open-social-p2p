import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:image_picker/image_picker.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';

/// Most photos or videos in one post (the node's limit).
const _maxAttachments = 4;

/// Ask before uploading more than this on mobile data.
const _mobileDataWarnBytes = 20 << 20;

/// Writes a post. Photos and videos go to the home node as they are; the
/// node makes the 720p version and strips their metadata before publishing.
class ComposeScreen extends StatefulWidget {
  const ComposeScreen({super.key, required this.node});

  final HomeNode node;

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
  StreamSubscription<PostProgress>? _sending;
  PostProgress? _progress;
  String? _error;

  int get _totalBytes => _files.fold(0, (sum, f) => sum + f.size);
  bool get _busy => _sending != null;

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
      final go = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: const Text('Upload on mobile data?'),
          content: Text(
            "You're on mobile data. Posting sends ${formatBytes(_totalBytes)} to your home "
            'node, which then makes a smaller version to publish.',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('Wait for Wi-Fi'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('Upload'),
            ),
          ],
        ),
      );
      if (go != true) return;
    }

    setState(() {
      _error = null;
      _progress = null;
    });
    widget.node.uploading = true;
    final stream = widget.node.app.post(
      text: _text.text.trim(),
      files: [for (final f in _files) f.file.path],
    );
    _sending = stream.listen((p) {
      setState(() => _progress = p);
      switch (p.stage) {
        case PostStage.done:
          _finish(null);
        case PostStage.failed:
          _finish(p.message ?? 'unknown error');
        default:
          break;
      }
    }, onError: (Object e) => _finish(describeError(e)));
  }

  void _finish(String? error) {
    widget.node.uploading = false;
    _sending?.cancel();
    if (!mounted) return;
    if (error == null) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('Posted.')));
      widget.node.refresh();
      Navigator.of(context).pop();
      return;
    }
    setState(() {
      _sending = null;
      _error = error;
    });
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final canPost =
        !_busy && (_text.text.trim().isNotEmpty || _files.isNotEmpty);
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
          title: const Text('New post'),
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
              enabled: !_busy,
              autofocus: true,
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
                        ),
                      ),
                title: Text(f.file.name, overflow: TextOverflow.ellipsis),
                subtitle: Text(formatBytes(f.size)),
                trailing: IconButton(
                  icon: const Icon(Icons.close),
                  onPressed: _busy
                      ? null
                      : () => setState(() => _files.remove(f)),
                ),
              ),
            if (_files.length < _maxAttachments)
              OutlinedButton.icon(
                onPressed: _busy ? null : _pick,
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
            if (_busy) _ProgressView(progress: _progress),
            if (_error != null)
              Text(
                "Couldn't post: $_error",
                style: TextStyle(color: theme.colorScheme.error),
              ),
          ],
        ),
      ),
    );
  }
}

class _ProgressView extends StatelessWidget {
  const _ProgressView({required this.progress});

  final PostProgress? progress;

  @override
  Widget build(BuildContext context) {
    final p = progress;
    if (p == null || p.stage == PostStage.processing || p.total == 0) {
      return const Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          LinearProgressIndicator(),
          SizedBox(height: 8),
          Text('Your home node is preparing your post…'),
        ],
      );
    }
    final fraction = p.sent / p.total;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        LinearProgressIndicator(value: fraction),
        const SizedBox(height: 8),
        Text(
          'Sending to your home node: ${(fraction * 100).round()}% '
          '(${formatBytes(p.sent)} of ${formatBytes(p.total)})',
        ),
      ],
    );
  }
}
