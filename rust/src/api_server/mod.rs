use std::convert::Infallible;

use uuid::Uuid;
use warp::http::StatusCode;
use warp::Filter;

mod download;
mod models;
mod storage;

pub use models::{ApiRequestContext, DownloadRequest, DownloadStatus};

use self::storage::TaskStore;

macro_rules! json_reply {
    ($status:expr, $body:expr) => {
        warp::reply::with_status(warp::reply::json(&$body), $status)
    };
}

pub async fn run_server() {
    let tasks = TaskStore::new();
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
) -> impl Filter<Extract = (TaskStore,), Error = Infallible> + Clone {
    warp::any().map(move || tasks.clone())
}

async fn handle_download(
    req: DownloadRequest,
    tasks: TaskStore,
) -> Result<impl warp::Reply, warp::Rejection> {
    let task_id = Uuid::new_v4().to_string();
    tasks.insert_queued(task_id.clone(), req.url.clone());

    let background_tasks = tasks.clone();
    let background_request = req.clone();
    let background_task_id = task_id.clone();
    tokio::spawn(async move {
        download::run_download_task(background_task_id, background_request, background_tasks).await;
    });

    Ok(json_reply!(
        StatusCode::ACCEPTED,
        serde_json::json!({
            "task_id": task_id,
            "status": "accepted",
            "message": "Download task accepted"
        })
    ))
}

async fn handle_status(
    task_id: String,
    tasks: TaskStore,
) -> Result<impl warp::Reply, warp::Rejection> {
    let reply = if let Some(status) = tasks.get(&task_id) {
        json_reply!(StatusCode::OK, status)
    } else {
        json_reply!(
            StatusCode::NOT_FOUND,
            serde_json::json!({
                "error": "Task not found",
                "task_id": task_id
            })
        )
    };
    Ok(reply)
}

async fn handle_list(tasks: TaskStore) -> Result<impl warp::Reply, warp::Rejection> {
    Ok(warp::reply::json(&tasks.list()))
}