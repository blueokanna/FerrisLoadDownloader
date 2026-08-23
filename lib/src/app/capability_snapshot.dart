/// Snapshot of the media capabilities available on the current platform.
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
