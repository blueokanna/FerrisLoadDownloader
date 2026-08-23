import 'dart:io';

import 'capability_snapshot.dart';
import 'platform_utils.dart';

// References come from the conditional import in `runtime_capabilities.dart`;
// see `platform_utils_stub.dart` for why this ignore is required.
// ignore_for_file: unused_element

Future<PlatformCapabilitySnapshot> probeDesktopCapabilities() async {
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
    platform: FerrisPlatform.operatingSystem,
    transcoderBackend: backend,
    hardwareAccelerated: accelerated,
    videoEncoders: encoders,
    videoDecoders: const [],
    ffmpegAvailable: ffmpeg != null,
    ytdlpAvailable: ytdlpAvailable,
    notes: notes,
  );
}

Future<String?> _resolveFfmpeg() async {
  final executable = FerrisPlatform.isWindows ? 'ffmpeg.exe' : 'ffmpeg';
  final appDirectory = File(Platform.resolvedExecutable).parent;
  final candidates = <String>[
    if ((Platform.environment['FERRISLOAD_FFMPEG_PATH'] ?? '').isNotEmpty)
      Platform.environment['FERRISLOAD_FFMPEG_PATH']!,
    '${appDirectory.path}${FerrisPlatform.pathSeparator}tools${FerrisPlatform.pathSeparator}$executable',
    '${appDirectory.path}${FerrisPlatform.pathSeparator}$executable',
    executable,
  ];
  for (final candidate in _unique(candidates)) {
    if (await _commandWorks(candidate, const ['-version'])) {
      return candidate;
    }
  }
  return null;
}

Future<bool> _resolveYtdlp() async {
  final executable = FerrisPlatform.isWindows ? 'yt-dlp.exe' : 'yt-dlp';
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
          '${appDirectory.path}${FerrisPlatform.pathSeparator}tools${FerrisPlatform.pathSeparator}$executable',
      prefix: const [],
    ),
    (
      program: '${appDirectory.path}${FerrisPlatform.pathSeparator}$executable',
      prefix: const [],
    ),
    if (cachePath != null) (program: cachePath, prefix: const []),
    (program: executable, prefix: const []),
    if (FerrisPlatform.isWindows)
      (program: 'py', prefix: const ['-m', 'yt_dlp']),
    (
      program: FerrisPlatform.isWindows ? 'python' : 'python3',
      prefix: const ['-m', 'yt_dlp'],
    ),
    if (!FerrisPlatform.isWindows)
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

String? _ytdlpCachePath() {
  if (FerrisPlatform.isWindows) {
    final root = Platform.environment['LOCALAPPDATA'];
    return root == null
        ? null
        : '$root${FerrisPlatform.pathSeparator}FerrisLoad${FerrisPlatform.pathSeparator}tools${FerrisPlatform.pathSeparator}yt-dlp.exe';
  }
  final home = Platform.environment['HOME'];
  if (FerrisPlatform.isMacOS && home != null) {
    return '$home/Library/Application Support/FerrisLoad/tools/yt-dlp';
  }
  final root = Platform.environment['XDG_DATA_HOME'] ??
      (home == null ? null : '$home/.local/share');
  return root == null ? null : '$root/ferrisload/tools/yt-dlp';
}

Future<_EncoderProbe?> _detectEncoder(String ffmpeg) async {
  final probes = <_EncoderProbe>[
    const _EncoderProbe('NVIDIA NVENC', 'h264_nvenc', true),
    const _EncoderProbe('AMD AMF', 'h264_amf', true),
    const _EncoderProbe('Intel Quick Sync', 'h264_qsv', true),
    if (FerrisPlatform.isLinux)
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
    if (FerrisPlatform.isMacOS)
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

Future<bool> _commandWorks(
  String executable,
  List<String> arguments,
) async {
  try {
    final result = await Process.run(executable, arguments)
        .timeout(const Duration(seconds: 8));
    return result.exitCode == 0;
  } catch (_) {
    return false;
  }
}

List<String> _unique(List<String> values) {
  final seen = <String>{};
  return [
    for (final value in values)
      if (seen.add(
        FerrisPlatform.isWindows ? value.toLowerCase() : value,
      ))
        value,
  ];
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
