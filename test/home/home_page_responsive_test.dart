import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/app/app_settings.dart';
import 'package:m3u8_downloader/src/app/app_theme.dart';
import 'package:m3u8_downloader/src/app/download_engine.dart';
import 'package:m3u8_downloader/src/home/home_page.dart';
import 'package:m3u8_downloader/src/home/home_settings_sheet.dart';

const _settings = AppSettings(
  themeProfileId: 'monet_flow',
  themeMode: ThemeMode.light,
  localeTag: 'zh',
  autoOpenAuthBrowser: true,
  apiBaseUrl: defaultApiBaseUrl,
  apiToken: '',
);

Widget _buildApp(Key captureKey) {
  return RepaintBoundary(
    key: captureKey,
    child: MaterialApp(
      debugShowCheckedModeBanner: false,
      locale: _settings.locale,
      supportedLocales: AppLocalizations.supportedLocales,
      localizationsDelegates: const [
        AppLocalizations.delegate,
        GlobalMaterialLocalizations.delegate,
        GlobalWidgetsLocalizations.delegate,
        GlobalCupertinoLocalizations.delegate,
      ],
      theme: buildAppTheme(_settings.themeProfile, Brightness.light),
      home: HomePage(settings: _settings, onSettingsChanged: (_) {}),
    ),
  );
}

Future<void> _capture(Finder finder, String fileName) async {
  if (Platform.environment['FERRIS_CAPTURE_UI'] != '1') {
    return;
  }
  await expectLater(
    finder,
    matchesGoldenFile('../../.dart_cli_state/$fileName'),
  );
}

void main() {
  for (final viewport in <(String, Size)>[
    ('mobile', const Size(390, 844)),
    ('desktop', const Size(1440, 900)),
  ]) {
    testWidgets('home page has no overflow at ${viewport.$1} size', (
      tester,
    ) async {
      tester.view.devicePixelRatio = 1;
      tester.view.physicalSize = viewport.$2;
      addTearDown(tester.view.reset);

      final captureKey = GlobalKey();
      await tester.pumpWidget(_buildApp(captureKey));
      await tester.pumpAndSettle();

      expect(tester.takeException(), isNull);
      expect(find.byType(HomePage), findsOneWidget);
      await _capture(find.byKey(captureKey), 'home_${viewport.$1}.png');
    });
  }

  testWidgets('mobile source actions and settings sheet have no overflow', (
    tester,
  ) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(390, 844);
    addTearDown(tester.view.reset);

    final captureKey = GlobalKey();
    await tester.pumpWidget(_buildApp(captureKey));
    await tester.pumpAndSettle();

    await tester.enterText(
      find.byType(TextFormField).first,
      '【哔哩哔哩】一个很长的分享标题 https://b23.tv/fixture?share_source=copy_web',
    );
    await tester.pump();
    expect(find.byIcon(Icons.close_rounded), findsOneWidget);
    expect(find.byIcon(Icons.content_paste_rounded), findsOneWidget);
    expect(tester.takeException(), isNull);

    await tester.tap(find.widgetWithIcon(IconButton, Icons.tune_rounded));
    await tester.pumpAndSettle();
    expect(find.byType(HomeSettingsSheet), findsOneWidget);
    expect(tester.takeException(), isNull);
    await _capture(find.byKey(captureKey), 'home_settings_mobile.png');
  });
}
