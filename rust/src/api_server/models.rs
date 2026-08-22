use std::collections::HashMap;

use nextjson::{NsonDeserialize, NsonSerialize};
use tzcraft::Zoned;

use crate::api::downloader::{HeaderEntry, RequestContext};

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
