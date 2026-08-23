//
//  VideoToolboxBridge.h
//  Runner
//
//  C interface between the Rust download engine and the native iOS
//  AVFoundation / VideoToolbox transcoder. AVAssetWriter automatically uses
//  the hardware H.264 encoder (VideoToolbox) on every Apple device — A-series
//  iPhones/iPads and M-series Macs — which is what "call the hardware codec"
//  means on iOS.
//
//  These functions are declared `extern "C"` on the Rust side
//  (`rust/src/api/downloader.rs`, gated by `#[cfg(target_os = "ios")]`) and
//  must stay in sync with that declaration.
//

#ifndef VideoToolboxBridge_h
#define VideoToolboxBridge_h

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/// Returns 1 when the native H.264 hardware encoder pipeline is usable,
/// 0 otherwise. Cheap; called once per download to select the backend.
int ferrisload_videotoolbox_available(void);

/// Transcode a single input file (MPEG-TS, MP4, ...) into an MP4 container.
/// The video track is re-encoded with the hardware H.264 encoder.
///
/// - `video_bitrate`: target video bitrate in kbps; 0 = keep source bitrate
///   (a sane default is derived from the resolution).
/// - `audio_bitrate`: target audio bitrate in kbps; 0 = passthrough the
///   original audio (must be AAC to mux into MP4, otherwise it is encoded).
/// - `expected_ms`: expected output duration in milliseconds; when > 0 the
///   produced file is checked for truncation and fails if too short.
/// - On failure returns 0 and writes a UTF-8 error string into `errbuf`.
int ferrisload_videotoolbox_transcode(const char *input,
                                      const char *output,
                                      int video_bitrate,
                                      int audio_bitrate,
                                      long long expected_ms,
                                      char *errbuf,
                                      size_t errbuf_len);

/// Merge a separate video file and audio file into one MP4, re-encoding the
/// video with the hardware H.264 encoder and muxing the audio track.
int ferrisload_videotoolbox_mux(const char *video,
                                const char *audio,
                                const char *output,
                                long long expected_ms,
                                char *errbuf,
                                size_t errbuf_len);

#ifdef __cplusplus
}
#endif

#endif /* VideoToolboxBridge_h */
