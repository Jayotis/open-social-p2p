import 'dart:math';

import 'package:flutter/material.dart';

import '../theme.dart';

/// The dark theme, for what sits on the night in either theme: the top
/// bar, and the Universe.
final ospNight = ospTheme(Brightness.dark);

final _barTheme = ospNight.copyWith(
  appBarTheme: ospNight.appBarTheme.copyWith(
    backgroundColor: Colors.transparent,
  ),
);

/// The top bar drawn as the Universe's sky (ADR 0040): navy with a few
/// faint stars, in both themes. What is in [bar] takes the dark theme, so
/// its title, buttons and fields read on the night on a light phone too.
/// The stars stand for nobody; they're the same each time for [seed].
class SkyBar extends StatelessWidget implements PreferredSizeWidget {
  const SkyBar(this.bar, {super.key, this.seed = 'sky'});

  final PreferredSizeWidget bar;
  final String seed;

  @override
  Size get preferredSize => bar.preferredSize;

  @override
  Widget build(BuildContext context) {
    return Theme(
      data: _barTheme,
      child: CustomPaint(
        painter: _BarSky(seed: seed, colors: OspColors.of(context)),
        child: bar,
      ),
    );
  }
}

class _BarSky extends CustomPainter {
  _BarSky({required this.seed, required this.colors});

  final String seed;
  final OspColors colors;

  @override
  void paint(Canvas canvas, Size size) {
    // Plain navy, no haze: a strip of someone's sky below starts on the
    // same colour, so the two join.
    canvas.drawRect(Offset.zero & size, Paint()..color = colors.sky);
    var h = 0;
    for (final c in seed.codeUnits) {
      h = (h * 31 + c) & 0x7fffffff;
    }
    final random = Random(h);
    final dust = Paint();
    // About as dense as a profile's strip, and fainter: the bar is busy.
    final count = (size.width * size.height / 900).round();
    for (var i = 0; i < count; i++) {
      final at = Offset(
        random.nextDouble() * size.width,
        random.nextDouble() * size.height,
      );
      final r = 0.4 + random.nextDouble() * 0.9;
      dust.color = colors.distantStar.withValues(
        alpha: 0.2 + random.nextDouble() * 0.35,
      );
      canvas.drawCircle(at, r, dust);
    }
  }

  @override
  bool shouldRepaint(_BarSky old) => old.seed != seed || old.colors != colors;
}
