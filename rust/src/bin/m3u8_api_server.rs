use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use warp::Filter;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DownloadRequest {
    url: String,
    output_filename: Option<String>,
    concurrency: Option<u32>,
    retries: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DownloadStatus {
    task_id: String,
    status: String,
    progress: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

type TaskStore = Arc<Mutex<HashMap<String, DownloadStatus>>>;

#[tokio::main]
async fn main() {
    println!("M3U8下载器 API 服务器启动中...");

    // 初始化任务存储
    let tasks: TaskStore = Arc::new(Mutex::new(HashMap::new()));

    // 设置CORS
    let cors = warp::cors()
        .allow_any_origin()
        .allow_headers(vec!["content-type"])
        .allow_methods(vec!["GET", "POST"]);

    // 健康检查端点
    let health = warp::path("health")
        .map(|| warp::reply::json(&serde_json::json!({"status": "ok"})));

    // 下载任务端点
    let download_tasks = tasks.clone();
    let download = warp::path("download")
        .and(warp::post())
        .and(warp::body::json())
        .and(with_tasks(download_tasks))
        .and_then(handle_download);

    // 任务状态查询端点
    let status_tasks = tasks.clone();
    let status = warp::path!("status" / String)
        .and(warp::get())
        .and(with_tasks(status_tasks))
        .and_then(handle_status);

    // 任务列表端点
    let list_tasks = tasks.clone();
    let list = warp::path("tasks")
        .and(warp::get())
        .and(with_tasks(list_tasks))
        .and_then(handle_list);

    let routes = health
        .or(download)
        .or(status)
        .or(list)
        .with(cors)
        .with(warp::log("api"));

    println!("API 服务器运行在 http://localhost:3000");
    println!("可用端点:");
    println!("  GET  /health     - 健康检查");
    println!("  POST /download   - 开始下载任务");
    println!("  GET  /status/:id - 查询任务状态");
    println!("  GET  /tasks      - 列出所有任务");

    warp::serve(routes)
        .run(([0, 0, 0, 0], 3000))
        .await;
}

fn with_tasks(tasks: TaskStore) -> impl Filter<Extract = (TaskStore,), Error = std::convert::Infallible> + Clone {
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
        progress: Some("任务已加入队列".to_string()),
        created_at: chrono::Utc::now(),
        completed_at: None,
    };

    // 存储任务状态
    {
        let mut task_store = tasks.lock().await;
        task_store.insert(task_id.clone(), status.clone());
    }

    // 在后台启动下载任务
    let download_tasks = tasks.clone();
    let download_req = req.clone();
    let download_task_id = task_id.clone();

    tokio::spawn(async move {
        run_download_task(download_task_id, download_req, download_tasks).await;
    });

    Ok(warp::reply::json(&serde_json::json!({
        "task_id": task_id,
        "status": "accepted",
        "message": "下载任务已开始处理"
    })))
}

async fn run_download_task(
    task_id: String,
    req: DownloadRequest,
    tasks: TaskStore,
) {
    // 更新任务状态为进行中
    {
        let mut task_store = tasks.lock().await;
        if let Some(task) = task_store.get_mut(&task_id) {
            task.status = "running".to_string();
            task.progress = Some(format!("开始处理: {}", req.url));
        }
    }

    // 这里应该调用实际的下载逻辑
    // 由于这是一个示例，我们只是模拟下载过程
    for i in 1..=10 {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let target = req
            .output_filename
            .as_deref()
            .unwrap_or("output.mp4");
        let progress = format!("{} 下载进度: {}%", target, i * 10);
        {
            let mut task_store = tasks.lock().await;
            if let Some(task) = task_store.get_mut(&task_id) {
                task.progress = Some(progress);
            }
        }
    }

    // 完成任务
    {
        let mut task_store = tasks.lock().await;
        if let Some(task) = task_store.get_mut(&task_id) {
            task.status = "completed".to_string();
            task.progress = Some("下载完成".to_string());
            task.completed_at = Some(chrono::Utc::now());
        }
    }
}

async fn handle_status(
    task_id: String,
    tasks: TaskStore,
) -> Result<impl warp::Reply, warp::Rejection> {
    let task_store = tasks.lock().await;
    if let Some(status) = task_store.get(&task_id) {
        Ok(warp::reply::json(status))
    } else {
        Ok(warp::reply::json(&serde_json::json!({
            "error": "任务未找到",
            "task_id": task_id
        })))
    }
}

async fn handle_list(
    tasks: TaskStore,
) -> Result<impl warp::Reply, warp::Rejection> {
    let task_store = tasks.lock().await;
    let task_list: Vec<&DownloadStatus> = task_store.values().collect();
    Ok(warp::reply::json(&task_list))
}
