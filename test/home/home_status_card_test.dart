import 'package:flutter_test/flutter_test.dart';
import 'package:m3u8_downloader/src/app/app_theme.dart';
import 'package:m3u8_downloader/src/home/home_page_controller.dart';
import 'package:m3u8_downloader/src/home/home_status_card.dart';
import 'package:m3u8_downloader/src/rust/api/downloader.dart';

import '../widget_test_harness.dart';

void main() {
  const candidate = MediaCandidate(
    id: 'candidate-1',
    title: 'Fixture Candidate',
    extractor: 'youtube',
    pageUrl: 'https://example.com/watch',
    mediaUrl: 'https://cdn.example/video.m3u8',
    audioUrl: 'https://cdn.example/audio.m4a',
    container: 'mp4',
    protocol: 'hls',
    mimeType: 'video/mp4',
    qualityLabel: '1080p',
    width: 1920,
    height: 1080,
    requiresFfmpeg: false,
    score: 0,
    segmentCount: 0,
    durationSeconds: 0,
    primary: true,
    reason: '',
  );

  const inspection = MediaInspectionResult(
    pageUrl: 'https://example.com/watch',
    pageTitle: 'Fixture Page',
    extractor: 'youtube',
    candidates: [candidate],
    warnings: [],
    authRequired: false,
    challengeReason: '',
  );

  testWidgets('renders retry download action with failed stage semantics', (
    tester,
  ) async {
    var retryCount = 0;

    await tester.pumpWidget(
      buildTestHarness(
        profile: appThemeProfiles
            .firstWhere((profile) => profile.id == 'amoled_pantone'),
        child: HomeStatusCard(
          status: 'Signed URL expired',
          error: 'Signed URL expired',
          resultPath: null,
          progress: 0.34,
          running: false,
          analyzing: false,
          selectedCandidate: candidate,
          inspection: inspection,
          stage: HomeWorkflowStage.failed,
          stageRevision: 3,
          recoveryMessage: null,
          recoveryRevision: 0,
          retryAction: HomeRetryAction.download,
          onRetry: () {
            retryCount += 1;
          },
        ),
      ),
    );

    await tester.pumpAndSettle();

    expect(find.text('Retry download'), findsOneWidget);
    expect(find.bySemanticsLabel('Needs attention. Signed URL expired'),
        findsOneWidget);

    await tester.tap(find.text('Retry download'));
    await tester.pumpAndSettle();

    expect(retryCount, 1);
  });

  testWidgets('renders recovery banner and retry affordance in amoled monet', (
    tester,
  ) async {
    await tester.pumpWidget(
      buildTestHarness(
        profile: appThemeProfiles
            .firstWhere((profile) => profile.id == 'amoled_monet'),
        child: HomeStatusCard(
          status: 'Authorized session imported',
          error: null,
          resultPath: null,
          progress: 0.16,
          running: false,
          analyzing: false,
          selectedCandidate: candidate,
          inspection: inspection,
          stage: HomeWorkflowStage.ready,
          stageRevision: 4,
          recoveryMessage: 'HTTP 403',
          recoveryRevision: 2,
          retryAction: HomeRetryAction.analyze,
          onRetry: () {},
        ),
      ),
    );

    await tester.pumpAndSettle();

    expect(find.text('Recovery in progress'), findsOneWidget);
    expect(find.text('Retry'), findsOneWidget);
    expect(find.text('Candidate ready'), findsWidgets);
  });
}
