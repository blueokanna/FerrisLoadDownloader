# FerrisLoad

FerrisLoad is a cross-platform media downloader built with Flutter and a Rust core (via `flutter_rust_bridge`). Its primary purpose is reliable **M3U8 / HLS** downloading: it fetches the playlist, downloads every segment concurrently, decrypts AES-128 streams when needed, merges the segments, and transcodes the result into a playable MP4 saved to a location you choose. It also recognizes direct media links (MP4/WebM/MKV/MPD) and exposed streams on generic pages, YouTube, and Bilibili, with additional fallback handling for alternate player JSON markers, DASH MPD manifests, and exposed backup media URLs.

## Highlights

- **M3U8 / HLS first.** Master and media playlists, AES-128-CBC decryption, automatic best-variant selection, default concurrent segment download with retry/backoff, automatic single-thread fallback after a concurrent segment failure, and segment merge.
- **Direct download without analysis.** Paste an `.m3u8`, direct media link, or supported page URL and press Download — no separate analyze step is required. The Rust core now auto-resolves plain YouTube and Bilibili page URLs into the best exposed candidate before it starts the actual transfer pipeline.
- **Optional page analysis.** Inspect a web page to detect and rank candidate streams (generic pages, YouTube, Bilibili), then download the selected core stream. The extractor now routes page parsing through dedicated site-adapter modules that try multiple player JSON markers, embedded/embed-live player-response variants, short-link hosts, live-replay manifests, membership-restricted responses, Bilibili initial-state title variants, paginated bangumi episode lists, multi-page collections, multi-audio DASH tracks, MPD manifest representations, and backup URL fields before falling back.
- **Transcoding backends.** FFmpeg on desktop (with NVENC/AMF/CPU detection) and Android `MediaCodec` hardware transcoding on devices without FFmpeg.
- **Multiple active downloads.** Each Download press snapshots the current URL, file name, output directory, auth context, and transfer settings, so one video can keep downloading while you edit the form and start another.
- **Material 3 dynamic color.** Theme profiles include dynamic Monet, true-black Amoled Monet, and Pantone-led Amoled palettes with refined surface layering, stronger container contrast, and smoother theme transitions.
- **Saves to your device.** On Android the file is exported to your chosen folder (SAF) or to the system `Downloads/Movies` collection via `MediaStore`. On desktop it is written to the chosen directory.
- **Authorized session support.** Import your own Cookie / User-Agent / Referer / Origin / custom headers (manually or through the in-app authorization browser) for sites that require login. FerrisLoad never bypasses Cloudflare, CAPTCHA, DRM, signatures, bans, or preview limits.
- **Categorized inspection warnings.** Analysis warnings are tagged by scope (`auth`, `site`, `media`) so the UI can distinguish access challenges, missing player JSON, signature-protected streams, generic extraction gaps, age-gates, and YouTube/Bilibili membership-required pages.
- **Fixture-backed extractor regression tests.** Site adapters now ship fixture-based unit tests for YouTube watch/embed/live, age-gated watch/live, members-only pages, and short-link live replay variants; Bilibili standard/bangumi/collection, paginated bangumi, multi-page collection, members-only, short-link, and multi-audio variants; plus generic pages.
- **Deeper workflow feedback.** The home screen now tracks workflow stages, retry intent, recovery-after-error state, and current candidate-switch feedback through the dedicated controller/view-model layer, then renders those transitions with stronger Material 3 status animations and explicit retry affordances.
- **Widget-level UI regression tests.** Home status and candidate surfaces now ship focused Flutter widget tests alongside theme-profile regressions so interaction polish can be validated without manual tapping.
- **Foreground download service** on Android with live progress notification and battery-optimization handling. Concurrent downloads share a foreground service, while Android `MediaCodec`/`MediaMuxer` access is serialized at the JNI boundary for device stability.
- **24 UI languages** with English fallback, RTL handling, and full coverage for the in-app authorization flow.

## Project layout

```
lib/                     Flutter app (UI, state, platform channels)
  main.dart              App bootstrap and top-level Material 3 shell
  src/app/
    app_localizations.dart
    app_settings.dart    Persistent theme/language/app behavior settings
    app_theme.dart       Material 3 Monet/Amoled/Pantone theme profiles
    auth_browser_page.dart
    platform_bridge.dart Android MediaStore / foreground-service bridge
  src/home/
    home_page.dart       Home screen state, layout orchestration, analyze/download flow
    home_page_controller.dart  HomePage view-model, workflow stage machine, and state transition controller
    auth_context_editor.dart
    home_candidates_card.dart
    home_download_tasks_card.dart Active download queue and per-task progress
    home_history_card.dart
    home_input_card.dart
    home_status_card.dart   Status timeline, retry actions, and recovery feedback
    home_widgets.dart    Reusable home screen cards, badges, motion primitives, preview widgets
    home_settings_sheet.dart
  src/rust/              Generated flutter_rust_bridge bindings
rust/                    Rust core
  src/api/downloader.rs  HLS pipeline, extractors, transcoding, JNI
  src/api/site_adapters/
    mod.rs               Host dispatch and shared adapter entrypoints
    common.rs            Shared JSON/title/media parsing helpers
    youtube.rs           YouTube player-response extraction variants
    bilibili.rs          Bilibili playinfo, DASH/progressive, membership, and multi-audio extraction
    generic.rs           Generic exposed media URL fallback extraction
    fixtures/            Stable HTML snapshots for watch/embed/live, members-only, short-link, age-gate, bangumi pagination, multi-page, and multi-audio regression tests
  src/api_server/        API server routes, task store, models, execution flow
  src/bin/               Standalone API server entrypoint
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

On the backend side, candidate ranking now probes HLS metadata concurrently instead of serially, which reduces page-analysis latency when a source exposes multiple playlists or mixed progressive/HLS/DASH candidates.

The Flutter shell now resolves locale variants against the supported language list, applies RTL direction automatically for supported right-to-left languages, animates theme switches while honoring Android 12+ dynamic color when a Monet profile is selected, and keeps the home surface fully componentized through dedicated input, candidate, active-downloads, status, history, settings, and authorization widgets. The current mobile pass also expands padding, rebalances card density, and cleans up the status and authorization surfaces so narrow screens no longer feel crowded.

`HomePage` now keeps text controllers, platform hooks, and navigation boundaries, while a dedicated controller/view-model layer owns progress, per-download task snapshots, workflow stage transitions, retry intent, selection feedback, inspection, auth-redirect, recovery-after-error state, directory, and history transitions. Shared home widgets now also provide the app-wide motion tokens and transition builders used by the input, candidate, active-downloads, status, history, and settings surfaces.

For narrow regression checks during extractor work, run `cargo test --manifest-path rust/Cargo.toml site_adapters` or `cargo test --manifest-path rust/Cargo.toml downloader::tests`. For Flutter UI/theme/motion changes, run `flutter analyze lib test` and `flutter test test/home/home_download_tasks_card_test.dart test/home/home_status_card_test.dart test/home/home_candidates_card_test.dart test/app/app_theme_test.dart`.

### Regenerating bridge bindings (only if you change the Rust API)

```bash
flutter_rust_bridge_codegen generate
```

## Building

Android (recommended per-ABI for the Rust core):

```bash
flutter build apk --release --split-per-abi
```

Other platforms:

```bash
flutter build windows --release
flutter build macos --release
flutter build linux --release
flutter build ios --release
```

## Release outputs

The GitHub Actions workflow in `.github/workflows/release.yml` produces:

- Android split APKs for `arm64-v8a`, `armeabi-v7a`, and `x86_64`
- Windows `x64` portable ZIP
- macOS `x64` ZIP and macOS `arm64` ZIP
- Linux `x64` portable tarball
- Multi-arch Docker API image on GHCR for `linux/amd64`, `linux/arm64`, and `linux/arm/v7`

If the workflow is started from a tag like `v1.2.0`, it also creates a GitHub Release and attaches the desktop/mobile artifacts automatically.

## Usage

1. Paste a source link into **Source URL**.
   - An `.m3u8` or direct media link can be downloaded immediately.
   - A web page (generic, YouTube, Bilibili) can be analyzed first to detect streams.
2. Set the **Output file** name and, optionally, a **Save location**.
3. Adjust **Download options** (concurrency, retries, video/audio bitrate). Bitrate `0` keeps the source bitrate.
4. Press **Download** to fetch directly, or **Analyze source** first and then download the selected core stream.
5. While a download is running, edit the URL or file name and press **Download** again to start another independent task.
6. When finished, the saved file path is shown and added to **History**.

For appearance tuning, open the settings sheet and switch between static Pantone palettes, dynamic Monet, Amoled Monet, and Amoled Pantone profiles. On supported Android devices, Monet profiles pull the system wallpaper palette automatically.

For sites that require login or human verification, open the authorization browser, complete it yourself, and import the session; FerrisLoad reuses that authorized context for analysis and download.

## Docker API

The API container runs the real Rust downloader, not a simulated progress loop. It can accept either:

- a direct `.m3u8` or media URL in `url`
- a page URL in `url` and it will inspect the page and pick the best detected candidate automatically
- an explicit `media_url` (and optional `audio_url`) if your caller already knows the exact streams

Run locally:

```bash
docker build -f Dockerfile.api -t ferrisload-api .
docker run --rm -p 3000:3000 -e DOWNLOAD_DIR=/app/downloads -v $(pwd)/downloads:/app/downloads ferrisload-api
```

Example request:

```bash
curl -X POST http://localhost:3000/download \
  -H 'Content-Type: application/json' \
  -d '{
    "url": "https://example.com/video/master.m3u8",
    "output_filename": "demo.mp4",
    "concurrency": 8,
    "retries": 3,
    "video_bitrate": 0,
    "audio_bitrate": 0,
    "keep_temp": false
  }'
```

The API exposes:

- `GET /health`
- `POST /download`
- `GET /status/:task_id`
- `GET /tasks`

Internally, the API server is split into dedicated modules for request models, task storage, and download orchestration. Task status updates now use a concurrent task store instead of a single async mutex, which reduces contention when multiple downloads stream progress at the same time.

## How M3U8 download works

1. Download and parse the playlist. For a master playlist the highest-resolution / highest-bandwidth variant is selected.
2. If the stream is AES-128 encrypted, the key and IV are fetched and each segment is decrypted (AES-128-CBC, PKCS7).
3. Segments are downloaded concurrently with bounded concurrency and exponential backoff retries. If the concurrent pass fails and concurrency is greater than one, temporary segment output is cleaned up and the same playlist is retried with single-threaded downloading before the failure is surfaced. Successful segment output is merged into a single `.ts` file in a writable temporary directory.
4. The `.ts` is transcoded to `.mp4` (FFmpeg or Android `MediaCodec`). The output is validated to ensure a non-trivial file was produced.
5. On Android the result is exported to your chosen folder or the system media collection; the temporary file is removed.

## Validation

```bash
flutter analyze lib
cargo check --manifest-path rust/Cargo.toml
cargo check --manifest-path rust/Cargo.toml --bin m3u8_api_server
```

## Responsible use

FerrisLoad is intended for content you own or are authorized to download. It does not circumvent access controls, DRM, paywalls, rate limits, or anti-bot challenges. Respect each site's terms of service and applicable law.

## License

See the repository for license details.
