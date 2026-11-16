/// The app's look as one set of tokens (ADR 0031 §4, ADR 0040): colours,
/// type, spacing, shapes and motion, light and dark, and the Universe's
/// night sky. Screens take their look from here, not from loose values.
///
/// The night sky is the brand: dark is the main look, with the sky's navy
/// behind every screen, starlight for what is yours and dusk blue for what
/// you can do. Light keeps the same three colours on a pale ground.
library;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

/// Spacing steps. Paddings and gaps use these, not loose numbers.
abstract final class Space {
  static const xs = 4.0;
  static const s = 8.0;
  static const m = 16.0;
  static const l = 24.0;
  static const xl = 32.0;
}

/// The app's typeface, for text drawn outside the theme (the map's names).
abstract final class Fonts {
  static const family = 'Inter';
}

/// Corner radii: small chips and media, cards, sheets and big buttons.
abstract final class Corner {
  static const s = 8.0;
  static const m = 16.0;
  static const l = 24.0;
}

/// How long things take to move. Each is zero when the phone asks for
/// reduced motion, so a screen asks [Motion.of] rather than using these.
abstract final class Motion {
  /// A tap's answer: a like filling, a chip lighting.
  static const quick = Duration(milliseconds: 150);

  /// A sheet, a card appearing, the map's lines fading.
  static const medium = Duration(milliseconds: 300);

  /// The Universe flying from one place to another.
  static const long = Duration(milliseconds: 600);

  /// Eases in and out, a little like something with weight.
  static const curve = Curves.easeInOutCubic;

  /// [d], or zero when the phone asks for reduced motion.
  static Duration of(BuildContext context, Duration d) =>
      MediaQuery.maybeDisableAnimationsOf(context) ?? false ? Duration.zero : d;
}

/// The brand's base colours. Everything else is derived from them.
abstract final class Palette {
  /// Dusk blue: buttons, links, the selected place.
  static const dusk = Color(0xFF4C5FD5);

  /// Dusk on the night: the same blue, light enough to read on navy.
  static const duskLight = Color(0xFF9AAEFF);

  /// Starlight: your own star, what is yours, highlights on the sky.
  static const starlight = Color(0xFFF4C55B);

  /// Starlight on a pale ground, dark enough to read.
  static const starlightDeep = Color(0xFF8A5E00);

  /// The night sky behind the Universe, and behind the app in dark.
  static const night = Color(0xFF0A0F1E);
  static const nightHaze = Color(0xFF1A2342);

  /// Layers of the night for cards and bars, each a little lighter.
  static const night1 = Color(0xFF111729);
  static const night2 = Color(0xFF161D33);
  static const night3 = Color(0xFF1C2440);
  static const night4 = Color(0xFF232C4C);

  /// Text on the night, and quieter text there.
  static const moon = Color(0xFFF1F3FA);
  static const moonQuiet = Color(0xFF9BA3BF);

  /// The pale ground in light, and its layers.
  static const day = Color(0xFFF7F8FC);
  static const day1 = Color(0xFFF0F2FA);
  static const day2 = Color(0xFFE9ECF7);
  static const day3 = Color(0xFFE1E5F3);
  static const ink = Color(0xFF151A2E);
  static const inkQuiet = Color(0xFF5A6280);
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
    required this.reshared,
    required this.creditedEarly,
    required this.group,
    required this.near,
    required this.followed,
    required this.kept,
    required this.yours,
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

  /// The map's lines: a public reshare, and one credited as found early.
  final Color reshared;
  final Color creditedEarly;

  /// A group of people on the map; a person near you; one you follow or
  /// know.
  final Color group;
  final Color near;
  final Color followed;

  /// Media kept available by a copy, on the map.
  final Color kept;

  /// What is yours away from the sky (your name on a card, your star's
  /// echo): starlight, dark enough to read on the ground it's on.
  final Color yours;

  /// The sky is the same in both themes; only [yours] follows the ground.
  static const night = OspColors(
    sky: Palette.night,
    skyHaze: Palette.nightHaze,
    onSky: Palette.moon,
    onSkyQuiet: Palette.moonQuiet,
    yourStar: Palette.starlight,
    distantStar: Color(0xFF6F7BA6),
    reshared: Color(0xFF7F95E8),
    creditedEarly: Color(0xFFE8C46A),
    group: Palette.duskLight,
    near: Color(0xFFE6EBFF),
    followed: Color(0xFFB9CCFF),
    kept: Color(0xFF6FD6C2),
    yours: Palette.starlight,
  );

  static final day = night.copyWith(yours: Palette.starlightDeep);

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
    Color? reshared,
    Color? creditedEarly,
    Color? group,
    Color? near,
    Color? followed,
    Color? kept,
    Color? yours,
  }) => OspColors(
    sky: sky ?? this.sky,
    skyHaze: skyHaze ?? this.skyHaze,
    onSky: onSky ?? this.onSky,
    onSkyQuiet: onSkyQuiet ?? this.onSkyQuiet,
    yourStar: yourStar ?? this.yourStar,
    distantStar: distantStar ?? this.distantStar,
    reshared: reshared ?? this.reshared,
    creditedEarly: creditedEarly ?? this.creditedEarly,
    group: group ?? this.group,
    near: near ?? this.near,
    followed: followed ?? this.followed,
    kept: kept ?? this.kept,
    yours: yours ?? this.yours,
  );

  @override
  OspColors lerp(OspColors? other, double t) {
    if (other == null) return this;
    Color l(Color a, Color b) => Color.lerp(a, b, t)!;
    return OspColors(
      sky: l(sky, other.sky),
      skyHaze: l(skyHaze, other.skyHaze),
      onSky: l(onSky, other.onSky),
      onSkyQuiet: l(onSkyQuiet, other.onSkyQuiet),
      yourStar: l(yourStar, other.yourStar),
      distantStar: l(distantStar, other.distantStar),
      reshared: l(reshared, other.reshared),
      creditedEarly: l(creditedEarly, other.creditedEarly),
      group: l(group, other.group),
      near: l(near, other.near),
      followed: l(followed, other.followed),
      kept: l(kept, other.kept),
      yours: l(yours, other.yours),
    );
  }
}

/// The colour scheme: Material's slots filled from the palette rather than
/// generated from a seed, so neither theme drifts to stock purple.
ColorScheme ospScheme(Brightness brightness) {
  final base = ColorScheme.fromSeed(
    seedColor: Palette.dusk,
    brightness: brightness,
  );
  return switch (brightness) {
    Brightness.dark => base.copyWith(
      primary: Palette.duskLight,
      onPrimary: Palette.night,
      primaryContainer: const Color(0xFF2C3880),
      onPrimaryContainer: const Color(0xFFDDE2FF),
      secondary: const Color(0xFFB9C3E8),
      onSecondary: Palette.night,
      secondaryContainer: Palette.night4,
      onSecondaryContainer: Palette.moon,
      tertiary: Palette.starlight,
      onTertiary: Palette.night,
      tertiaryContainer: const Color(0xFF4A3A10),
      onTertiaryContainer: const Color(0xFFFFE6A8),
      surface: Palette.night,
      onSurface: Palette.moon,
      onSurfaceVariant: Palette.moonQuiet,
      surfaceContainerLowest: const Color(0xFF070B16),
      surfaceContainerLow: Palette.night1,
      surfaceContainer: Palette.night2,
      surfaceContainerHigh: Palette.night3,
      surfaceContainerHighest: Palette.night4,
      surfaceTint: Colors.transparent,
      outline: const Color(0xFF4A5478),
      outlineVariant: const Color(0xFF262F50),
      inverseSurface: Palette.moon,
      onInverseSurface: Palette.night,
      inversePrimary: Palette.dusk,
    ),
    Brightness.light => base.copyWith(
      primary: Palette.dusk,
      onPrimary: Colors.white,
      primaryContainer: const Color(0xFFDDE2FF),
      onPrimaryContainer: const Color(0xFF1A2370),
      secondary: const Color(0xFF4E5776),
      onSecondary: Colors.white,
      secondaryContainer: Palette.day3,
      onSecondaryContainer: Palette.ink,
      tertiary: Palette.starlightDeep,
      onTertiary: Colors.white,
      tertiaryContainer: const Color(0xFFFFE6A8),
      onTertiaryContainer: const Color(0xFF3A2800),
      surface: Palette.day,
      onSurface: Palette.ink,
      onSurfaceVariant: Palette.inkQuiet,
      surfaceContainerLowest: Colors.white,
      surfaceContainerLow: Colors.white,
      surfaceContainer: Palette.day1,
      surfaceContainerHigh: Palette.day2,
      surfaceContainerHighest: Palette.day3,
      surfaceTint: Colors.transparent,
      outline: const Color(0xFF8C93AE),
      outlineVariant: const Color(0xFFD9DDEC),
      inverseSurface: Palette.night2,
      onInverseSurface: Palette.moon,
      inversePrimary: Palette.duskLight,
    ),
  };
}

/// Type: Inter throughout, Material's scale with tighter headings and no
/// extra letter spacing in body text, which Inter doesn't need.
TextTheme _type(TextTheme base) {
  TextStyle? head(TextStyle? s) =>
      s?.copyWith(fontWeight: FontWeight.w600, letterSpacing: -0.3);
  TextStyle? title(TextStyle? s) =>
      s?.copyWith(fontWeight: FontWeight.w600, letterSpacing: -0.1);
  TextStyle? body(TextStyle? s, double height) =>
      s?.copyWith(letterSpacing: 0, height: height);
  TextStyle? label(TextStyle? s) =>
      s?.copyWith(fontWeight: FontWeight.w500, letterSpacing: 0.1);
  return base
      .copyWith(
        displaySmall: head(base.displaySmall),
        headlineLarge: head(base.headlineLarge),
        headlineMedium: head(base.headlineMedium),
        headlineSmall: head(base.headlineSmall),
        titleLarge: title(base.titleLarge),
        titleMedium: title(base.titleMedium),
        titleSmall: title(base.titleSmall),
        bodyLarge: body(base.bodyLarge, 1.45),
        bodyMedium: body(base.bodyMedium, 1.4),
        bodySmall: body(base.bodySmall, 1.35),
        labelLarge: label(base.labelLarge),
        labelMedium: label(base.labelMedium),
        labelSmall: label(base.labelSmall),
      )
      .apply(fontFamily: Fonts.family);
}

/// Buttons: a pill on one line, as the corners are rounder than half its
/// height; on two, with large text, the corners stay clear of the words.
const buttonShape = RoundedRectangleBorder(
  borderRadius: BorderRadius.all(Radius.circular(Corner.l)),
);

ThemeData ospTheme(Brightness brightness) {
  final scheme = ospScheme(brightness);
  final dark = brightness == Brightness.dark;
  final base = ThemeData(
    colorScheme: scheme,
    useMaterial3: true,
    fontFamily: Fonts.family,
  );
  // Complete styles (sizes as well as colours), so a component's text
  // animates cleanly between this theme and another, as the Universe's
  // night does.
  final text = _type(
    Typography.material2021(
      colorScheme: scheme,
    ).englishLike.merge(base.textTheme),
  );
  final rounded = RoundedRectangleBorder(
    borderRadius: BorderRadius.circular(Corner.m),
  );
  return base.copyWith(
    textTheme: text,
    scaffoldBackgroundColor: scheme.surface,
    // The top bar: the Universe's sky in dark, the pale ground in light
    // except where it meets a sky (see SkyBar).
    appBarTheme: AppBarTheme(
      backgroundColor: dark ? Palette.night : scheme.surface,
      foregroundColor: scheme.onSurface,
      surfaceTintColor: Colors.transparent,
      scrolledUnderElevation: 0,
      centerTitle: false,
      systemOverlayStyle: dark
          ? SystemUiOverlayStyle.light
          : SystemUiOverlayStyle.dark,
      // No titleTextStyle: Material's default is titleLarge in the bar's
      // foregroundColor.
    ),
    cardTheme: CardThemeData(
      color: scheme.surfaceContainerLow,
      surfaceTintColor: Colors.transparent,
      elevation: 0,
      margin: const EdgeInsets.symmetric(
        horizontal: Space.m,
        vertical: Space.s,
      ),
      shape: rounded.copyWith(side: BorderSide(color: scheme.outlineVariant)),
      clipBehavior: Clip.antiAlias,
    ),
    navigationBarTheme: NavigationBarThemeData(
      backgroundColor: dark ? Palette.night1 : scheme.surfaceContainer,
      surfaceTintColor: Colors.transparent,
      indicatorColor: scheme.primary.withValues(alpha: dark ? 0.2 : 0.14),
      elevation: 0,
      height: 68,
      labelTextStyle: WidgetStateProperty.resolveWith(
        (states) => text.labelMedium?.copyWith(
          color: states.contains(WidgetState.selected)
              ? scheme.onSurface
              : scheme.onSurfaceVariant,
        ),
      ),
      iconTheme: WidgetStateProperty.resolveWith(
        (states) => IconThemeData(
          color: states.contains(WidgetState.selected)
              ? scheme.primary
              : scheme.onSurfaceVariant,
        ),
      ),
    ),
    floatingActionButtonTheme: FloatingActionButtonThemeData(
      backgroundColor: scheme.primary,
      foregroundColor: scheme.onPrimary,
      // Flat: on the night a shadow is a dark ring rather than depth.
      elevation: 0,
      focusElevation: 0,
      hoverElevation: 0,
      highlightElevation: 0,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(Corner.l),
      ),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        shape: buttonShape,
        textStyle: text.labelLarge,
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        shape: buttonShape,
        side: BorderSide(color: scheme.outline),
        textStyle: text.labelLarge,
      ),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(textStyle: text.labelLarge),
    ),
    chipTheme: ChipThemeData(
      backgroundColor: scheme.surfaceContainerHigh,
      side: BorderSide.none,
      shape: const StadiumBorder(),
      labelStyle: text.labelLarge?.copyWith(color: scheme.onSurface),
    ),
    listTileTheme: ListTileThemeData(
      iconColor: scheme.onSurfaceVariant,
      titleTextStyle: text.titleMedium?.copyWith(
        color: scheme.onSurface,
        fontWeight: FontWeight.w500,
      ),
      subtitleTextStyle: text.bodyMedium?.copyWith(
        color: scheme.onSurfaceVariant,
      ),
      contentPadding: const EdgeInsets.symmetric(horizontal: Space.m),
    ),
    dividerTheme: DividerThemeData(
      color: scheme.outlineVariant,
      thickness: 1,
      space: 1,
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: scheme.surfaceContainerHigh,
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(Corner.m),
        borderSide: BorderSide.none,
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(Corner.m),
        borderSide: BorderSide(color: scheme.primary, width: 1.5),
      ),
    ),
    bottomSheetTheme: BottomSheetThemeData(
      backgroundColor: scheme.surfaceContainer,
      surfaceTintColor: Colors.transparent,
      showDragHandle: true,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(Corner.l)),
      ),
    ),
    dialogTheme: DialogThemeData(
      backgroundColor: scheme.surfaceContainerHigh,
      surfaceTintColor: Colors.transparent,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(Corner.l),
      ),
    ),
    snackBarTheme: SnackBarThemeData(
      behavior: SnackBarBehavior.floating,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(Corner.s),
      ),
    ),
    badgeTheme: BadgeThemeData(
      backgroundColor: scheme.tertiary,
      textColor: scheme.onTertiary,
    ),
    progressIndicatorTheme: ProgressIndicatorThemeData(
      color: scheme.primary,
      linearTrackColor: scheme.surfaceContainerHighest,
    ),
    extensions: [dark ? OspColors.night : OspColors.day],
  );
}
