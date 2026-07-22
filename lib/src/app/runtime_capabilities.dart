import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';

class PlatformCapabilitySnapshot {
  const PlatformCapabilitySnapshot({
    required this.platform,
    required this.transcoderBackend,
    required this.hardwareAccelerated,
    required this.videoEncoders,
    required this.videoDecoders,
    required this.ffmpegAvailable,
    required this.ytdlpAvailable,
    required this.notes,
  });

  final String platform;
  final String transcoderBackend;
  final bool hardwareAccelerated;
  final List<String> videoEncoders;
  final List<String> videoDecoders;
  final bool ffmpegAvailable;
  final bool ytdlpAvailable;
  final List<String> notes;
}

class RuntimeCapabilityProbe {
  RuntimeCapabilityProbe._();

  static const _channel = MethodChannel('com.blue.ferrisload/media_store');
  static const _timeout = Duration(seconds: 8);

  static Future<PlatformCapabilitySnapshot> inspect() async {
    if (Platform.isAndroid) {
      return _inspectAndroid();
    }
    if (Platform.isIOS) {
      return const PlatformCapabilitySnapshot(
        platform: 'ios',
        transcoderBackend: 'Unavailable',
        hardwareAccelerated: false,
        videoEncoders: [],
        videoDecoders: [],
        ffmpegAvailable: false,
        ytdlpAvailable: false,
        notes: [
          'No native iOS MP4 transcoder is registered in this build.',
          'YouTube uses the native resolver; signature-protected formats may not be reusable.',
        ],
      );
    }
    return _inspectDesktop();
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

  static Future<PlatformCapabilitySnapshot> _inspectDesktop() async {
    final notes = <String>[];
    final ffmpeg = await _resolveFfmpeg();
    final ytdlpAvailable = await _resolveYtdlp();
    var backend = 'Unavailable';
    var accelerated = false;
    var encoders = <String>[];

    if (ffmpeg != null) {
      final encoder = await _detectEncoder(ffmpeg);
      if (encoder != null) {
        backend = 'FFmpeg / ${encoder.label}';
        accelerated = encoder.hardware;
        encoders = [encoder.label];
      } else {
        backend = 'FFmpeg / no usable H.264 encoder';
        notes.add(
            'FFmpeg was found, but every H.264 encoder runtime probe failed.');
      }
    } else {
      notes.add(
          'FFmpeg was not found in the configured, bundled, or system paths.');
    }
    if (!ytdlpAvailable) {
      notes.add(
        'yt-dlp is not installed yet; desktop downloads install the verified official binary on demand.',
      );
    }

    return PlatformCapabilitySnapshot(
      platform: Platform.operatingSystem,
      transcoderBackend: backend,
      hardwareAccelerated: accelerated,
      videoEncoders: encoders,
      videoDecoders: const [],
      ffmpegAvailable: ffmpeg != null,
      ytdlpAvailable: ytdlpAvailable,
      notes: notes,
    );
  }

  static Future<String?> _resolveFfmpeg() async {
    final executable = Platform.isWindows ? 'ffmpeg.exe' : 'ffmpeg';
    final appDirectory = File(Platform.resolvedExecutable).parent;
    final candidates = <String>[
      if ((Platform.environment['FERRISLOAD_FFMPEG_PATH'] ?? '').isNotEmpty)
        Platform.environment['FERRISLOAD_FFMPEG_PATH']!,
      '${appDirectory.path}${Platform.pathSeparator}tools${Platform.pathSeparator}$executable',
      '${appDirectory.path}${Platform.pathSeparator}$executable',
      executable,
    ];
    for (final candidate in _unique(candidates)) {
      if (await _commandWorks(candidate, const ['-version'])) {
        return candidate;
      }
    }
    return null;
  }

  static Future<bool> _resolveYtdlp() async {
    final executable = Platform.isWindows ? 'yt-dlp.exe' : 'yt-dlp';
    final appDirectory = File(Platform.resolvedExecutable).parent;
    final cachePath = _ytdlpCachePath();
    final candidates = <({String program, List<String> prefix})>[
      if ((Platform.environment['FERRISLOAD_YTDLP_PATH'] ?? '').isNotEmpty)
        (
          program: Platform.environment['FERRISLOAD_YTDLP_PATH']!,
          prefix: const [],
        ),
      (
        program:
            '${appDirectory.path}${Platform.pathSeparator}tools${Platform.pathSeparator}$executable',
        prefix: const [],
      ),
      (
        program: '${appDirectory.path}${Platform.pathSeparator}$executable',
        prefix: const [],
      ),
      if (cachePath != null) (program: cachePath, prefix: const []),
      (program: executable, prefix: const []),
      if (Platform.isWindows) (program: 'py', prefix: const ['-m', 'yt_dlp']),
      (
        program: Platform.isWindows ? 'python' : 'python3',
        prefix: const ['-m', 'yt_dlp'],
      ),
      if (!Platform.isWindows)
        (program: 'python', prefix: const ['-m', 'yt_dlp']),
    ];
    for (final candidate in candidates) {
      if (await _commandWorks(
        candidate.program,
        [...candidate.prefix, '--version'],
      )) {
        return true;
      }
    }
    return false;
  }

  static String? _ytdlpCachePath() {
    if (Platform.isWindows) {
      final root = Platform.environment['LOCALAPPDATA'];
      return root == null
          ? null
          : '$root${Platform.pathSeparator}FerrisLoad${Platform.pathSeparator}tools${Platform.pathSeparator}yt-dlp.exe';
    }
    final home = Platform.environment['HOME'];
    if (Platform.isMacOS && home != null) {
      return '$home/Library/Application Support/FerrisLoad/tools/yt-dlp';
    }
    final root = Platform.environment['XDG_DATA_HOME'] ??
        (home == null ? null : '$home/.local/share');
    return root == null ? null : '$root/ferrisload/tools/yt-dlp';
  }

  static Future<_EncoderProbe?> _detectEncoder(String ffmpeg) async {
    final probes = <_EncoderProbe>[
      const _EncoderProbe('NVIDIA NVENC', 'h264_nvenc', true),
      const _EncoderProbe('AMD AMF', 'h264_amf', true),
      const _EncoderProbe('Intel Quick Sync', 'h264_qsv', true),
      if (Platform.isLinux)
        const _EncoderProbe(
          'Linux VAAPI',
          'h264_vaapi',
          true,
          extraArguments: [
            '-vaapi_device',
            '/dev/dri/renderD128',
            '-vf',
            'format=nv12,hwupload',
          ],
        ),
      if (Platform.isMacOS)
        const _EncoderProbe('Apple VideoToolbox', 'h264_videotoolbox', true),
      const _EncoderProbe('CPU libx264', 'libx264', false),
    ];
    for (final probe in probes) {
      if (await _commandWorks(ffmpeg, [
        '-hide_banner',
        '-loglevel',
        'error',
        '-f',
        'lavfi',
        '-i',
        'color=c=black:s=64x64:r=1',
        '-frames:v',
        '1',
        '-an',
        '-c:v',
        probe.encoder,
        ...probe.extraArguments,
        '-f',
        'null',
        '-',
      ])) {
        return probe;
      }
    }
    return null;
  }

  static Future<bool> _commandWorks(
    String executable,
    List<String> arguments,
  ) async {
    try {
      final result = await Process.run(executable, arguments).timeout(_timeout);
      return result.exitCode == 0;
    } catch (_) {
      return false;
    }
  }

  static List<String> _stringList(Object? value) {
    return switch (value) {
      List<Object?> values => values.whereType<String>().toList(),
      _ => const [],
    };
  }

  static List<String> _unique(List<String> values) {
    final seen = <String>{};
    return [
      for (final value in values)
        if (seen.add(Platform.isWindows ? value.toLowerCase() : value)) value,
    ];
  }
}

class _EncoderProbe {
  const _EncoderProbe(
    this.label,
    this.encoder,
    this.hardware, {
    this.extraArguments = const [],
  });

  final String label;
  final String encoder;
  final bool hardware;
  final List<String> extraArguments;
}
