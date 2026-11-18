import 'package:flutter/material.dart';

import '../node.dart';
import '../theme.dart';

/// Someone's profile picture (ADR 0032), or their [InitialAvatar] when they
/// have none or it can't be fetched.
///
/// Opening a stranger's picture makes the node fetch it without saying who
/// looks, at most 200 an hour, so it's fetched only where a profile is
/// opened. Posts and lists pass a picture only when the node already holds
/// it: the owner's own, those of people they follow, and opened profiles'.
class PersonAvatar extends StatelessWidget {
  const PersonAvatar({
    super.key,
    required this.node,
    required this.name,
    this.identity,
    this.picture,
    this.radius = 20,
  });

  final HomeNode node;
  final String name;

  /// Their identity, which picks the star's colour; the name if unknown.
  final String? identity;

  /// The picture's file, from `Profile.picture`.
  final String? picture;
  final double radius;

  @override
  Widget build(BuildContext context) {
    final initial = InitialAvatar(
      name: name,
      seed: identity ?? name,
      yours: identity != null && identity == node.paired?.identity,
      radius: radius,
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

/// A person without a picture: the first letter of their name on a small
/// star, its colour the same for them everywhere, from [seed]. Your own is
/// starlight, as your star is on the map.
class InitialAvatar extends StatelessWidget {
  const InitialAvatar({
    super.key,
    required this.name,
    required this.seed,
    this.yours = false,
    this.radius = 20,
  });

  final String name;
  final String seed;
  final bool yours;
  final double radius;

  /// Star colours for others: light tints that read on night and day.
  static const tints = [
    Color(0xFF9AAEFF), // dusk
    Color(0xFF8FD3FF), // sky
    Color(0xFF6FD6C2), // teal
    Color(0xFFC3A6FF), // lavender
    Color(0xFFF2A7C3), // rose
    Color(0xFFFFB98A), // peach
  ];

  /// The tint for [seed], the same on every run (not `hashCode`, which
  /// may change between runs).
  static Color tintFor(String seed) {
    var h = 0;
    for (final c in seed.codeUnits) {
      h = (h * 31 + c) & 0x7fffffff;
    }
    return tints[h % tints.length];
  }

  @override
  Widget build(BuildContext context) {
    final tint = yours ? Palette.starlight : tintFor(seed);
    final letter = name.isEmpty ? '?' : name.characters.first.toUpperCase();
    return ExcludeSemantics(
      child: Container(
        width: radius * 2,
        height: radius * 2,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          gradient: RadialGradient(
            center: const Alignment(-0.3, -0.35),
            radius: 0.95,
            colors: [Color.lerp(tint, Colors.white, 0.45)!, tint],
          ),
        ),
        child: Text(
          letter,
          style: TextStyle(
            fontFamily: Fonts.family,
            fontSize: radius * 0.85,
            fontWeight: FontWeight.w600,
            color: Palette.night,
            height: 1,
          ),
        ),
      ),
    );
  }
}
