//! Synchronous entry points for the download engine.
//!
//! These wrap the synchronous `courierust`-backed core so that
//! non-Flutter callers (the HTTP API server) can drive downloads
//! without an async runtime.

use std::sync::Arc;

use anyhow::Result;

use crate::api::downloader::{self, ProgressUpdate, RequestContext};

pub type DownloadProgressHandler = Arc<dyn Fn(ProgressUpdate) + Send + Sync>;

#[allow(clippy::too_many_arguments)]
pub fn hls_to_mp4(
    url: String,
    concurrency: i32,
    output: String,
    retries: i32,
    video_bitrate: i32,
    audio_bitrate: i32,
    keep_temp: bool,
    progress: Option<DownloadProgressHandler>,
) -> Result<()> {
    downloader::hls2mp4_core(
        progress.unwrap_or_else(downloader::noop_progress_reporter),
        url,
        concurrency,
        output,
        retries,
        video_bitrate,
        audio_bitrate,
        keep_temp,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn download_media(
    page_url: String,
    media_url: String,
    audio_url: Option<String>,
    output: String,
    concurrency: i32,
    retries: i32,
    video_bitrate: i32,
    audio_bitrate: i32,
    keep_temp: bool,
    request_context: RequestContext,
    progress: Option<DownloadProgressHandler>,
) -> Result<()> {
    downloader::download_media_with_context_core(
        progress.unwrap_or_else(downloader::noop_progress_reporter),
        page_url,
        media_url,
        audio_url,
        output,
        concurrency,
        retries,
        video_bitrate,
        audio_bitrate,
        keep_temp,
        request_context,
    )
}
