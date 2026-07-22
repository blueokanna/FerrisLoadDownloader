import 'package:flutter_test/flutter_test.dart';
import 'package:m3u8_downloader/src/home/home_page_controller.dart';

void main() {
  test('records the actual encoder and replaces it after CPU fallback', () {
    final controller = HomePageController();
    addTearDown(controller.dispose);
    final taskId = controller.beginDownloadTask(
      fileName: 'video.mp4',
      sourcePage: 'https://example.com/video.m3u8',
      status: 'Preparing',
    );

    controller.updateDownloadTask(
      taskId,
      'Selected hardware encoder: Android MediaCodec',
      0.02,
    );
    expect(controller.value.downloadTasks.single.backend, 'Android MediaCodec');
    expect(
        controller.value.downloadTasks.single.stage, HomeWorkflowStage.backend);

    controller.updateDownloadTask(
      taskId,
      'NVIDIA NVENC unavailable; retrying merge with CPU libx264',
      0.94,
    );
    expect(controller.value.downloadTasks.single.backend, 'CPU libx264');
  });

  test('labels stream copy as remuxing rather than hardware encoding', () {
    final controller = HomePageController();
    addTearDown(controller.dispose);
    final taskId = controller.beginDownloadTask(
      fileName: 'video.mp4',
      sourcePage: 'https://example.com/video.m3u8',
      status: 'Preparing',
    );

    controller.updateDownloadTask(
      taskId,
      'Selected FFmpeg stream copy (no re-encoding)',
      0.02,
    );

    expect(controller.value.downloadTasks.single.backend, 'FFmpeg stream copy');
    expect(
        controller.value.downloadTasks.single.stage, HomeWorkflowStage.backend);
  });

  test('shows yt-dlp as a real site engine and tracks its transfer stage', () {
    final controller = HomePageController();
    addTearDown(controller.dispose);
    final taskId = controller.beginDownloadTask(
      fileName: 'video.mp4',
      sourcePage: 'https://www.youtube.com/watch?v=video-id',
      status: 'Preparing',
    );

    controller.updateDownloadTask(
      taskId,
      'Selected site engine: yt-dlp',
      0.02,
    );
    expect(controller.value.downloadTasks.single.backend, 'yt-dlp');
    expect(
      controller.value.downloadTasks.single.stage,
      HomeWorkflowStage.backend,
    );

    controller.updateDownloadTask(
      taskId,
      'yt-dlp download 42.0% | 4.2MiB/s | ETA 00:12',
      0.386,
    );
    expect(
      controller.value.downloadTasks.single.stage,
      HomeWorkflowStage.transfer,
    );
    expect(controller.value.downloadTasks.single.backend, 'yt-dlp');
  });
}
