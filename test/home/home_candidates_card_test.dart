import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:m3u8_downloader/src/app/app_theme.dart';
import 'package:m3u8_downloader/src/home/home_candidates_card.dart';
import 'package:m3u8_downloader/src/rust/api/downloader.dart';

import '../widget_test_harness.dart';

void main() {
  const primaryCandidate = MediaCandidate(
    id: 'candidate-a',
    title: 'Primary Stream',
    extractor: 'bilibili',
    pageUrl: 'https://www.bilibili.com/video/BV1fixture',
    mediaUrl: 'https://cdn.example/primary-video.mp4',
    audioUrl: 'https://cdn.example/primary-audio.m4a',
    container: 'mp4',
    protocol: 'dash',
    mimeType: 'video/mp4',
    qualityLabel: '1080p · Mandarin',
    width: 1920,
    height: 1080,
    requiresFfmpeg: false,
    score: 0,
    segmentCount: 0,
    durationSeconds: 0,
    primary: true,
    reason: '',
  );

  const secondaryCandidate = MediaCandidate(
    id: 'candidate-b',
    title: 'Secondary Stream',
    extractor: 'bilibili',
    pageUrl: 'https://www.bilibili.com/video/BV1fixture',
    mediaUrl: 'https://cdn.example/secondary-video.mp4',
    audioUrl: null,
    container: 'mp4',
    protocol: 'https',
    mimeType: 'video/mp4',
    qualityLabel: '720p',
    width: 1280,
    height: 720,
    requiresFfmpeg: false,
    score: 0,
    segmentCount: 0,
    durationSeconds: 0,
    primary: false,
    reason: '',
  );

  const inspection = MediaInspectionResult(
    pageUrl: 'https://www.bilibili.com/video/BV1fixture',
    pageTitle: 'Fixture Candidate Page',
    extractor: 'bilibili',
    candidates: [primaryCandidate, secondaryCandidate],
    warnings: ['[site:test] fixture warning'],
    authRequired: false,
    challengeReason: '',
  );

  testWidgets('renders current selection summary and selection callback', (
    tester,
  ) async {
    MediaCandidate? tappedCandidate;

    await tester.pumpWidget(
      buildTestHarness(
        profile: appThemeProfiles
            .firstWhere((profile) => profile.id == 'monet_flow'),
        brightness: Brightness.light,
        child: HomeCandidatesCard(
          inspection: inspection,
          selectedCandidate: primaryCandidate,
          running: false,
          analyzing: false,
          selectionRevision: 6,
          onCandidateSelected: (candidate) {
            tappedCandidate = candidate;
          },
          onOpenAuthBrowser: () {},
        ),
      ),
    );

    await tester.pumpAndSettle();

    expect(find.text('Current selection'), findsOneWidget);
    expect(find.text('Separate audio'), findsOneWidget);
    expect(find.text('Primary Stream'), findsWidgets);

    await tester.tap(find.text('Secondary Stream'));
    await tester.pumpAndSettle();

    expect(tappedCandidate?.id, 'candidate-b');
  });
}
