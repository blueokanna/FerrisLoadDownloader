import 'package:flutter/material.dart';

class AppThemeProfile {
  const AppThemeProfile({
    required this.id,
    required this.name,
    required this.description,
    required this.seed,
    required this.accent,
    required this.canvasLight,
    required this.canvasDark,
  });

  final String id;
  final String name;
  final String description;
  final Color seed;
  final Color accent;
  final Color canvasLight;
  final Color canvasDark;
}

const appThemeProfiles = <AppThemeProfile>[
  AppThemeProfile(
    id: 'cloud_dancer',
    name: 'Cloud Dancer',
    description: 'Pantone 11-4201 inspired calm white atmosphere',
    seed: Color(0xFFD7DDD9),
    accent: Color(0xFF7FA8B2),
    canvasLight: Color(0xFFF4F1EB),
    canvasDark: Color(0xFF14191C),
  ),
  AppThemeProfile(
    id: 'sea_glass',
    name: 'Sea Glass',
    description: 'Cool aqua depth for monitoring and analysis',
    seed: Color(0xFF4C8F8A),
    accent: Color(0xFFB7E0D8),
    canvasLight: Color(0xFFEAF5F2),
    canvasDark: Color(0xFF101818),
  ),
  AppThemeProfile(
    id: 'oxide',
    name: 'Oxide Ember',
    description: 'Warm clay contrast for active downloads',
    seed: Color(0xFFA44A3F),
    accent: Color(0xFFF0B8A9),
    canvasLight: Color(0xFFF9ECE7),
    canvasDark: Color(0xFF1C1414),
  ),
  AppThemeProfile(
    id: 'forest_signal',
    name: 'Forest Signal',
    description: 'Deep mineral greens with soft surveillance glow',
    seed: Color(0xFF2E6A57),
    accent: Color(0xFFB8DDC1),
    canvasLight: Color(0xFFEEF5F0),
    canvasDark: Color(0xFF101614),
  ),
];

ThemeData buildAppTheme(AppThemeProfile profile, Brightness brightness) {
  final scheme = ColorScheme.fromSeed(
    seedColor: profile.seed,
    brightness: brightness,
    dynamicSchemeVariant: DynamicSchemeVariant.fidelity,
  );
  final canvas = brightness == Brightness.light
      ? profile.canvasLight
      : profile.canvasDark;

  return ThemeData(
    useMaterial3: true,
    colorScheme: scheme.copyWith(surface: canvas),
    scaffoldBackgroundColor: canvas,
    canvasColor: canvas,
    appBarTheme: AppBarTheme(
      backgroundColor: canvas,
      foregroundColor: scheme.onSurface,
      scrolledUnderElevation: 0,
      elevation: 0,
      centerTitle: false,
      titleTextStyle: TextStyle(
        color: scheme.onSurface,
        fontSize: 20,
        fontWeight: FontWeight.w700,
        letterSpacing: 0.1,
      ),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      isDense: true,
      fillColor: scheme.surfaceContainerHighest.withValues(alpha: 0.45),
      border: OutlineInputBorder(borderRadius: BorderRadius.circular(18)),
      enabledBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(18),
        borderSide: BorderSide(color: scheme.outlineVariant),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: BorderRadius.circular(18),
        borderSide: BorderSide(color: scheme.primary, width: 1.4),
      ),
    ),
    cardTheme: CardThemeData(
      color: scheme.surface.withValues(alpha: brightness == Brightness.light ? 0.86 : 0.9),
      margin: EdgeInsets.zero,
      elevation: 0,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(26)),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        minimumSize: const Size.fromHeight(54),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(18)),
        textStyle: const TextStyle(fontWeight: FontWeight.w700, fontSize: 15),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        minimumSize: const Size.fromHeight(52),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(18)),
      ),
    ),
    segmentedButtonTheme: SegmentedButtonThemeData(
      style: ButtonStyle(
        shape: WidgetStatePropertyAll(
          RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
        ),
      ),
    ),
    chipTheme: ChipThemeData(
      backgroundColor: scheme.surfaceContainerHigh,
      selectedColor: scheme.primaryContainer,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(999)),
      side: BorderSide.none,
      labelStyle: TextStyle(color: scheme.onSurface),
    ),
    pageTransitionsTheme: const PageTransitionsTheme(
      builders: {
        TargetPlatform.android: PredictiveBackPageTransitionsBuilder(),
        TargetPlatform.iOS: CupertinoPageTransitionsBuilder(),
      },
    ),
    dividerTheme: DividerThemeData(color: scheme.outlineVariant.withValues(alpha: 0.35)),
  );
}