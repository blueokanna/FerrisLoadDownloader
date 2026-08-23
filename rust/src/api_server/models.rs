use std::collections::HashMap;

use nextjson::{NsonDeserialize, NsonSerialize};
use tzcraft::Zoned;

use crate::api::downloader::{HeaderEntry, MediaCandidate, MediaInspectionResult, RequestContext};

#[derive(Debug, Clone, NsonDeserialize)]
pub struct DownloadRequest {
    pub url: String,
    pub media_url: Option<String>,
    pub audio_url: Option<String>,
    pub output_filename: Option<String>,
    pub concurrency: Option<u32>,
    pub retries: Option<u32>,
    pub video_bitrate: Option<u32>,
    pub audio_bitrate: Option<u32>,
    pub keep_temp: Option<bool>,
    pub request_context: Option<ApiRequestContext>,
}

#[derive(Debug, Clone, Default, NsonDeserialize)]
pub struct ApiRequestContext {
    pub user_agent: Option<String>,
    pub referer: Option<String>,
    pub origin: Option<String>,
    pub cookie: Option<String>,
    pub headers: Option<HashMap<String, String>>,
}

impl From<ApiRequestContext> for RequestContext {
    fn from(value: ApiRequestContext) -> Self {
        let headers = value
            .headers
            .unwrap_or_default()
            .into_iter()
            .map(|(name, value)| HeaderEntry { name, value })
            .collect();

        Self {
            user_agent: value.user_agent.unwrap_or_default(),
            referer: value.referer.unwrap_or_default(),
            origin: value.origin.unwrap_or_default(),
            cookie: value.cookie.unwrap_or_default(),
            headers,
        }
    }
}

#[derive(Debug, Clone, NsonSerialize)]
pub struct DownloadStatus {
    pub task_id: String,
    pub status: String,
    pub message: String,
    pub progress_percent: Option<f64>,
    pub source_url: String,
    pub output_path: Option<String>,
    pub error: Option<String>,
    pub created_at: Zoned,
    pub completed_at: Option<Zoned>,
}

/// Request body of `POST /inspect`.
#[derive(Debug, Clone, NsonDeserialize)]
pub struct InspectRequest {
    pub url: String,
    pub request_context: Option<ApiRequestContext>,
}

/// JSON snapshot of `MediaInspectionResult` returned by `POST /inspect`.
/// The web client reconstructs the same model the native UI uses, so the
/// browser build can reuse the full analysis pipeline (via the API server).
#[derive(Debug, Clone, NsonSerialize)]
pub struct InspectionResponse {
    pub page_url: String,
    pub page_title: String,
    pub extractor: String,
    pub candidates: Vec<CandidateJson>,
    pub warnings: Vec<String>,
    pub auth_required: bool,
    pub challenge_reason: String,
}

/// JSON snapshot of `MediaCandidate` (snake_case keys for the web client).
#[derive(Debug, Clone, NsonSerialize)]
pub struct CandidateJson {
    pub id: String,
    pub title: String,
    pub extractor: String,
    pub page_url: String,
    pub media_url: String,
    pub audio_url: Option<String>,
    pub container: String,
    pub protocol: String,
    pub mime_type: String,
    pub quality_label: String,
    pub width: i32,
    pub height: i32,
    pub requires_ffmpeg: bool,
    pub score: i32,
    pub segment_count: i32,
    pub duration_seconds: f64,
    pub primary: bool,
    pub reason: String,
}

impl From<&MediaCandidate> for CandidateJson {
    fn from(candidate: &MediaCandidate) -> Self {
        Self {
            id: candidate.id.clone(),
            title: candidate.title.clone(),
            extractor: candidate.extractor.clone(),
            page_url: candidate.page_url.clone(),
            media_url: candidate.media_url.clone(),
            audio_url: candidate.audio_url.clone(),
            container: candidate.container.clone(),
            protocol: candidate.protocol.clone(),
            mime_type: candidate.mime_type.clone(),
            quality_label: candidate.quality_label.clone(),
            width: candidate.width,
            height: candidate.height,
            requires_ffmpeg: candidate.requires_ffmpeg,
            score: candidate.score,
            segment_count: candidate.segment_count,
            duration_seconds: candidate.duration_seconds,
            primary: candidate.primary,
            reason: candidate.reason.clone(),
        }
    }
}

impl From<&MediaInspectionResult> for InspectionResponse {
    fn from(result: &MediaInspectionResult) -> Self {
        Self {
            page_url: result.page_url.clone(),
            page_title: result.page_title.clone(),
            extractor: result.extractor.clone(),
            candidates: result.candidates.iter().map(CandidateJson::from).collect(),
            warnings: result.warnings.clone(),
            auth_required: result.auth_required,
            challenge_reason: result.challenge_reason.clone(),
        }
    }
}

impl DownloadStatus {
    pub fn queued(task_id: String, source_url: String) -> Self {
        let now = Zoned::now_utc()
            .unwrap_or_else(|_| Zoned::from_ticks(tzcraft::Ticks::EPOCH, tzcraft::Zone::Utc));
        Self {
            task_id,
            status: "queued".to_string(),
            message: "Task queued".to_string(),
            progress_percent: Some(0.0),
            source_url,
            output_path: None,
            error: None,
            created_at: now,
            completed_at: None,
        }
    }
}
