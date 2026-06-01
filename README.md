# FerrisLoad

FerrisLoad is a cross-platform media downloader built with Flutter and a Rust core (via `flutter_rust_bridge`). Its primary purpose is reliable **M3U8 / HLS** downloading: it fetches the playlist, downloads every segment concurrently, decrypts AES-128 streams when needed, merges the segments, and transcodes the result into a playable MP4 saved to a location you choose. It also recognizes direct media links (MP4/WebM/MKV/MPD) and exposed streams on generic pages, YouTube, and Bilibili.

## Highlights

- **M3U8 / HLS first.** Master and media playlists, AES-128-CBC decryption, automatic best-variant selection, concurrent segment download with retry/backoff, and segment merge.
- **Direct download without analysis.** Paste an `.m3u8` (or direct media) link and press Download — no analyze step required. The Rust core detects HLS automatically and runs the full pipeline.
- **Optional page analysis.** Inspect a web page to detect and rank candidate streams (generic pages, YouTube, Bilibili), then download the selected core stream.
- **Transcoding backends.** FFmpeg on desktop (with NVENC/AMF/CPU detection) and Android `MediaCodec` hardware transcoding on devices without FFmpeg.
- **Saves to your device.** On Android the file is exported to your chosen folder (SAF) or to the system `Downloads/Movies` collection via `MediaStore`. On desktop it is written to the chosen directory.
- **Authorized session support.** Import your own Cookie / User-Agent / Referer / Origin / custom headers (manually or through the in-app authorization browser) for sites that require login. FerrisLoad never bypasses Cloudflare, CAPTCHA, DRM, signatures, bans, or preview limits.
- **Foreground download service** on Android with live progress notification and battery-optimization handling.
- **24 UI languages** with English fallback.

## Project layout

```
lib/                     Flutter app (UI, state, platform channels)
  main.dart              Main screen, download/analyze flow, Android export
  src/app/               Localization, theme, authorization browser
  src/rust/              Generated flutter_rust_bridge bindings
rust/                    Rust core
  src/api/downloader.rs  HLS pipeline, extractors, transcoding, JNI
  src/bin/               Optional standalone API server
android/                 Android host, MediaStore/SAF helpers, foreground service
ios/ macos/ windows/ linux/ web/   Platform runners
```

## Requirements

- Flutter SDK `>=3.3.0 <4.0.0`
- Rust toolchain (stable) with the targets for your platform
- Desktop transcoding: `ffmpeg` available on `PATH`
- Android transcoding: no FFmpeg needed; the bundled Rust `.so` registers a `MediaCodec` transcoder through `JNI_OnLoad`

## Getting started

```bash
flutter pub get
```

Run on a connected device or desktop:

```bash
flutter run
```

The Rust library is built automatically by the `rust_builder` package (Cargokit) during the Flutter build.

### Regenerating bridge bindings (only if you change the Rust API)

```bash
flutter_rust_bridge_codegen generate
```

## Building

Android (recommended per-ABI for the Rust core):

```bash
flutter build apk --release --target-platform=android-arm64
```

Other platforms:

```bash
flutter build windows --release
flutter build macos --release
flutter build linux --release
flutter build ios --release
```

## Usage

1. Paste a source link into **Source URL**.
   - An `.m3u8` or direct media link can be downloaded immediately.
   - A web page (generic, YouTube, Bilibili) can be analyzed first to detect streams.
2. Set the **Output file** name and, optionally, a **Save location**.
3. Adjust **Download options** (concurrency, retries, video/audio bitrate). Bitrate `0` keeps the source bitrate.
4. Press **Download** to fetch directly, or **Analyze source** first and then download the selected core stream.
5. When finished, the saved file path is shown and added to **History**.

For sites that require login or human verification, open the authorization browser, complete it yourself, and import the session; FerrisLoad reuses that authorized context for analysis and download.

## How M3U8 download works

1. Download and parse the playlist. For a master playlist the highest-resolution / highest-bandwidth variant is selected.
2. If the stream is AES-128 encrypted, the key and IV are fetched and each segment is decrypted (AES-128-CBC, PKCS7).
3. Segments are downloaded concurrently with bounded concurrency and exponential backoff retries, then merged into a single `.ts` file in a writable temporary directory.
4. The `.ts` is transcoded to `.mp4` (FFmpeg or Android `MediaCodec`). The output is validated to ensure a non-trivial file was produced.
5. On Android the result is exported to your chosen folder or the system media collection; the temporary file is removed.

## Validation

```bash
flutter analyze lib
cargo check --manifest-path rust/Cargo.toml
```

## Responsible use

FerrisLoad is intended for content you own or are authorized to download. It does not circumvent access controls, DRM, paywalls, rate limits, or anti-bot challenges. Respect each site's terms of service and applicable law.

## License

See the repository for license details.
