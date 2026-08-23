import 'package:flutter/cupertino.dart' show CupertinoPageTransitionsBuilder;
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
    this.schemeVariant = DynamicSchemeVariant.tonalSpot,
    this.supportsDynamicColor = false,
    this.useAmoledSurface = false,
  });

  final String id;
  final String name;
  final String description;
  final Color seed;
  final Color accent;
  final Color canvasLight;
  final Color canvasDark;
  final DynamicSchemeVariant schemeVariant;
  final bool supportsDynamicColor;
  final bool useAmoledSurface;
}

const appThemeProfiles = <AppThemeProfile>[
  AppThemeProfile(
    id: 'monet_flow',
    name: 'Monet Flow',
    description: 'Material You dynamic color with layered tonal surfaces',
    seed: Color(0xFF5C8DFF),
    accent: Color(0xFFB6C7FF),
    canvasLight: Color(0xFFF5F6FB),
    canvasDark: Color(0xFF10131A),
    schemeVariant: DynamicSchemeVariant.tonalSpot,
    supportsDynamicColor: true,
  ),
  AppThemeProfile(
    id: 'amoled_monet',
    name: 'Amoled Monet',
    description: 'Dynamic color with true-black OLED surfaces',
    seed: Color(0xFF7C91FF),
    accent: Color(0xFFC8D1FF),
    canvasLight: Color(0xFFF4F6FC),
    canvasDark: Color(0xFF000000),
    schemeVariant: DynamicSchemeVariant.content,
    supportsDynamicColor: true,
    useAmoledSurface: true,
  ),
  AppThemeProfile(
    id: 'amoled_pantone',
    name: 'Amoled Pantone',
    description: 'Pantone-led accents over true-black AMOLED framing',
    seed: Color(0xFF0F766E),
    accent: Color(0xFFF59E7A),
    canvasLight: Color(0xFFF8F4EF),
    canvasDark: Color(0xFF000000),
    schemeVariant: DynamicSchemeVariant.fidelity,
    useAmoledSurface: true,
  ),
  AppThemeProfile(
    id: 'cloud_dancer',
    name: 'Cloud Dancer',
    description: 'Pantone 11-4201 inspired calm white atmosphere',
    seed: Color(0xFFD7DDD9),
    accent: Color(0xFF7FA8B2),
    canvasLight: Color(0xFFF4F1EB),
    canvasDark: Color(0xFF14191C),
    schemeVariant: DynamicSchemeVariant.neutral,
  ),
  AppThemeProfile(
    id: 'sea_glass',
    name: 'Sea Glass',
    description: 'Cool aqua depth for monitoring and analysis',
    seed: Color(0xFF4C8F8A),
    accent: Color(0xFFB7E0D8),
    canvasLight: Color(0xFFEAF5F2),
    canvasDark: Color(0xFF101818),
    schemeVariant: DynamicSchemeVariant.vibrant,
  ),
  AppThemeProfile(
    id: 'oxide',
    name: 'Oxide Ember',
    description: 'Warm clay contrast for active downloads',
    seed: Color(0xFFA44A3F),
    accent: Color(0xFFF0B8A9),
    canvasLight: Color(0xFFF9ECE7),
    canvasDark: Color(0xFF1C1414),
    schemeVariant: DynamicSchemeVariant.expressive,
  ),
  AppThemeProfile(
    id: 'forest_signal',
    name: 'Forest Signal',
    description: 'Deep mineral greens with soft surveillance glow',
    seed: Color(0xFF2E6A57),
    accent: Color(0xFFB8DDC1),
    canvasLight: Color(0xFFEEF5F0),
    canvasDark: Color(0xFF101614),
    schemeVariant: DynamicSchemeVariant.content,
  ),
];

ThemeData buildAppTheme(
  AppThemeProfile profile,
  Brightness brightness, {
  ColorScheme? dynamicScheme,
}) {
  final isDark = brightness == Brightness.dark;
  final generatedScheme = ColorScheme.fromSeed(
    seedColor: profile.seed,
    brightness: brightness,
    dynamicSchemeVariant: profile.schemeVariant,
  );
  final baseScheme = profile.supportsDynamicColor && dynamicScheme != null
      ? dynamicScheme
      : generatedScheme;
  final canvas = profile.useAmoledSurface && isDark
      ? const Color(0xFF000000)
      : isDark
          ? profile.canvasDark
          : profile.canvasLight;
  final surface =
      profile.useAmoledSurface && isDark ? const Color(0xFF000000) : canvas;
  final surfaceLow = profile.useAmoledSurface && isDark
      ? const Color(0xFF050505)
      : Color.alphaBlend(
          baseScheme.primary.withValues(alpha: isDark ? 0.08 : 0.04),
          canvas,
        );
  final surfaceContainer = profile.useAmoledSurface && isDark
      ? const Color(0xFF0A0A0A)
      : Color.alphaBlend(
          baseScheme.primary.withValues(alpha: isDark ? 0.14 : 0.07),
          canvas,
        );
  final surfaceHigh = profile.useAmoledSurface && isDark
      ? const Color(0xFF121212)
      : Color.alphaBlend(
          profile.accent.withValues(alpha: isDark ? 0.18 : 0.1),
          canvas,
        );
  final surfaceHighest = profile.useAmoledSurface && isDark
      ? const Color(0xFF191919)
      : Color.alphaBlend(
          baseScheme.secondary.withValues(alpha: isDark ? 0.2 : 0.12),
          canvas,
        );
  final elevatedSurface = Color.alphaBlend(
    baseScheme.primary.withValues(alpha: isDark ? 0.18 : 0.06),
    surfaceHigh,
  );
  final scheme = baseScheme.copyWith(
    surface: surface,
    surfaceDim: surfaceLow,
    surfaceBright: surfaceHigh,
    surfaceContainerLowest: surface,
    surfaceContainerLow: surfaceLow,
    surfaceContainer: surfaceContainer,
    surfaceContainerHigh: surfaceHigh,
    surfaceContainerHighest: surfaceHighest,
  );
  final baseTextTheme = ThemeData(brightness: brightness).textTheme;
  final textTheme = baseTextTheme.copyWith(
    displayLarge: baseTextTheme.displayLarge?.copyWith(letterSpacing: 0),
    displayMedium: baseTextTheme.displayMedium?.copyWith(letterSpacing: 0),
    displaySmall: baseTextTheme.displaySmall?.copyWith(letterSpacing: 0),
    headlineLarge: baseTextTheme.headlineLarge?.copyWith(letterSpacing: 0),
    headlineMedium: baseTextTheme.headlineMedium?.copyWith(letterSpacing: 0),
    headlineSmall: baseTextTheme.headlineSmall?.copyWith(letterSpacing: 0),
    titleLarge: baseTextTheme.titleLarge?.copyWith(letterSpacing: 0),
    titleMedium: baseTextTheme.titleMedium?.copyWith(letterSpacing: 0),
    titleSmall: baseTextTheme.titleSmall?.copyWith(letterSpacing: 0),
    bodyLarge: baseTextTheme.bodyLarge?.copyWith(letterSpacing: 0),
    bodyMedium: baseTextTheme.bodyMedium?.copyWith(letterSpacing: 0),
    bodySmall: baseTextTheme.bodySmall?.copyWith(letterSpacing: 0),
    labelLarge: baseTextTheme.labelLarge?.copyWith(letterSpacing: 0),
    labelMedium: baseTextTheme.labelMedium?.copyWith(letterSpacing: 0),
    labelSmall: baseTextTheme.labelSmall?.copyWith(letterSpacing: 0),
  );

  return ThemeData(
    useMaterial3: true,
    brightness: brightness,
    colorScheme: scheme,
    textTheme: textTheme,
    scaffoldBackgroundColor: canvas,
    canvasColor: canvas,
    appBarTheme: AppBarTheme(
      backgroundColor: canvas,
      foregroundColor: scheme.onSurface,
      surfaceTintColor: Colors.transparent,
      scrolledUnderElevation: 0,
      elevation: 0,
      centerTitle: false,
      titleTextStyle: TextStyle(
        color: scheme.onSurface,
        fontSize: 20,
        fontWeight: FontWeight.w700,
        letterSpacing: 0,
      ),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      isDense: true,
      fillColor: scheme.surfaceContainerHighest.withValues(alpha: 0.45),
      border: OutlineInputBorder(
        borderRadius: FerrisShapes.standard.input,
        borderSide: BorderSide.none,
      ),
      enabledBorder: OutlineInputBorder(
        borderRadius: FerrisShapes.standard.input,
        borderSide: BorderSide(color: scheme.outlineVariant),
      ),
      focusedBorder: OutlineInputBorder(
        borderRadius: FerrisShapes.standard.input,
        borderSide: BorderSide(color: scheme.primary, width: 2),
      ),
      errorBorder: OutlineInputBorder(
        borderRadius: FerrisShapes.standard.input,
        borderSide: BorderSide(color: scheme.error),
      ),
      focusedErrorBorder: OutlineInputBorder(
        borderRadius: FerrisShapes.standard.input,
        borderSide: BorderSide(color: scheme.error, width: 2),
      ),
    ),
    cardTheme: CardThemeData(
      color: scheme.surface.withValues(
        alpha: brightness == Brightness.light ? 0.86 : 0.9,
      ),
      surfaceTintColor: Colors.transparent,
      margin: EdgeInsets.zero,
      elevation: 0,
      shape: RoundedRectangleBorder(
        borderRadius: FerrisShapes.standard.card,
      ),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        minimumSize: const Size.fromHeight(54),
        animationDuration: const Duration(milliseconds: 220),
        shape: const StadiumBorder(),
        textStyle: const TextStyle(fontWeight: FontWeight.w700, fontSize: 15),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        minimumSize: const Size.fromHeight(52),
        animationDuration: const Duration(milliseconds: 220),
        shape: const StadiumBorder(),
      ),
    ),
    iconButtonTheme: IconButtonThemeData(
      style: IconButton.styleFrom(
        foregroundColor: scheme.onSurface,
        shape: const CircleBorder(),
      ),
    ),
    segmentedButtonTheme: SegmentedButtonThemeData(
      style: ButtonStyle(
        shape: WidgetStatePropertyAll(
          RoundedRectangleBorder(
            borderRadius: FerrisShapes.standard.md,
          ),
        ),
      ),
    ),
    switchTheme: SwitchThemeData(
      trackOutlineColor: WidgetStatePropertyAll(scheme.outlineVariant),
      thumbIcon: WidgetStateProperty.resolveWith((states) {
        if (states.contains(WidgetState.selected)) {
          return const Icon(Icons.check_rounded, size: 14);
        }
        return const Icon(Icons.circle, size: 12);
      }),
    ),
    snackBarTheme: SnackBarThemeData(
      behavior: SnackBarBehavior.floating,
      backgroundColor: scheme.surfaceContainerHigh,
      contentTextStyle: TextStyle(color: scheme.onSurface),
      shape: RoundedRectangleBorder(
        borderRadius: FerrisShapes.standard.md,
      ),
    ),
    bottomSheetTheme: BottomSheetThemeData(
      backgroundColor: scheme.surface,
      modalBackgroundColor: scheme.surface,
      surfaceTintColor: Colors.transparent,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(28)),
      ),
    ),
    dialogTheme: DialogThemeData(
      backgroundColor: scheme.surface,
      surfaceTintColor: Colors.transparent,
      shape: RoundedRectangleBorder(
        borderRadius: FerrisShapes.standard.xl,
      ),
    ),
    navigationBarTheme: NavigationBarThemeData(
      backgroundColor: scheme.surface,
      indicatorColor: scheme.primaryContainer,
      labelTextStyle: WidgetStatePropertyAll(
        TextStyle(color: scheme.onSurface, fontWeight: FontWeight.w700),
      ),
    ),
    progressIndicatorTheme: ProgressIndicatorThemeData(
      color: scheme.primary,
      linearTrackColor: scheme.surfaceContainerHighest,
      circularTrackColor: scheme.surfaceContainerHighest,
    ),
    chipTheme: ChipThemeData(
      backgroundColor: scheme.surfaceContainerHigh,
      selectedColor: scheme.primaryContainer,
      shape: const StadiumBorder(),
      side: BorderSide.none,
      labelStyle: TextStyle(color: scheme.onSurface),
    ),
    listTileTheme: ListTileThemeData(
      iconColor: scheme.onSurfaceVariant,
      tileColor: elevatedSurface.withValues(
        alpha: brightness == Brightness.dark ? 0.7 : 0.56,
      ),
      shape: RoundedRectangleBorder(
        borderRadius: FerrisShapes.standard.md,
      ),
    ),
    textSelectionTheme: TextSelectionThemeData(
      cursorColor: scheme.primary,
      selectionColor: scheme.primary.withValues(alpha: 0.24),
      selectionHandleColor: scheme.primary,
    ),
    pageTransitionsTheme: const PageTransitionsTheme(
      builders: {
        TargetPlatform.android: PredictiveBackPageTransitionsBuilder(),
        TargetPlatform.iOS: CupertinoPageTransitionsBuilder(),
      },
    ),
    splashFactory: InkSparkle.splashFactory,
    dividerTheme: DividerThemeData(
      color: scheme.outlineVariant.withValues(alpha: 0.35),
    ),
    extensions: [
      FerrisShapes.standard,
      FerrisSpacing.standard,
    ],
  );
}

/// Material 3 shape tokens for this app, exposed as a `ThemeExtension` so
/// every widget reads radii from the theme instead of hard-coding them.
///
/// Follows the Material 3 reference shape scale:
///   xs=4, sm=8, md=12, lg=16, xl=28.
@immutable
class FerrisShapes extends ThemeExtension<FerrisShapes> {
  const FerrisShapes({
    required this.xs,
    required this.sm,
    required this.md,
    required this.lg,
    required this.xl,
    required this.card,
    required this.input,
    required this.pill,
  });

  /// 4dp — small controls, progress rails, inline separators.
  final BorderRadius xs;

  /// 8dp — compact chips, list tiles, small surfaces.
  final BorderRadius sm;

  /// 12dp — text fields, medium surfaces, dialog internals.
  final BorderRadius md;

  /// 16dp — cards, bottom sheets, navigation drawers.
  final BorderRadius lg;

  /// 28dp — large containers, dialogs, sheets.
  final BorderRadius xl;

  /// Standard surface radius for app cards (16dp).
  final BorderRadius card;

  /// Radius for input fields / search surfaces (12dp).
  final BorderRadius input;

  /// Fully rounded (pill / stadium) shape.
  final BorderRadius pill;

  static const FerrisShapes standard = FerrisShapes(
    xs: BorderRadius.all(Radius.circular(4)),
    sm: BorderRadius.all(Radius.circular(8)),
    md: BorderRadius.all(Radius.circular(12)),
    lg: BorderRadius.all(Radius.circular(16)),
    xl: BorderRadius.all(Radius.circular(28)),
    card: BorderRadius.all(Radius.circular(16)),
    input: BorderRadius.all(Radius.circular(12)),
    pill: BorderRadius.all(Radius.circular(999)),
  );

  static FerrisShapes of(BuildContext context) =>
      Theme.of(context).extension<FerrisShapes>() ?? standard;

  @override
  FerrisShapes copyWith({
    BorderRadius? xs,
    BorderRadius? sm,
    BorderRadius? md,
    BorderRadius? lg,
    BorderRadius? xl,
    BorderRadius? card,
    BorderRadius? input,
    BorderRadius? pill,
  }) {
    return FerrisShapes(
      xs: xs ?? this.xs,
      sm: sm ?? this.sm,
      md: md ?? this.md,
      lg: lg ?? this.lg,
      xl: xl ?? this.xl,
      card: card ?? this.card,
      input: input ?? this.input,
      pill: pill ?? this.pill,
    );
  }

  @override
  FerrisShapes lerp(FerrisShapes? other, double t) {
    if (other == null) {
      return this;
    }
    return FerrisShapes(
      xs: BorderRadius.lerp(xs, other.xs, t)!,
      sm: BorderRadius.lerp(sm, other.sm, t)!,
      md: BorderRadius.lerp(md, other.md, t)!,
      lg: BorderRadius.lerp(lg, other.lg, t)!,
      xl: BorderRadius.lerp(xl, other.xl, t)!,
      card: BorderRadius.lerp(card, other.card, t)!,
      input: BorderRadius.lerp(input, other.input, t)!,
      pill: BorderRadius.lerp(pill, other.pill, t)!,
    );
  }
}

/// Consistent spacing scale so surfaces, cards and stacks align everywhere.
@immutable
class FerrisSpacing extends ThemeExtension<FerrisSpacing> {
  const FerrisSpacing({
    required this.unit,
    required this.xs,
    required this.sm,
    required this.md,
    required this.lg,
    required this.xl,
  });

  final double unit;
  final double xs;
  final double sm;
  final double md;
  final double lg;
  final double xl;

  static const FerrisSpacing standard = FerrisSpacing(
    unit: 8,
    xs: 4,
    sm: 8,
    md: 12,
    lg: 16,
    xl: 24,
  );

  static FerrisSpacing of(BuildContext context) =>
      Theme.of(context).extension<FerrisSpacing>() ?? standard;

  @override
  FerrisSpacing copyWith({
    double? unit,
    double? xs,
    double? sm,
    double? md,
    double? lg,
    double? xl,
  }) {
    return FerrisSpacing(
      unit: unit ?? this.unit,
      xs: xs ?? this.xs,
      sm: sm ?? this.sm,
      md: md ?? this.md,
      lg: lg ?? this.lg,
      xl: xl ?? this.xl,
    );
  }

  @override
  FerrisSpacing lerp(FerrisSpacing? other, double t) {
    if (other == null) {
      return this;
    }
    return FerrisSpacing(
      unit: unit + (other.unit - unit) * t,
      xs: xs + (other.xs - xs) * t,
      sm: sm + (other.sm - sm) * t,
      md: md + (other.md - md) * t,
      lg: lg + (other.lg - lg) * t,
      xl: xl + (other.xl - xl) * t,
    );
  }
}
