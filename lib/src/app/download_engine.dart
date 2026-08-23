import 'dart:async';

import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:m3u8_downloader/src/rust/api/downloader.dart';

import 'api_download_engine.dart';

/// Default API server base URL used by the web build.
///
/// The web version has no local media engine: downloads are executed by the
/// FerrisLoad HTTP API (the same server the Docker image ships). Users can
/// override this value in Settings.
const String defaultApiBaseUrl = 'http://localhost:3000';

/// Contract implemented by both download backends so the UI is identical on
/// every platform:
///  - native: Rust engine bridged through flutter_rust_bridge;
///  - web:    the FerrisLoad HTTP API (Docker) driven over HTTP.
abstract class DownloadEngine {
  Future<void> init();

  Future<MediaInspectionResult> inspect({
    required String url,
    required RequestContext requestContext,
  });

  Stream<ProgressUpdate> download({
    required String pageUrl,
    required String mediaUrl,
    String? audioUrl,
    required String output,
    required int concurrency,
    required int retries,
    required int videoBitrate,
    required int audioBitrate,
    required bool keepTemp,
    required RequestContext requestContext,
  });

  /// Whether the engine writes its output into the local filesystem that the
  /// UI can verify and (on Android) export via MediaStore. The web backend
  /// delegates output handling to the API server, so this is `false` there.
  bool get writesLocalFiles;

  /// Absolute path of the finished output, when the engine knows it. Native
  /// engines leave this `null` because the UI already owns the local path;
  /// the web backend fills it with the API server's output path.
  String? get lastResultPath => null;
}

/// Native engine backed by the Rust library (`flutter_rust_bridge`).
class FrbDownloadEngine implements DownloadEngine {
  const FrbDownloadEngine();

  @override
  Future<void> init() async {}

  @override
  Future<MediaInspectionResult> inspect({
    required String url,
    required RequestContext requestContext,
  }) {
    return inspectMediaWithContext(url: url, requestContext: requestContext);
  }

  @override
  Stream<ProgressUpdate> download({
    required String pageUrl,
    required String mediaUrl,
    String? audioUrl,
    required String output,
    required int concurrency,
    required int retries,
    required int videoBitrate,
    required int audioBitrate,
    required bool keepTemp,
    required RequestContext requestContext,
  }) {
    return downloadMediaWithContext(
      pageUrl: pageUrl,
      mediaUrl: mediaUrl,
      audioUrl: audioUrl,
      output: output,
      concurrency: concurrency,
      retries: retries,
      videoBitrate: videoBitrate,
      audioBitrate: audioBitrate,
      keepTemp: keepTemp,
      requestContext: requestContext,
    );
  }

  @override
  bool get writesLocalFiles => true;

  @override
  String? get lastResultPath => null;
}

/// Build the download engine for the current platform.
///
/// [apiBaseUrl] and [apiToken] are only consulted on the web; native targets
/// always use the Rust engine.
DownloadEngine createDownloadEngine({
  String apiBaseUrl = defaultApiBaseUrl,
  String apiToken = '',
}) {
  if (kIsWeb) {
    return ApiDownloadEngine(apiBaseUrl: apiBaseUrl, apiToken: apiToken);
  }
  return const FrbDownloadEngine();
}
