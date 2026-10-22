import 'package:flutter/material.dart';
import 'package:video_player/video_player.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';

/// Ask before playing a video larger than this on mobile data.
const _mobileDataWarnBytes = 10 << 20;

/// Plays a video from the home node. The player reads it over a local URL
/// that the Rust side serves from the node, so seeking works (ADR 0008).
class PlayerScreen extends StatefulWidget {
  const PlayerScreen({
    super.key,
    required this.node,
    required this.post,
    required this.media,
  });

  final HomeNode node;
  final Post post;
  final Media media;

  @override
  State<PlayerScreen> createState() => _PlayerScreenState();
}

class _PlayerScreenState extends State<PlayerScreen> {
  VideoPlayerController? _controller;
  String _stage = 'Loading…';
  String? _error;

  @override
  void initState() {
    super.initState();
    _start();
  }

  @override
  void dispose() {
    _controller?.dispose();
    super.dispose();
  }

  Future<void> _start() async {
    final media = widget.media;
    try {
      if (widget.node.onMobileData && media.size > _mobileDataWarnBytes) {
        final go = await _confirmMobileData();
        if (!go) {
          if (mounted) Navigator.of(context).pop();
          return;
        }
      }
      // Each step can take a while; stop if the screen was closed meanwhile.
      if (!media.onNode) {
        // The player would give up waiting, so let the node fetch it first.
        if (!mounted) return;
        setState(
          () =>
              _stage = 'Your home node is fetching this video from its author…',
        );
        await widget.node.app.fetchMedia(postId: widget.post.id);
      }
      if (!mounted) return;
      setState(() => _stage = 'Starting…');
      final url = await widget.node.mediaUrl(media.blob, media.mediaType);
      if (!mounted) return;
      final controller = VideoPlayerController.networkUrl(Uri.parse(url));
      _controller = controller;
      await controller.initialize();
      if (!mounted) return;
      controller.addListener(() {
        if (mounted) setState(() {});
      });
      await controller.play();
      if (mounted) setState(() {});
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    }
  }

  Future<bool> _confirmMobileData() async {
    final go = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Play on mobile data?'),
        content: Text(
          "This video is ${formatBytes(widget.media.size)}. You're on mobile data.",
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Not now'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Play'),
          ),
        ],
      ),
    );
    return go ?? false;
  }

  @override
  Widget build(BuildContext context) {
    final controller = _controller;
    final ready = controller != null && controller.value.isInitialized;
    return Scaffold(
      backgroundColor: Colors.black,
      appBar: AppBar(
        backgroundColor: Colors.black,
        foregroundColor: Colors.white,
        title: Text(widget.post.author),
      ),
      body: Center(
        child: _error != null
            ? Padding(
                padding: const EdgeInsets.all(24),
                child: Text(
                  "Can't play this video: $_error",
                  style: const TextStyle(color: Colors.white),
                ),
              )
            : !ready
            ? Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const CircularProgressIndicator(),
                  const SizedBox(height: 16),
                  Text(
                    _stage,
                    style: const TextStyle(color: Colors.white),
                    textAlign: TextAlign.center,
                  ),
                ],
              )
            : Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Flexible(
                    child: AspectRatio(
                      aspectRatio: controller.value.aspectRatio,
                      child: GestureDetector(
                        onTap: () => controller.value.isPlaying
                            ? controller.pause()
                            : controller.play(),
                        child: Stack(
                          alignment: Alignment.center,
                          children: [
                            VideoPlayer(controller),
                            if (!controller.value.isPlaying)
                              const Icon(
                                Icons.play_circle_outline,
                                color: Colors.white70,
                                size: 72,
                              ),
                            if (controller.value.isBuffering)
                              const CircularProgressIndicator(),
                          ],
                        ),
                      ),
                    ),
                  ),
                  VideoProgressIndicator(
                    controller,
                    allowScrubbing: true,
                    padding: const EdgeInsets.all(12),
                  ),
                  Text(
                    '${formatDuration(controller.value.position.inMilliseconds)} / '
                    '${formatDuration(controller.value.duration.inMilliseconds)}',
                    style: const TextStyle(color: Colors.white70),
                  ),
                  const SizedBox(height: 16),
                ],
              ),
      ),
    );
  }
}
