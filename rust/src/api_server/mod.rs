//! HTTP API server backed by `courierust_server`.
//!
//! Exposes a small REST API for queueing and inspecting media
//! downloads. Unlike the previous `warp`/tokio implementation, the
//! handler is synchronous: `courierust_server` dispatches each request
//! to a worker thread, and long-running download tasks run on their own
//! dedicated threads so the accept loop is never blocked.

use courierust::courierust_body::Body;
use courierust::courierust_http::header::HeaderName;
use courierust::courierust_http::request::Request;
use courierust::courierust_http::response::Response;
use courierust::courierust_http::status::StatusCode;
use courierust::courierust_server::Server;
use uuid::Uuid;

mod download;
mod models;
mod storage;

pub use models::{ApiRequestContext, DownloadRequest, DownloadStatus};

use self::storage::TaskStore;

/// Maximum request body accepted by the API (defense in depth; the
/// server config also bounds it).
const MAX_REQUEST_BODY: usize = 1024 * 1024;

/// Bind and serve the API forever (blocking).
pub fn run_server() -> std::io::Result<()> {
    let tasks = TaskStore::new();

    let host = std::env::var("API_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = std::env::var("API_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);
    let bind_ip = host
        .parse::<std::net::IpAddr>()
        .unwrap_or(std::net::IpAddr::from([0, 0, 0, 0]));

    let config = courierust::courierust_server::ServerConfig {
        max_body: MAX_REQUEST_BODY,
        http2: true,
        ..Default::default()
    };
    let server = Server::bind_with_config((bind_ip, port), config)?;
    let bound = server.local_addr()?;
    log::info!("FerrisLoad API listening on http://{}", bound);

    server.serve(move |request: Request<Body>| handle_request(request, &tasks))
}

fn handle_request(request: Request<Body>, tasks: &TaskStore) -> Response<Body> {
    let method = request.method.as_str().to_ascii_uppercase();
    let path = request.uri.path().to_string();

    let mut response = if method == "GET" && path == "/health" {
        handle_health()
    } else if method == "POST" && path == "/download" {
        handle_download(request, tasks)
    } else if method == "GET" && path == "/tasks" {
        handle_list(tasks)
    } else if method == "GET" && path.starts_with("/status/") {
        let task_id = path.trim_start_matches("/status/").to_string();
        handle_status(task_id, tasks)
    } else {
        json_response(StatusCode::NOT_FOUND, r#"{"error":"Not found"}"#)
    };

    // CORS: allow browser clients from any origin for this local
    // service. The previous warp implementation allowed any origin,
    // headers `content-type`, and methods GET/POST.
    response = response.header(
        HeaderName::from_lowercase("access-control-allow-origin"),
        "*",
    );
    response = response.header(
        HeaderName::from_lowercase("access-control-allow-methods"),
        "GET, POST, OPTIONS",
    );
    response = response.header(
        HeaderName::from_lowercase("access-control-allow-headers"),
        "content-type",
    );
    response = response.header(
        HeaderName::from_lowercase("content-type"),
        "application/json",
    );
    response
}

fn handle_health() -> Response<Body> {
    json_response(
        StatusCode::OK,
        r#"{"status":"ok","service":"ferrisload-api"}"#,
    )
}

fn handle_download(request: Request<Body>, tasks: &TaskStore) -> Response<Body> {
    // Read and bound the request body.
    let body = match request.body.collect_limited(MAX_REQUEST_BODY) {
        Ok(bytes) => bytes,
        Err(error) => {
            let message = format!(r#"{{"error":"{}"}}"#, escape_json(&error.to_string()));
            return json_response(StatusCode::BAD_REQUEST, &message);
        }
    };
    let req: DownloadRequest = match nextjson::from_slice(&body) {
        Ok(parsed) => parsed,
        Err(error) => {
            let message = format!(
                r#"{{"error":"Invalid JSON body: {}"}}"#,
                escape_json(&error.to_string())
            );
            return json_response(StatusCode::BAD_REQUEST, &message);
        }
    };
    if req.url.trim().is_empty() {
        return json_response(StatusCode::BAD_REQUEST, r#"{"error":"url is required"}"#);
    }

    let task_id = Uuid::new_v4().to_string();
    tasks.insert_queued(task_id.clone(), req.url.clone());

    let background_tasks = tasks.clone();
    let background_request = req.clone();
    let background_task_id = task_id.clone();
    std::thread::Builder::new()
        .name("ferrisload-download".into())
        .spawn(move || {
            download::run_download_task(background_task_id, background_request, background_tasks);
        })
        .expect("failed to spawn download thread");

    let payload = format!(
        r#"{{"task_id":"{}","status":"accepted","message":"Download task accepted"}}"#,
        task_id
    );
    json_response(StatusCode::ACCEPTED, &payload)
}

fn handle_status(task_id: String, tasks: &TaskStore) -> Response<Body> {
    match tasks.get(&task_id) {
        Some(status) => match nextjson::to_string(&status) {
            Ok(payload) => json_response(StatusCode::OK, &payload),
            Err(error) => {
                let message = format!(r#"{{"error":"{}"}}"#, escape_json(&error.to_string()));
                json_response(StatusCode::INTERNAL_SERVER_ERROR, &message)
            }
        },
        None => {
            let payload = format!(
                r#"{{"error":"Task not found","task_id":"{}"}}"#,
                escape_json(&task_id)
            );
            json_response(StatusCode::NOT_FOUND, &payload)
        }
    }
}

fn handle_list(tasks: &TaskStore) -> Response<Body> {
    match nextjson::to_string(&tasks.list()) {
        Ok(payload) => json_response(StatusCode::OK, &payload),
        Err(error) => {
            let message = format!(r#"{{"error":"{}"}}"#, escape_json(&error.to_string()));
            json_response(StatusCode::INTERNAL_SERVER_ERROR, &message)
        }
    }
}

fn json_response(status: StatusCode, payload: &str) -> Response<Body> {
    let mut response = Response::with_status(status);
    response.body = Body::Bytes(payload.as_bytes().to_vec().into());
    response
}

/// Escape a string for safe embedding inside a JSON string literal.
fn escape_json(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
