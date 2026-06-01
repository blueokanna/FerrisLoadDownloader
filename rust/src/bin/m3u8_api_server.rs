use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;
use warp::http::StatusCode;
use warp::Filter;

use rust_lib_m3u8_downloader::api::downloader::{
    inspect_media_with_context, HeaderEntry, RequestContext,
};
use rust_lib_m3u8_downloader::download_service::{self, DownloadProgressHandler};

#[derive(Debug, Clone, Deserialize)]
struct DownloadRequest {
    url: String,
    media_url: Option<String>,
    audio_url: Option<String>,
    output_filename: Option<String>,
    concurrency: Option<u32>,
    retries: Option<u32>,
    video_bitrate: Option<u32>,
    audio_bitrate: Option<u32>,
    keep_temp: Option<bool>,
    request_context: Option<ApiRequestContext>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ApiRequestContext {
    user_agent: Option<String>,
    referer: Option<String>,
    origin: Option<String>,
    cookie: Option<String>,
    headers: Option<HashMap<String, String>>,
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

#[derive(Debug, Clone, Serialize)]
struct DownloadStatus {
    task_id: String,
    status: String,
    message: String,
    progress_percent: Option<f64>,
    source_url: String,
    output_path: Option<String>,
    error: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

type TaskStore = Arc<Mutex<HashMap<String, DownloadStatus>>>;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .try_init()
        .ok();

    let tasks: TaskStore = Arc::new(Mutex::new(HashMap::new()));
    let cors = warp::cors()
        .allow_any_origin()
        .allow_headers(vec!["content-type"])
        .allow_methods(vec!["GET", "POST"]);

    let health = warp::path("health").and(warp::get()).map(|| {
        warp::reply::json(&serde_json::json!({
            "status": "ok",
            "service": "ferrisload-api"
        }))
    });

    let download_tasks = tasks.clone();
    let download = warp::path("download")
        .and(warp::post())
        .and(warp::body::json())
        .and(with_tasks(download_tasks))
        .and_then(handle_download);

    let status_tasks = tasks.clone();
    let status = warp::path!("status" / String)
        .and(warp::get())
        .and(with_tasks(status_tasks))
        .and_then(handle_status);

    let list_tasks = tasks.clone();
    let list = warp::path("tasks")
        .and(warp::get())
        .and(with_tasks(list_tasks))
        .and_then(handle_list);

    let routes = health.or(download).or(status).or(list).with(cors).with(warp::log("api"));

    let host = std::env::var("API_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = std::env::var("API_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);
    let bind_ip = host
        .parse::<std::net::IpAddr>()
        .unwrap_or(std::net::IpAddr::from([0, 0, 0, 0]));

    log::info!("FerrisLoad API listening on http://{}:{}", bind_ip, port);
    warp::serve(routes).run((bind_ip, port)).await;
}

fn with_tasks(
    tasks: TaskStore,
) -> impl Filter<Extract = (TaskStore,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || tasks.clone())
}

async fn handle_download(
    req: DownloadRequest,
    tasks: TaskStore,
) -> Result<impl warp::Reply, warp::Rejection> {
    let task_id = Uuid::new_v4().to_string();
    let status = DownloadStatus {
        task_id: task_id.clone(),
        status: "queued".to_string(),
        message: "Task queued".to_string(),
        progress_percent: Some(0.0),
        source_url: req.url.clone(),
        output_path: None,
        error: None,
        created_at: chrono::Utc::now(),
        completed_at: None,
    };

    {
        let mut store = tasks.lock().await;
        store.insert(task_id.clone(), status);
    }

    let background_tasks = tasks.clone();
    let background_request = req.clone();
    let background_task_id = task_id.clone();
    tokio::spawn(async move {
        run_download_task(background_task_id, background_request, background_tasks).await;
    });

    Ok(warp::reply::with_status(
        warp::reply::json(&serde_json::json!({
            "task_id": task_id,
            "status": "accepted",
            "message": "Download task accepted"
        })),
        StatusCode::ACCEPTED,
    ))
}

async fn handle_status(
    task_id: String,
    tasks: TaskStore,
) -> Result<impl warp::Reply, warp::Rejection> {
    let store = tasks.lock().await;
    let reply = if let Some(status) = store.get(&task_id) {
        warp::reply::with_status(warp::reply::json(status), StatusCode::OK)
    } else {
        warp::reply::with_status(
            warp::reply::json(&serde_json::json!({
                "error": "Task not found",
                "task_id": task_id
            })),
            StatusCode::NOT_FOUND,
        )
    };
    Ok(reply)
}

async fn handle_list(tasks: TaskStore) -> Result<impl warp::Reply, warp::Rejection> {
    let store = tasks.lock().await;
    let task_list: Vec<DownloadStatus> = store.values().cloned().collect();
    Ok(warp::reply::json(&task_list))
}

async fn run_download_task(task_id: String, req: DownloadRequest, tasks: TaskStore) {
    if let Err(error) = execute_download_task(&task_id, &req, tasks.clone()).await {
        update_task_failed(&tasks, &task_id, &error.to_string()).await;
    }
}

async fn execute_download_task(task_id: &str, req: &DownloadRequest, tasks: TaskStore) -> Result<()> {
    update_task_progress(&tasks, task_id, "running", "Inspecting source...", Some(1.0), None).await;

    let request_context: RequestContext = req.request_context.clone().unwrap_or_default().into();
    let (page_url, media_url, audio_url, inferred_name) = resolve_media(req, &request_context).await?;
    let file_name = normalized_output_name(req.output_filename.as_deref(), &inferred_name);
    let download_dir = std::env::var("DOWNLOAD_DIR").unwrap_or_else(|_| "/app/downloads".to_string());
    let output_path = build_output_path(&download_dir, &file_name)?;

    update_task_progress(
        &tasks,
        task_id,
        "running",
        &format!("Resolved target stream: {}", media_url),
        Some(5.0),
        Some(output_path.to_string_lossy().to_string()),
    )
    .await;

    let runtime = tokio::runtime::Handle::current();
    let progress_tasks = tasks.clone();
    let progress_task_id = task_id.to_string();
    let output_path_string = output_path.to_string_lossy().to_string();
    let progress: DownloadProgressHandler = Arc::new(move |update| {
        let progress_tasks = progress_tasks.clone();
        let progress_task_id = progress_task_id.clone();
        let output_path_string = output_path_string.clone();
        runtime.spawn(async move {
            update_task_progress(
                &progress_tasks,
                &progress_task_id,
                "running",
                &update.message,
                Some((update.progress * 100.0).clamp(0.0, 100.0)),
                Some(output_path_string),
            )
            .await;
        });
    });

    download_service::download_media(
        page_url,
        media_url,
        audio_url,
        output_path.to_string_lossy().to_string(),
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

    let mut store = tasks.lock().await;
    if let Some(task) = store.get_mut(task_id) {
        task.status = "completed".to_string();
        task.message = "Download completed".to_string();
        task.progress_percent = Some(100.0);
        task.output_path = Some(output_path.to_string_lossy().to_string());
        task.error = None;
        task.completed_at = Some(chrono::Utc::now());
    }

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

async fn update_task_progress(
    tasks: &TaskStore,
    task_id: &str,
    status: &str,
    message: &str,
    progress_percent: Option<f64>,
    output_path: Option<String>,
) {
    let mut store = tasks.lock().await;
    if let Some(task) = store.get_mut(task_id) {
        task.status = status.to_string();
        task.message = message.to_string();
        task.progress_percent = progress_percent;
        if let Some(output_path) = output_path {
            task.output_path = Some(output_path);
        }
    }
}

async fn update_task_failed(tasks: &TaskStore, task_id: &str, error: &str) {
    let mut store = tasks.lock().await;
    if let Some(task) = store.get_mut(task_id) {
        task.status = "failed".to_string();
        task.message = "Download failed".to_string();
        task.error = Some(error.to_string());
        task.completed_at = Some(chrono::Utc::now());
    }
}

fn build_output_path(download_dir: &str, file_name: &str) -> Result<PathBuf> {
    let directory = Path::new(download_dir);
    std::fs::create_dir_all(directory)
        .with_context(|| format!("Failed to create download directory: {}", directory.display()))?;
    Ok(directory.join(file_name))
}

fn normalized_output_name(preferred: Option<&str>, fallback: &str) -> String {
    let raw = preferred.unwrap_or(fallback).trim();
    let source = if raw.is_empty() { "video" } else { raw };
    let sanitized: String = source
        .chars()
        .map(|ch| match ch {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ if ch.is_control() => '_',
            _ => ch,
        })
        .collect();
    if sanitized.to_ascii_lowercase().ends_with(".mp4") {
        sanitized
    } else {
        format!("{}.mp4", sanitized)
    }
}

fn infer_name_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()
                .and_then(|segments| segments.filter(|segment| !segment.is_empty()).last())
                .map(|segment| segment.to_string())
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("download-{}", Uuid::new_v4()))
}
