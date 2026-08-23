import 'capability_snapshot.dart';

/// Web (WASM) desktop-capability probe stub.
///
/// The web build never runs desktop probing: `RuntimeCapabilityProbe.inspect`
/// short-circuits to a web-specific snapshot before calling this. The stub
/// only exists so the conditional import in `runtime_capabilities.dart`
/// compiles on `dart.library.js_interop` targets.
//
// References come from the conditional import in `runtime_capabilities.dart`;
// see `platform_utils_stub.dart` for why this ignore is required.
// ignore_for_file: unused_element
Future<PlatformCapabilitySnapshot> probeDesktopCapabilities() async {
  return const PlatformCapabilitySnapshot(
    platform: 'web',
    transcoderBackend: 'Unavailable',
    hardwareAccelerated: false,
    videoEncoders: [],
    videoDecoders: [],
    ffmpegAvailable: false,
    ytdlpAvailable: false,
    notes: ['Desktop probing is not available on the web.'],
  );
}
