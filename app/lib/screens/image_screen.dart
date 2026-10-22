import 'package:flutter/material.dart';

import '../node.dart';
import '../src/rust/api/osp.dart';

/// A photo at full size, with pinch to zoom.
class ImageScreen extends StatelessWidget {
  const ImageScreen({super.key, required this.node, required this.media});

  final HomeNode node;
  final Media media;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.black,
      appBar: AppBar(
        backgroundColor: Colors.black,
        foregroundColor: Colors.white,
      ),
      body: FutureBuilder<String>(
        future: node.mediaUrl(media.blob, media.mediaType),
        builder: (context, url) {
          if (!url.hasData) {
            return const Center(child: CircularProgressIndicator());
          }
          return InteractiveViewer(
            maxScale: 5,
            child: Center(
              child: Image.network(
                url.data!,
                loadingBuilder: (context, child, progress) => progress == null
                    ? child
                    : const Center(child: CircularProgressIndicator()),
                errorBuilder: (_, _, _) => const Text(
                  "Couldn't load this photo from your home node.",
                  style: TextStyle(color: Colors.white),
                ),
              ),
            ),
          );
        },
      ),
    );
  }
}
