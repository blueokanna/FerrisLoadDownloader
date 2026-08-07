use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use uuid::Uuid;

use crate::api::downloader::{inspect_media_with_context, RequestContext};
use crate::download_service::{self, DownloadProgressHandler};

use super::models::DownloadRequest;
use super::storage::TaskStore;

pub async fn run_download_task(task_id: String, req: DownloadRequest, tasks: TaskStore) {
    if let Err(error) = execute_download_task(&task_id, &req, &tasks).await {
        tasks.mark_failed(&task_id, &error.to_string());
    }
}

async fn execute_download_task(
    task_id: &str,
    req: &DownloadRequest,
    tasks: &TaskStore,
) -> Result<()> {
    tasks.update_progress(task_id, "running", "Inspecting source...", Some(1.0), None);

    let request_context: RequestContext = req.request_context.clone().unwrap_or_default().into();
    let (page_url, media_url, audio_url, inferred_name) =
        resolve_media(req, &request_context).await?;
    let file_name = normalized_output_name(req.output_filename.as_deref(), &inferred_name);
    let download_dir =
        std::env::var("DOWNLOAD_DIR").unwrap_or_else(|_| "/app/downloads".to_string());
    let output_path = build_output_path(&download_dir, &file_name)?;
    let output_path_string = output_path.to_string_lossy().to_string();

    tasks.update_progress(
        task_id,
        "running",
        &format!("Resolved target stream: {}", media_url),
        Some(5.0),
        Some(output_path_string.clone()),
    );

    let progress_tasks = tasks.clone();
    let progress_task_id = task_id.to_string();
    let progress_output_path = output_path_string.clone();
    let progress: DownloadProgressHandler = Arc::new(move |update| {
        progress_tasks.update_progress(
            &progress_task_id,
            "running",
            &update.message,
            Some((update.progress * 100.0).clamp(0.0, 100.0)),
            Some(progress_output_path.clone()),
        );
    });

    download_service::download_media(
        page_url,
        media_url,
        audio_url,
        output_path_string.clone(),
        req.concurrency.unwrap_or(8) as i32,
        req.retries.unwrap_or(3) as i32,
        req.video_bitrate.unwrap_or(0) as i32,
        req.audio_bitrate.unwrap_or(0) as i32,
        req.keep_temp.unwrap_or(false),
        request_context,
        Some(progress),
    )
    .await?;

    let metadata = tokio::fs::metadata(&output_path)
        .await
        .with_context(|| format!("Output file was not created: {}", output_path.display()))?;
    if metadata.len() == 0 {
        return Err(anyhow!("Output file is empty: {}", output_path.display()));
    }

    tasks.mark_completed(task_id, output_path_string);
    Ok(())
}

async fn resolve_media(
    req: &DownloadRequest,
    request_context: &RequestContext,
) -> Result<(String, String, Option<String>, String)> {
    if let Some(media_url) = &req.media_url {
        return Ok((
            req.url.clone(),
            media_url.clone(),
            req.audio_url.clone(),
            req.output_filename
                .clone()
                .unwrap_or_else(|| infer_name_from_url(media_url)),
        ));
    }

    let inspection = inspect_media_with_context(req.url.clone(), request_context.clone()).await?;
    if inspection.auth_required {
        return Err(anyhow!(
            "Authorized session required: {}",
            if inspection.challenge_reason.is_empty() {
                "access challenge detected"
            } else {
                inspection.challenge_reason.as_str()
            }
        ));
    }

    let candidate = inspection
        .candidates
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("No downloadable media candidates were found for this URL"))?;

    Ok((
        candidate.page_url,
        candidate.media_url,
        candidate.audio_url,
        if candidate.title.trim().is_empty() {
            infer_name_from_url(&req.url)
        } else {
            candidate.title
        },
    ))
}

fn build_output_path(download_dir: &str, file_name: &str) -> Result<PathBuf> {
    let directory = Path::new(download_dir);
    std::fs::create_dir_all(directory).with_context(|| {
        format!(
            "Failed to create download directory: {}",
            directory.display()
        )
    })?;
    Ok(directory.join(file_name))
}

fn normalized_output_name(preferred: Option<&str>, fallback: &str) -> String {
    let raw = preferred.unwrap_or(fallback).trim();
    let source = if raw.is_empty() { "video" } else { raw };
    let sanitized = source
        .chars()
        .map(|ch| match ch {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ if ch.is_control() => '_',
            _ => ch,
        })
        .collect::<String>()
        .trim_matches(['.', ' '])
        .to_string();
    let file_name = if sanitized.is_empty() {
        format!("video-{}", Uuid::new_v4())
    } else {
        sanitized
    };
    if file_name.to_ascii_lowercase().ends_with(".mp4") {
        file_name
    } else {
        format!("{}.mp4", file_name)
    }
}

fn infer_name_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()
                .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
                .map(|segment| segment.to_string())
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("download-{}", Uuid::new_v4()))
}
