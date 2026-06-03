import 'package:flutter/material.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'app_theme.dart';

class AppSettings {
  const AppSettings({
    required this.themeProfileId,
    required this.themeMode,
    required this.localeTag,
    required this.autoOpenAuthBrowser,
  });

  final String themeProfileId;
  final ThemeMode themeMode;
  final String localeTag;
  final bool autoOpenAuthBrowser;

  Locale get locale => parseLocaleTag(localeTag);

  AppThemeProfile get themeProfile {
    return appThemeProfiles.firstWhere(
      (profile) => profile.id == themeProfileId,
      orElse: () => appThemeProfiles.first,
    );
  }

  AppSettings copyWith({
    String? themeProfileId,
    ThemeMode? themeMode,
    String? localeTag,
    bool? autoOpenAuthBrowser,
  }) {
    return AppSettings(
      themeProfileId: themeProfileId ?? this.themeProfileId,
      themeMode: themeMode ?? this.themeMode,
      localeTag: localeTag ?? this.localeTag,
      autoOpenAuthBrowser: autoOpenAuthBrowser ?? this.autoOpenAuthBrowser,
    );
  }

  static Locale parseLocaleTag(String tag) {
    final normalized = tag.trim();
    if (normalized.isEmpty) {
      return const Locale('zh');
    }

    final parts = normalized.split(RegExp(r'[-_]'));
    if (parts.length >= 3) {
      return Locale.fromSubtags(
        languageCode: parts[0],
        scriptCode: parts[1],
        countryCode: parts[2].toUpperCase(),
      );
    }
    if (parts.length == 2) {
      final second = parts[1];
      final isScript = second.length == 4;
      return Locale.fromSubtags(
        languageCode: parts[0],
        scriptCode: isScript ? second : null,
        countryCode: isScript ? null : second.toUpperCase(),
      );
    }
    return Locale(parts[0]);
  }

  static String serializeLocale(Locale locale) {
    final parts = <String>[locale.languageCode];
    if (locale.scriptCode?.isNotEmpty ?? false) {
      parts.add(locale.scriptCode!);
    }
    if (locale.countryCode?.isNotEmpty ?? false) {
      parts.add(locale.countryCode!);
    }
    return parts.join('_');
  }

  static Future<AppSettings> load() async {
    final prefs = await SharedPreferences.getInstance();
    final modeName = prefs.getString('themeMode') ?? ThemeMode.system.name;
    final themeProfileId =
        prefs.getString('themeProfileId') ?? appThemeProfiles.first.id;
    final localeTag = prefs.getString('localeTag') ?? 'zh';
    final autoOpenAuthBrowser = prefs.getBool('autoOpenAuthBrowser') ?? true;
    return AppSettings(
      themeProfileId: themeProfileId,
      themeMode: ThemeMode.values.firstWhere(
        (item) => item.name == modeName,
        orElse: () => ThemeMode.system,
      ),
      localeTag: localeTag,
      autoOpenAuthBrowser: autoOpenAuthBrowser,
    );
  }

  Future<void> save() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString('themeMode', themeMode.name);
    await prefs.setString('themeProfileId', themeProfileId);
    await prefs.setString('localeTag', localeTag);
    await prefs.setBool('autoOpenAuthBrowser', autoOpenAuthBrowser);
  }
}
