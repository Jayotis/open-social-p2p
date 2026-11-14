/// The app's look as one set of tokens (ADR 0031 §4): colours, type and
/// spacing, light and dark, and the Universe's night sky. Screens take their
/// colours from here, so P9 refines these values rather than repainting
/// screens one by one.
library;

import 'package:flutter/material.dart';

/// Spacing steps. Paddings and gaps use these, not loose numbers.
abstract final class Space {
  static const xs = 4.0;
  static const s = 8.0;
  static const m = 16.0;
  static const l = 24.0;
  static const xl = 32.0;
}

/// The brand's base colours. Everything else is derived from them.
abstract final class Palette {
  /// Dusk blue: buttons, links, the selected place.
  static const dusk = Color(0xFF4C5FD5);

  /// Starlight: your own star, highlights on the sky.
  static const starlight = Color(0xFFF4C55B);

  /// The night sky behind the Universe, in both themes.
  static const night = Color(0xFF0A0F1E);
  static const nightHaze = Color(0xFF1A2342);
}

/// Colours the Material scheme has no slot for.
@immutable
class OspColors extends ThemeExtension<OspColors> {
  const OspColors({
    required this.sky,
    required this.skyHaze,
    required this.onSky,
    required this.onSkyQuiet,
    required this.yourStar,
    required this.distantStar,
  });

  /// The Universe's background, and the haze at its centre.
  final Color sky;
  final Color skyHaze;

  /// Text on the sky, and quieter text there.
  final Color onSky;
  final Color onSkyQuiet;

  /// Your star at the centre, and the faint ones around it.
  final Color yourStar;
  final Color distantStar;

  static const night = OspColors(
    sky: Palette.night,
    skyHaze: Palette.nightHaze,
    onSky: Color(0xFFF1F3FA),
    onSkyQuiet: Color(0xFF9BA3BF),
    yourStar: Palette.starlight,
    distantStar: Color(0xFF6F7BA6),
  );

  /// The extension in [context]'s theme, or the night sky if a test's
  /// plain theme has none.
  static OspColors of(BuildContext context) =>
      Theme.of(context).extension<OspColors>() ?? night;

  @override
  OspColors copyWith({
    Color? sky,
    Color? skyHaze,
    Color? onSky,
    Color? onSkyQuiet,
    Color? yourStar,
    Color? distantStar,
  }) => OspColors(
    sky: sky ?? this.sky,
    skyHaze: skyHaze ?? this.skyHaze,
    onSky: onSky ?? this.onSky,
    onSkyQuiet: onSkyQuiet ?? this.onSkyQuiet,
    yourStar: yourStar ?? this.yourStar,
    distantStar: distantStar ?? this.distantStar,
  );

  @override
  OspColors lerp(OspColors? other, double t) {
    if (other == null) return this;
    return OspColors(
      sky: Color.lerp(sky, other.sky, t)!,
      skyHaze: Color.lerp(skyHaze, other.skyHaze, t)!,
      onSky: Color.lerp(onSky, other.onSky, t)!,
      onSkyQuiet: Color.lerp(onSkyQuiet, other.onSkyQuiet, t)!,
      yourStar: Color.lerp(yourStar, other.yourStar, t)!,
      distantStar: Color.lerp(distantStar, other.distantStar, t)!,
    );
  }
}

ThemeData ospTheme(Brightness brightness) {
  final scheme = ColorScheme.fromSeed(
    seedColor: Palette.dusk,
    brightness: brightness,
  );
  final base = ThemeData(colorScheme: scheme, useMaterial3: true);
  final text = base.textTheme;
  return base.copyWith(
    textTheme: text.copyWith(
      headlineSmall: text.headlineSmall?.copyWith(fontWeight: FontWeight.w600),
      titleLarge: text.titleLarge?.copyWith(fontWeight: FontWeight.w600),
      titleMedium: text.titleMedium?.copyWith(fontWeight: FontWeight.w600),
    ),
    extensions: const [OspColors.night],
  );
}
