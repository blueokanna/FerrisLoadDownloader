import 'dart:async';

import 'package:flutter/services.dart';

import 'capability_snapshot.dart';
import 'platform_utils.dart';
import 'runtime_capabilities_stub.dart'
    if (dart.library.io) 'runtime_capabilities_io.dart' as probe;

export 'capability_snapshot.dart' show PlatformCapabilitySnapshot;

class RuntimeCapabilityProbe {
  RuntimeCapabilityProbe._();

  static const _channel = MethodChannel('com.blue.ferrisload/media_store');

  static Future<PlatformCapabilitySnapshot> inspect() async {
    if (FerrisPlatform.isWeb) {
      return _inspectWeb();
    }
    if (FerrisPlatform.isAndroid) {
      return _inspectAndroid();
    }
    if (FerrisPlatform.isIOS) {
      return _inspectIos();
    }
    return probe.probeDesktopCapabilities();
  }

  /// The web build has no local media engine: it drives downloads through the
  /// FerrisLoad HTTP API (the same server that ships in the Docker image), so
  /// hardware transcode is performed by the API container, not the browser.
  static PlatformCapabilitySnapshot _inspectWeb() {
    return const PlatformCapabilitySnapshot(
      platform: 'web',
      transcoderBackend: 'API server (Docker)',
      hardwareAccelerated: true,
      videoEncoders: ['server-side FFmpeg / MediaCodec'],
      videoDecoders: ['server-side'],
      ffmpegAvailable: true,
      ytdlpAvailable: true,
      notes: [
        'The web build drives downloads through the FerrisLoad API server.',
        'Run the API server locally (docker compose up) and set its URL in Settings.',
      ],
    );
  }

  static Future<PlatformCapabilitySnapshot> _inspectAndroid() async {
    try {
      final raw = await _channel.invokeMapMethod<String, dynamic>(
        'getRuntimeCapabilities',
      );
      if (raw == null) {
        throw const FormatException('Android returned an empty report');
      }
      return PlatformCapabilitySnapshot(
        platform: raw['platform'] as String? ?? 'android',
        transcoderBackend: raw['transcoderBackend'] as String? ?? 'Unavailable',
        hardwareAccelerated: raw['hardwareAccelerated'] as bool? ?? false,
        videoEncoders: _stringList(raw['videoEncoders']),
        videoDecoders: _stringList(raw['videoDecoders']),
        ffmpegAvailable: raw['ffmpegAvailable'] as bool? ?? false,
        ytdlpAvailable: raw['ytdlpAvailable'] as bool? ?? false,
        notes: const [
          'Android uses native MediaCodec and MediaMuxer; unavailable codecs are never simulated.',
          'YouTube uses the native resolver; signature-protected formats may require desktop yt-dlp.',
        ],
      );
    } catch (error) {
      return PlatformCapabilitySnapshot(
        platform: 'android',
        transcoderBackend: 'Capability probe failed',
        hardwareAccelerated: false,
        videoEncoders: const [],
        videoDecoders: const [],
        ffmpegAvailable: false,
        ytdlpAvailable: false,
        notes: ['MediaCodec capability query failed: $error'],
      );
    }
  }

  static Future<PlatformCapabilitySnapshot> _inspectIos() async {
    try {
      final raw = await _channel.invokeMapMethod<String, dynamic>(
        'getRuntimeCapabilities',
      );
      if (raw == null) {
        throw const FormatException('iOS returned an empty report');
      }
      return PlatformCapabilitySnapshot(
        platform: 'ios',
        transcoderBackend: raw['transcoderBackend'] as String? ?? 'Unavailable',
        hardwareAccelerated: raw['hardwareAccelerated'] as bool? ?? false,
        videoEncoders: _stringList(raw['videoEncoders']),
        videoDecoders: _stringList(raw['videoDecoders']),
        ffmpegAvailable: raw['ffmpegAvailable'] as bool? ?? false,
        ytdlpAvailable: raw['ytdlpAvailable'] as bool? ?? false,
        notes: const [
          'iOS uses AVFoundation / VideoToolbox; the hardware H.264 encoder is used on A-series and M-series chips.',
          'YouTube uses the native resolver; signature-protected formats may require desktop yt-dlp.',
        ],
      );
    } catch (error) {
      return PlatformCapabilitySnapshot(
        platform: 'ios',
        transcoderBackend: 'Unavailable',
        hardwareAccelerated: false,
        videoEncoders: const [],
        videoDecoders: const [],
        ffmpegAvailable: false,
        ytdlpAvailable: false,
        notes: [
          'Native iOS transcoder is registered, but the capability query failed: $error',
        ],
      );
    }
  }

  static List<String> _stringList(Object? value) {
    return switch (value) {
      List<Object?> values => values.whereType<String>().toList(),
      _ => const [],
    };
  }
}
