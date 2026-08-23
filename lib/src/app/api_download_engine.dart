import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;
import 'package:m3u8_downloader/src/rust/api/downloader.dart';

import 'download_engine.dart';

/// Web (WASM) download engine that drives the FerrisLoad HTTP API instead of
/// the local Rust runtime.
///
/// The API server (see `Dockerfile.api` / `docker-compose.yml`) executes the
/// full native download + hardware-transcode pipeline server-side and exposes:
///   POST /inspect           -> inspect a page / playlist
///   POST /download          -> queue a download, returns { task_id }
///   GET  /status/:task_id   -> poll progress
///
/// Because browsers cannot run the Rust engine, this client translates the
/// REST responses into the same `ProgressUpdate` / `MediaInspectionResult`
/// objects the native UI consumes.
class ApiDownloadEngine implements DownloadEngine {
  ApiDownloadEngine({required String apiBaseUrl, this.apiToken = ''})
      : apiBaseUrl = _normalizeBaseUrl(apiBaseUrl);

  final String apiBaseUrl;

  /// Optional bearer token forwarded to the API server (`FERRISLOAD_API_TOKEN`).
  final String apiToken;

  static const _pollInterval = Duration(seconds: 1);

  /// Upper bound for a single queued download. Long transcodes (CPU) can take
  /// hours; this only fires when the server stops making progress forever.
  static const _maxPollDuration = Duration(hours: 8);

  String? _lastOutputPath;

  /// Path on the API server where the finished file was written.
  @override
  String? get lastResultPath => _lastOutputPath;

  Map<String, String> _authHeaders() => {
        'content-type': 'application/json',
        if (apiToken.isNotEmpty) 'authorization': 'Bearer $apiToken',
      };

  /// Strip a trailing slash and reject entirely-empty values.
  static String _normalizeBaseUrl(String raw) {
    final trimmed = raw.trim();
    if (trimmed.isEmpty) {
      return defaultApiBaseUrl;
    }
    final uri = Uri.tryParse(trimmed);
    if (uri == null || !uri.hasScheme) {
      return defaultApiBaseUrl;
    }
    return uri.toString().replaceFirst(RegExp(r'/$'), '');
  }

  /// The engine forwards the user's authorized headers/cookies to the API
  /// server, so credentials must never travel in plaintext: a non-loopback
  /// URL that is not `https` is refused. Loopback (localhost) HTTP is allowed
  /// because traffic never leaves the machine.
  void _assertTransportSecurity() {
    final uri = Uri.tryParse(apiBaseUrl);
    if (uri == null || !uri.hasScheme) {
      throw ApiEngineException(
        'Invalid FerrisLoad API base URL: $apiBaseUrl',
      );
    }
    final host = uri.host;
    final isLoopback = host.isEmpty ||
        host == 'localhost' ||
        host == '127.0.0.1' ||
        host == '::1';
    if (!isLoopback && uri.scheme != 'https') {
      throw ApiEngineException(
        'FerrisLoad API base URL must use https for non-local servers '
        '(got $apiBaseUrl); refusing to send your session over plaintext HTTP.',
      );
    }
  }

  Uri _uri(String path) => Uri.parse('$apiBaseUrl$path');

  @override
  Future<void> init() async {}

  @override
  bool get writesLocalFiles => false;

  Future<Map<String, dynamic>> _postJson(
    String path,
    Map<String, dynamic> body,
  ) async {
    _assertTransportSecurity();
    final response = await http
        .post(
          _uri(path),
          headers: _authHeaders(),
          body: jsonEncode(body),
        )
        .timeout(const Duration(seconds: 60));
    final payload = _decodeResponse(response, path);
    return payload;
  }

  Future<Map<String, dynamic>> _getJson(String path) async {
    _assertTransportSecurity();
    final response = await http
        .get(_uri(path), headers: _authHeaders())
        .timeout(const Duration(seconds: 30));
    return _decodeResponse(response, path);
  }

  Map<String, dynamic> _decodeResponse(http.Response response, String path) {
    final decoded = jsonDecode(utf8.decode(response.bodyBytes));
    if (decoded is! Map<String, dynamic>) {
      throw FormatException('Unexpected response from $path');
    }
    if (response.statusCode >= 400) {
      final message = decoded['error'] ??
          'API request failed (HTTP ${response.statusCode})';
      throw ApiEngineException('$message');
    }
    return decoded;
  }

  @override
  Future<MediaInspectionResult> inspect({
    required String url,
    required RequestContext requestContext,
  }) async {
    final payload = await _postJson('/inspect', {
      'url': url,
      'request_context': _contextToJson(requestContext),
    });
    return MediaInspectionResult(
      pageUrl: payload['page_url'] as String? ?? '',
      pageTitle: payload['page_title'] as String? ?? '',
      extractor: payload['extractor'] as String? ?? '',
      candidates: [
        for (final raw in _asList(payload['candidates']))
          if (raw is Map<String, dynamic>) _candidateFromJson(raw),
      ],
      warnings: _asList(payload['warnings']).whereType<String>().toList(),
      authRequired: payload['auth_required'] as bool? ?? false,
      challengeReason: payload['challenge_reason'] as String? ?? '',
    );
  }

  MediaCandidate _candidateFromJson(Map<String, dynamic> raw) {
    return MediaCandidate(
      id: raw['id'] as String? ?? '',
      title: raw['title'] as String? ?? '',
      extractor: raw['extractor'] as String? ?? '',
      pageUrl: raw['page_url'] as String? ?? '',
      mediaUrl: raw['media_url'] as String? ?? '',
      audioUrl: raw['audio_url'] as String?,
      container: raw['container'] as String? ?? '',
      protocol: raw['protocol'] as String? ?? '',
      mimeType: raw['mime_type'] as String? ?? '',
      qualityLabel: raw['quality_label'] as String? ?? '',
      width: (raw['width'] as num?)?.toInt() ?? 0,
      height: (raw['height'] as num?)?.toInt() ?? 0,
      requiresFfmpeg: raw['requires_ffmpeg'] as bool? ?? false,
      score: (raw['score'] as num?)?.toInt() ?? 0,
      segmentCount: (raw['segment_count'] as num?)?.toInt() ?? 0,
      durationSeconds: (raw['duration_seconds'] as num?)?.toDouble() ?? 0,
      primary: raw['primary'] as bool? ?? false,
      reason: raw['reason'] as String? ?? '',
    );
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
  }) async* {
    final accepted = await _postJson('/download', {
      'url': pageUrl,
      'media_url': mediaUrl,
      'audio_url': audioUrl,
      'output_filename': _basename(output),
      'concurrency': concurrency,
      'retries': retries,
      'video_bitrate': videoBitrate,
      'audio_bitrate': audioBitrate,
      'keep_temp': keepTemp,
      'request_context': _contextToJson(requestContext),
    });
    final taskId = accepted['task_id'] as String?;
    if (taskId == null || taskId.isEmpty) {
      throw ApiEngineException('API did not return a task id');
    }

    var sawProgress = false;
    final started = DateTime.now();
    while (true) {
      // Guard against a server that never progresses: fail instead of
      // polling forever.
      if (DateTime.now().difference(started) > _maxPollDuration) {
        throw ApiEngineException(
          'API server did not finish the task within '
          '${_maxPollDuration.inHours}h; check the server logs.',
        );
      }
      final status = await _getJson('/status/$taskId');
      final state = status['status'] as String? ?? 'running';
      final message = status['message'] as String? ?? '';
      final percent = (status['progress_percent'] as num?)?.toDouble();
      final error = status['error'] as String?;
      final serverOutput = status['output_path'] as String?;
      if (serverOutput != null && serverOutput.isNotEmpty) {
        _lastOutputPath = serverOutput;
      }

      switch (state) {
        case 'completed':
          yield ProgressUpdate(
            message: message.isEmpty ? 'All tasks completed' : message,
            progress: 1.0,
          );
          return;
        case 'failed':
          yield ProgressUpdate(
            message: '',
            progress: 0.0,
            error: (error == null || error.isEmpty)
                ? 'Download failed on the API server'
                : error,
          );
          return;
        default:
          final progress = (percent ?? 0.0).clamp(0.0, 100.0) / 100.0;
          if (progress > 0 || message.isNotEmpty) {
            sawProgress = true;
            yield ProgressUpdate(
              message: message.isEmpty ? 'Working...' : message,
              progress: progress,
            );
          }
          if (!sawProgress && _pollInterval.inMilliseconds > 0) {
            // Keep emitting an initial progress so the UI never stalls blank.
            yield ProgressUpdate(
                message: 'Waiting for API server...', progress: 0.0);
          }
          await Future<void>.delayed(_pollInterval);
      }
    }
  }

  static String _basename(String path) {
    final normalized = path.replaceAll('\\', '/');
    final slash = normalized.lastIndexOf('/');
    return slash >= 0 ? normalized.substring(slash + 1) : path;
  }

  static Map<String, dynamic> _contextToJson(RequestContext context) {
    return {
      'user_agent': context.userAgent,
      'referer': context.referer,
      'origin': context.origin,
      'cookie': context.cookie,
      'headers': {
        for (final entry in context.headers) entry.name: entry.value,
      },
    };
  }

  static List<dynamic> _asList(Object? value) {
    return switch (value) {
      List<dynamic> list => list,
      _ => const [],
    };
  }
}

class ApiEngineException implements Exception {
  ApiEngineException(this.message);

  final String message;

  @override
  String toString() => message;
}
