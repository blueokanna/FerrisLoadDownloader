# FerrisLoad

A cross-platform media downloader with a Flutter UI and a Rust core (bridged via `flutter_rust_bridge`).

It focuses on HLS / M3U8: fetch the playlist, download segments concurrently, decrypt AES-128 streams when needed, merge, and remux/transcode into an MP4 saved wherever you choose. Direct MP4 / WebM / MKV / DASH links work too, and it can pull usable streams out of ordinary web pages, YouTube, and Bilibili.

> 中文版： [README.md](README.md)

## What it does

- **HLS / M3U8** — master and media playlists, automatic best-variant selection, AES-128-CBC decryption, bounded-concurrency segment downloads with backoff retries, and an automatic single-threaded fallback instead of failing on the first hiccup.
- **Direct download** — paste an `.m3u8` or a media link and hit download; no separate analyze step needed. YouTube / Bilibili page URLs are auto-resolved to a downloadable stream.
- **Optional page analysis** — list candidate streams ranked by resolution/bitrate, then pick one.
- **Remux / transcode** — FFmpeg on desktop (auto-detects NVENC / AMF / Intel QSV / VAAPI / VideoToolbox); Android uses the system `MediaCodec` hardware codecs, no FFmpeg required.
- **Multiple concurrent tasks** — while a download runs you can edit the URL or file name and start another independent task.
- **Authorized sessions** — for login-gated sites, provide Cookie / User-Agent / Referer / Origin / custom headers manually or via the built-in authorization browser. It does not bypass Cloudflare, CAPTCHAs, DRM, anti-leech signatures, or rate limits — those shouldn't be bypassed.

## Layout

```
lib/            Flutter UI (Material 3, dynamic color, i18n)
rust/           Rust core
  src/api/      download pipelines (HLS/DASH/direct), site adapters, FFmpeg, Android JNI
  src/api_server/  optional HTTP API (Docker)
  src/crypto/   self-contained AES-128-CBC and SHA-256
  src/hls.rs    self-contained HLS playlist parser (resource-bounded)
  src/net.rs    synchronous HTTP client (courierust primary; rustls/ureq fallback for hosts the bespoke TLS stack cannot handshake with)
android/        Android host: MediaCodec transcoder, MediaStore export, foreground service
```

## Getting started

```bash
flutter pub get
flutter run
```

The Rust library is compiled automatically by the `rust_builder` package (Cargokit) during the Flutter build.

Desktop transcoding needs `ffmpeg` on `PATH`; Android does not.

### Regenerating FRB bindings (only when you change the Rust API)

```bash
flutter_rust_bridge_codegen generate
```

The codegen version must match `flutter_rust_bridge` in `rust/Cargo.toml` (currently 2.13.0).

## Usage

1. Paste a link into **Source URL**.
   - An `.m3u8` or direct media link can be downloaded immediately.
   - For a web page, run **Analyze** first and pick a candidate, or download directly.
2. Set the **Output file** name and optionally a save directory.
3. Tune **Download options** (concurrency, retries, video/audio bitrate; `0` keeps the source bitrate).
4. Press **Download**. Analyze first if you want to pick from candidates.
5. The saved path is shown and recorded in **History**.

For login-gated sites: open the authorization browser in settings, sign in yourself, and import the session; analysis and downloads reuse that context.

## How Android transcoding works

There is no FFmpeg on Android; everything goes through `MediaCodec`:

- **Hardware first for both decode and encode** (codecs are scored by vendor: Qualcomm, MediaTek, HiSilicon, Samsung, …), falling back to a software encoder only when no hardware AVC encoder works.
- **Segment-by-segment HLS** — we no longer byte-concatenate TS segments (their PTS resets at every boundary, which made MediaCodec decode only the first few seconds). Each segment is fed to the decoder independently and the timeline is re-based continuously across segments — the equivalent of FFmpeg's concat demuxer. Re-basing only fires on a real discontinuity (a 500 ms backward jump), never on the ordinary intra-GOP PTS reordering of B-frame streams: rewriting those timestamps is exactly what stretched timelines and halved playback frame rates on B-frame content.
- **Stream-copy remux first** — at bitrate 0 we try a lossless remux into MP4; only fall back to a hardware re-encode when that fails or the output is truncated.
- **Self-checking output** — after conversion, `MediaExtractor` / `MediaMetadataRetriever` verify the tracks and duration exist, so a broken "first few seconds only" file never ships. Duration is checked both ways: an output much shorter than the playlist (truncation) or much longer (an encoder that stretched the timeline, e.g. 30 min becoming 60 min) is rejected and retried.
- Encoder bitrate is scaled to resolution, the real frame rate is measured for `KEY_FRAME_RATE` / `KEY_OPERATING_RATE` (passed in frames per second so the codec does not re-time the timeline), and B-frames are disabled to reduce playback stutter. Realtime priority is deliberately not used: this is an offline batch transcode, and realtime priority makes some vendor encoders re-stamp output against the wall clock, stretching the timeline.

## Docker API

The container runs the real downloader (not simulated progress). Endpoints:

- `GET /health`
- `POST /inspect` — analyze a page/direct link and return the candidate streams (used by the Web UI)
- `POST /download` — pass `url`, or specify exact streams via `media_url` / `audio_url`
- `GET /status/:task_id`
- `GET /tasks`

```bash
docker build -f Dockerfile.api -t ferrisload-api .
docker run --rm -p 3000:3000 -e DOWNLOAD_DIR=/app/downloads \
  -v $(pwd)/downloads:/app/downloads ferrisload-api
```

## Security & privacy

- Only `http/https` targets; scheme allow-lists at both the network and parsing layers (SSRF / local-file protection).
- Custom request headers are validated against CR/LF injection; cookies and other credentials live in memory only, never on disk.
- HLS keys and DASH segment URLs are forced to `http/https` as well.
- TLS verification is always on. The primary engine is `courierust`; hosts whose certificate chains or TLS 1.3 signature schemes its bespoke stack rejects (a stricter verifier than rustls on some real-world deployments) automatically fall back to a rustls-backed engine, so a verifier quirk can never block a download.
- Output file names are sanitized against path traversal.
- Download only content you own or are authorized to access.

## Validation

```bash
flutter analyze lib
cargo test --manifest-path rust/Cargo.toml
```

## License

See [LICENSE](LICENSE).
