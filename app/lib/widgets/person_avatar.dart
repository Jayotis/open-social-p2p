import 'package:flutter/material.dart';

import '../node.dart';

/// Someone's profile picture (ADR 0032), or the first letter of their name
/// when they have none or it can't be fetched.
///
/// Opening a stranger's picture makes the node fetch it without saying who
/// looks, at most 200 an hour, so it's shown only where a profile is opened,
/// not in every list.
class PersonAvatar extends StatelessWidget {
  const PersonAvatar({
    super.key,
    required this.node,
    required this.name,
    this.picture,
    this.radius = 20,
  });

  final HomeNode node;
  final String name;

  /// The picture's file, from `Profile.picture`.
  final String? picture;
  final double radius;

  @override
  Widget build(BuildContext context) {
    final initial = CircleAvatar(
      radius: radius,
      child: Text(
        name.isEmpty ? '?' : name[0].toUpperCase(),
        style: TextStyle(fontSize: radius * 0.8),
      ),
    );
    final picture = this.picture;
    if (picture == null) return initial;
    return FutureBuilder<String>(
      future: node.mediaUrl(picture, 'image/webp'),
      builder: (context, url) {
        if (!url.hasData) return initial;
        return ClipOval(
          child: Image.network(
            url.data!,
            width: radius * 2,
            height: radius * 2,
            fit: BoxFit.cover,
            frameBuilder: (_, child, frame, sync) =>
                frame == null && !sync ? initial : child,
            errorBuilder: (_, _, _) => initial,
          ),
        );
      },
    );
  }
}
