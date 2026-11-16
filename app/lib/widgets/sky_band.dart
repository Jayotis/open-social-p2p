import 'dart:math';

import 'package:flutter/material.dart';

import '../theme.dart';

/// A strip of the night sky behind someone's profile: the Universe's sky,
/// a few faint stars and a glow in their star's colour at the bottom, where
/// their picture sits (ADR 0040). Decoration only: the stars stand for
/// nobody, and are the same for a person each time, from [seed].
class SkyBand extends StatelessWidget {
  const SkyBand({
    super.key,
    required this.seed,
    required this.glow,
    this.glowAt = Alignment.bottomCenter,
    this.height = 120,
  });

  final String seed;
  final Color glow;

  /// Where the glow is: under their picture.
  final Alignment glowAt;
  final double height;

  @override
  Widget build(BuildContext context) {
    return ExcludeSemantics(
      child: SizedBox(
        height: height,
        width: double.infinity,
        child: CustomPaint(
          painter: _Sky(
            seed: seed,
            glow: glow,
            at: glowAt,
            colors: OspColors.of(context),
          ),
        ),
      ),
    );
  }
}

class _Sky extends CustomPainter {
  _Sky({
    required this.seed,
    required this.glow,
    required this.at,
    required this.colors,
  });

  final String seed;
  final Color glow;
  final Alignment at;
  final OspColors colors;

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    canvas.drawRect(
      rect,
      Paint()
        ..shader = LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [colors.sky, colors.skyHaze],
        ).createShader(rect),
    );
    final bottom = at.withinRect(rect);
    canvas.drawCircle(
      bottom,
      size.height * 1.1,
      Paint()
        ..shader =
            RadialGradient(
              colors: [glow.withValues(alpha: 0.35), glow.withValues(alpha: 0)],
            ).createShader(
              Rect.fromCircle(center: bottom, radius: size.height * 1.1),
            ),
    );
    var h = 0;
    for (final c in seed.codeUnits) {
      h = (h * 31 + c) & 0x7fffffff;
    }
    final random = Random(h);
    final dust = Paint();
    for (var i = 0; i < 40; i++) {
      final at = Offset(
        random.nextDouble() * size.width,
        random.nextDouble() * size.height,
      );
      final r = 0.5 + random.nextDouble() * 1.1;
      dust.color = colors.distantStar.withValues(
        alpha: 0.3 + random.nextDouble() * 0.5,
      );
      canvas.drawCircle(at, r, dust);
    }
  }

  @override
  bool shouldRepaint(_Sky old) =>
      old.seed != seed ||
      old.glow != glow ||
      old.at != at ||
      old.colors != colors;
}
