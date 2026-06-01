#![allow(dead_code)]
#![allow(unused_imports, unused_variables)]

use aes::Aes128;
use anyhow::{anyhow, bail, Context, Result};
use block_modes::block_padding::Pkcs7;
use block_modes::{BlockMode, Cbc};
use futures::stream::{self, StreamExt};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::{error, info, warn};
use m3u8_rs::{parse_playlist, Playlist};
use regex::Regex;
use reqwest::{header, Client};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(target_os = "android")]
use std::sync::OnceLock;
use std::time::Duration;
#[cfg(target_os = "android")]
use std::env;
use tokio::sync::Semaphore;
use tokio::{fs, process::Command, sync::Mutex};
use url::Url;
use crate::frb_generated::StreamSink;

#[cfg(target_os = "android")]
use jni::objects::{GlobalRef, JClass, JObject, JValue};
#[cfg(target_os = "android")]
use jni::JavaVM;

type Aes128Cbc = Cbc<Aes128, Pkcs7>;

#[derive(Clone, Copy, Debug)]
enum AccelType {
    Nvidia,
    AMD,
    CPU,
}

#[derive(Clone, Copy, Debug)]
enum TranscoderKind {
    Ffmpeg(AccelType),
    AndroidHardware,
}

#[derive(Clone)]
pub struct ProgressUpdate {
    pub message: String,
    pub progress: f64,
}

#[derive(Clone, Default)]
pub struct RequestContext {
    pub user_agent: String,
    pub referer: String,
    pub origin: String,
    pub cookie: String,
    pub headers: Vec<HeaderEntry>,
}

#[derive(Clone)]
pub struct HeaderEntry {
    pub name: String,
    pub value: String,
}

#[derive(Clone)]
pub struct MediaCandidate {
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

#[derive(Clone)]
pub struct MediaInspectionResult {
    pub page_url: String,
    pub page_title: String,
    pub extractor: String,
    pub candidates: Vec<MediaCandidate>,
    pub warnings: Vec<String>,
    pub auth_required: bool,
    pub challenge_reason: String,
}

#[cfg(target_os = "android")]
static ANDROID_HW_TRANSCODER: OnceLock<Arc<AndroidMediaCodecTranscoder>> = OnceLock::new();

#[cfg(target_os = "android")]
static ANDROID_CONTEXT: OnceLock<AndroidContextData> = OnceLock::new();

/// Cached MediaTranscoder class reference (must be cached from main thread with app classloader)
#[cfg(target_os = "android")]
static MEDIA_TRANSCODER_CLASS: OnceLock<GlobalRef> = OnceLock::new();

#[cfg(target_os = "android")]
pub struct AndroidMediaCodecTranscoder {
    jvm: Arc<JavaVM>,
}

#[cfg(target_os = "android")]
impl AndroidMediaCodecTranscoder {
    pub fn new(jvm: Arc<JavaVM>) -> Self {
        Self { jvm }
    }

    pub async fn transcode(
        &self,
        input_ts: &str,
        output_mp4: &str,
        video_bitrate: u32,
        audio_bitrate: u32,
    ) -> Result<()> {
        let jvm = self.jvm.clone();
        let input_ts = input_ts.to_string();
        let output_mp4 = output_mp4.to_string();

        tokio::task::spawn_blocking(move || {
            let mut env = jvm
                .attach_current_thread()
                .map_err(|e| anyhow!("JNI attach thread failed: {}", e))?;

            // Try to get cached class first, otherwise load it using app ClassLoader
            let class: JClass = if let Some(class_ref) = MEDIA_TRANSCODER_CLASS.get() {
                // SAFETY: GlobalRef was created from a JClass, so it's safe to cast back
                unsafe { JClass::from_raw(class_ref.as_obj().as_raw()) }
            } else {
                // Load class using app context's ClassLoader (works from any thread)
                let ctx = get_android_context()
                    .map_err(|e| anyhow!("Failed to get Android context: {}", e))?;
                
                // Get ClassLoader from app context
                let class_loader = env
                    .call_method(ctx.app_context.as_obj(), "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
                    .map_err(|e| anyhow!("Failed to get ClassLoader: {:?}", e))?
                    .l()
                    .map_err(|e| anyhow!("ClassLoader is not an object: {:?}", e))?;
                
                // Use ClassLoader.loadClass() to load MediaTranscoder
                let class_name = env
                    .new_string("com.bluevale.m3u8_downloader.MediaTranscoder")
                    .map_err(|e| anyhow!("Failed to create class name string: {:?}", e))?;
                
                let loaded_class = env
                    .call_method(
                        &class_loader,
                        "loadClass",
                        "(Ljava/lang/String;)Ljava/lang/Class;",
                        &[JValue::Object(&class_name)],
                    )
                    .map_err(|e| anyhow!("Failed to load MediaTranscoder class: {:?}", e))?
                    .l()
                    .map_err(|e| anyhow!("loadClass did not return a Class: {:?}", e))?;
                
                info!("✅ MediaTranscoder class loaded via ClassLoader");
                
                // Cache the class for future use
                if let Ok(global_ref) = env.new_global_ref(&loaded_class) {
                    let _ = MEDIA_TRANSCODER_CLASS.set(global_ref);
                }
                
                // SAFETY: loaded_class is a java.lang.Class object
                unsafe { JClass::from_raw(loaded_class.as_raw()) }
            };

            let input_ts_jstring = env
                .new_string(&input_ts)
                .map_err(|e| anyhow!("JNI new_string failed: {}", e))?;

            let output_mp4_jstring = env
                .new_string(&output_mp4)
                .map_err(|e| anyhow!("JNI new_string failed: {}", e))?;

            let result = env
                .call_static_method(
                    class,
                    "transcode",
                    "(Ljava/lang/String;Ljava/lang/String;II)Z",
                    &[
                        JValue::Object(&input_ts_jstring),
                        JValue::Object(&output_mp4_jstring),
                        JValue::Int(video_bitrate as i32),
                        JValue::Int(audio_bitrate as i32),
                    ],
                )
                .map_err(|e| anyhow!("JNI call_static_method failed: {}", e))?;

            let success = result
                .z()
                .map_err(|e| anyhow!("JNI get boolean return failed: {}", e))?;

            if success {
                Ok(())
            } else {
                Err(anyhow!("Java MediaCodec transcode failed"))
            }
        })
        .await
        .map_err(|e| anyhow!("tokio spawn_blocking failed: {}", e))?
    }

    pub async fn mux(
        &self,
        video_path: &str,
        audio_path: &str,
        output_mp4: &str,
    ) -> Result<()> {
        let jvm = self.jvm.clone();
        let video_path = video_path.to_string();
        let audio_path = audio_path.to_string();
        let output_mp4 = output_mp4.to_string();

        tokio::task::spawn_blocking(move || {
            let mut env = jvm
                .attach_current_thread()
                .map_err(|e| anyhow!("JNI attach thread failed: {}", e))?;

            let class: JClass = if let Some(class_ref) = MEDIA_TRANSCODER_CLASS.get() {
                unsafe { JClass::from_raw(class_ref.as_obj().as_raw()) }
            } else {
                let ctx = get_android_context()
                    .map_err(|e| anyhow!("Failed to get Android context: {}", e))?;
                let class_loader = env
                    .call_method(ctx.app_context.as_obj(), "getClassLoader", "()Ljava/lang/ClassLoader;", &[])
                    .map_err(|e| anyhow!("Failed to get ClassLoader: {:?}", e))?
                    .l()
                    .map_err(|e| anyhow!("ClassLoader is not an object: {:?}", e))?;
                let class_name = env
                    .new_string("com.bluevale.m3u8_downloader.MediaTranscoder")
                    .map_err(|e| anyhow!("Failed to create class name string: {:?}", e))?;
                let loaded_class = env
                    .call_method(
                        &class_loader,
                        "loadClass",
                        "(Ljava/lang/String;)Ljava/lang/Class;",
                        &[JValue::Object(&class_name)],
                    )
                    .map_err(|e| anyhow!("Failed to load MediaTranscoder class: {:?}", e))?
                    .l()
                    .map_err(|e| anyhow!("loadClass did not return a Class: {:?}", e))?;
                if let Ok(global_ref) = env.new_global_ref(&loaded_class) {
                    let _ = MEDIA_TRANSCODER_CLASS.set(global_ref);
                }
                unsafe { JClass::from_raw(loaded_class.as_raw()) }
            };

            let video_jstring = env
                .new_string(&video_path)
                .map_err(|e| anyhow!("JNI new_string failed: {}", e))?;
            let audio_jstring = env
                .new_string(&audio_path)
                .map_err(|e| anyhow!("JNI new_string failed: {}", e))?;
            let output_jstring = env
                .new_string(&output_mp4)
                .map_err(|e| anyhow!("JNI new_string failed: {}", e))?;

            let result = env
                .call_static_method(
                    class,
                    "mux",
                    "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Z",
                    &[
                        JValue::Object(&video_jstring),
                        JValue::Object(&audio_jstring),
                        JValue::Object(&output_jstring),
                    ],
                )
                .map_err(|e| anyhow!("JNI call_static_method mux failed: {}", e))?;

            let success = result
                .z()
                .map_err(|e| anyhow!("JNI get boolean return failed: {}", e))?;

            if success {
                Ok(())
            } else {
                Err(anyhow!("Java MediaMuxer merge failed"))
            }
        })
        .await
        .map_err(|e| anyhow!("tokio spawn_blocking failed: {}", e))?
    }
}

#[cfg(target_os = "android")]
pub fn register_android_mediacodec_transcoder(jvm: Arc<JavaVM>) -> Result<()> {
    if ANDROID_HW_TRANSCODER.get().is_some() {
        info!("Android MediaCodec transcoder already registered");
        return Ok(());
    }

    let transcoder = AndroidMediaCodecTranscoder::new(jvm);
    ANDROID_HW_TRANSCODER
        .set(Arc::new(transcoder))
        .map_err(|_| anyhow!("Failed to register Android MediaCodec transcoder"))?;

    info!("鉁� Android MediaCodec transcoder registered");
    Ok(())
}

#[cfg(target_os = "android")]
struct AndroidContextData {
    jvm: Arc<JavaVM>,
    app_context: GlobalRef,
}

#[cfg(target_os = "android")]
pub fn init_android_context(jvm: Arc<JavaVM>, context: GlobalRef) -> Result<()> {
    let data = AndroidContextData {
        jvm,
        app_context: context,
    };

    ANDROID_CONTEXT
        .set(data)
        .map_err(|_| anyhow!("Android Context already initialized"))?;

    info!("Android context initialized");
    Ok(())
}

#[cfg(target_os = "android")]
fn get_android_context() -> Result<&'static AndroidContextData> {
    ANDROID_CONTEXT
        .get()
        .ok_or_else(|| anyhow!("Android Context not initialized; call init_android_context()"))
}

#[cfg(target_os = "android")]
fn verify_directory_writable(path: &PathBuf) -> bool {
    if !path.exists() {
        if let Err(e) = std::fs::create_dir_all(&path) {
            warn!("Failed to create directory {}: {}", path.display(), e);
            return false;
        }
    }

    if !path.is_dir() {
        warn!("Path exists but is not a directory: {}", path.display());
        return false;
    }

    let test_file = path.join(".writable_test_temp");
    match std::fs::write(&test_file, b"test") {
        Ok(_) => {
            if let Err(e) = std::fs::remove_file(&test_file) {
                warn!("Failed to remove test file: {}", e);
            }
            info!("Directory is writable: {}", path.display());
            true
        }
        Err(e) => {
            warn!(
                "Directory {} is not writable: {} (OS Error: {})",
                path.display(),
                e,
                e.raw_os_error().unwrap_or(0)
            );
            false
        }
    }
}

#[cfg(target_os = "android")]
pub fn get_app_cache_dir() -> Result<PathBuf> {
    let ctx_data = get_android_context()?;
    let mut env = ctx_data
        .jvm
        .attach_current_thread()
        .map_err(|e| anyhow!("JNI attach thread failed: {}", e))?;

    let context_obj = ctx_data.app_context.as_obj();

    let cache_dir_obj = env
        .call_method(context_obj, "getCacheDir", "()Ljava/io/File;", &[])
        .map_err(|e| anyhow!("JNI getCacheDir call failed: {}", e))?
        .l()
        .map_err(|e| anyhow!("JNI getCacheDir returned invalid object: {}", e))?;

    let path_str = env
        .call_method(
            &cache_dir_obj,
            "getAbsolutePath",
            "()Ljava/lang/String;",
            &[],
        )
        .map_err(|e| anyhow!("JNI getAbsolutePath call failed: {}", e))?
        .l()
        .map_err(|e| anyhow!("JNI getAbsolutePath returned invalid object: {}", e))?;

    let path_jstring = env
        .get_string((&path_str).into())
        .map_err(|e| anyhow!("JNI string conversion failed: {}", e))?;

    Ok(PathBuf::from(path_jstring.to_string_lossy().to_string()))
}

#[cfg(target_os = "android")]
pub fn get_app_files_dir() -> Result<PathBuf> {
    let ctx_data = get_android_context()?;
    let mut env = ctx_data
        .jvm
        .attach_current_thread()
        .map_err(|e| anyhow!("JNI attach thread failed: {}", e))?;

    let context_obj = ctx_data.app_context.as_obj();

    let files_dir_obj = env
        .call_method(context_obj, "getFilesDir", "()Ljava/io/File;", &[])
        .map_err(|e| anyhow!("JNI getFilesDir call failed: {}", e))?
        .l()
        .map_err(|e| anyhow!("JNI getFilesDir returned invalid object: {}", e))?;

    let path_str = env
        .call_method(
            &files_dir_obj,
            "getAbsolutePath",
            "()Ljava/lang/String;",
            &[],
        )
        .map_err(|e| anyhow!("JNI getAbsolutePath call failed: {}", e))?
        .l()
        .map_err(|e| anyhow!("JNI getAbsolutePath returned invalid object: {}", e))?;

    let path_jstring = env
        .get_string((&path_str).into())
        .map_err(|e| anyhow!("JNI string conversion failed: {}", e))?;

    Ok(PathBuf::from(path_jstring.to_string_lossy().to_string()))
}

#[cfg(target_os = "android")]
pub fn get_external_files_dir() -> Result<PathBuf> {
    let ctx_data = get_android_context()?;
    let mut env = ctx_data
        .jvm
        .attach_current_thread()
        .map_err(|e| anyhow!("JNI attach thread failed: {}", e))?;

    let context_obj = ctx_data.app_context.as_obj();

    let ext_files_call = env.call_method(
        context_obj,
        "getExternalFilesDir",
        "(Ljava/lang/String;)Ljava/io/File;",
        &[JValue::Object(&JObject::null())],
    );

    let ext_files_obj = match ext_files_call {
        Ok(v) => v
            .l()
            .map_err(|e| anyhow!("JNI getExternalFilesDir returned invalid object: {}", e))?,
        Err(e) => {
            warn!("JNI getExternalFilesDir call failed: {}", e);
            return get_app_files_dir();
        }
    };

    if ext_files_obj.is_null() {
        warn!("getExternalFilesDir returned null, falling back to app files dir");
        return get_app_files_dir();
    }

    let path_str = env
        .call_method(
            &ext_files_obj,
            "getAbsolutePath",
            "()Ljava/lang/String;",
            &[],
        )
        .map_err(|e| anyhow!("JNI getAbsolutePath call failed: {}", e))?
        .l()
        .map_err(|e| anyhow!("JNI getAbsolutePath returned invalid object: {}", e))?;

    let path_jstring = env
        .get_string((&path_str).into())
        .map_err(|e| anyhow!("JNI string conversion failed: {}", e))?;

    Ok(PathBuf::from(path_jstring.to_string_lossy().to_string()))
}

#[cfg(target_os = "android")]
fn select_writable_temp_dir() -> Result<PathBuf> {
    info!("Selecting writable temporary directory");

    let candidates = vec![
        ("app_cache", get_app_cache_dir()),
        ("app_files", get_app_files_dir()),
        ("external_files", get_external_files_dir()),
    ];

    for (name, result) in candidates {
        match result {
            Ok(dir) => {
                info!("Trying candidate [{}]: {}", name, dir.display());
                if verify_directory_writable(&dir) {
                    info!("Selected writable temporary directory: {}", dir.display());
                    return Ok(dir);
                } else {
                    warn!("Directory not writable: {} ({})", dir.display(), name);
                }
            }
            Err(e) => {
                warn!("Failed to get {}: {}", name, e);
            }
        }
    }

    let env_temp = env::temp_dir();
    info!("Trying env::temp_dir(): {}", env_temp.display());
    if verify_directory_writable(&env_temp) {
        return Ok(env_temp);
    }

    let data_local_tmp = PathBuf::from("/data/local/tmp");
    info!("Trying /data/local/tmp");
    if verify_directory_writable(&data_local_tmp) {
        return Ok(data_local_tmp);
    }

    let current = PathBuf::from(".");
    info!("Trying current directory");
    if verify_directory_writable(&current) {
        return Ok(current);
    }

    bail!(
        "No writable temporary directory found. Ensure init_android_context() was called with an Application context and that storage is configured."
    )
}

#[flutter_rust_bridge::frb()]
pub async fn hls2mp4_run(
    sink: StreamSink<ProgressUpdate>,
    url: String,
    concurrency: i32,
    output: String,
    retries: i32,
    video_bitrate: i32,
    audio_bitrate: i32,
    keep_temp: bool,
) -> Result<()> {
    let request_context = RequestContext::default();
    run_hls_pipeline(
        sink,
        &url,
        &request_context,
        concurrency,
        &output,
        retries,
        video_bitrate,
        audio_bitrate,
        keep_temp,
    )
    .await
}

#[flutter_rust_bridge::frb()]
pub async fn inspect_media_from_url(url: String) -> Result<MediaInspectionResult> {
    inspect_media_with_context(url, RequestContext::default()).await
}

#[flutter_rust_bridge::frb()]
pub async fn inspect_media_with_context(
    url: String,
    request_context: RequestContext,
) -> Result<MediaInspectionResult> {
    init_runtime_logging();

    if let Some(candidate) = direct_media_candidate(&url, &url) {
        return Ok(MediaInspectionResult {
            page_url: url.clone(),
            page_title: infer_title_from_url(&url),
            extractor: "direct".to_string(),
            candidates: vec![candidate],
            warnings: Vec::new(),
            auth_required: false,
            challenge_reason: String::new(),
        });
    }

    let page_url = Url::parse(&url).context("Invalid inspection URL")?;
    let client = create_http_client_for_context(Some(&url), &request_context)?;
    let response = client
        .get(page_url.clone())
        .send()
        .await?;
    let status = response.status();
    let html = response.text().await?;
    let mut warnings = Vec::new();
    if let Some(reason) = detect_access_challenge(status.as_u16(), &html) {
        warnings.push(reason.clone());
        return Ok(MediaInspectionResult {
            page_url: url,
            page_title: extract_page_title(&html).unwrap_or_else(|| "Authorization required".to_string()),
            extractor: extractor_name_for_host(page_url.domain()),
            candidates: Vec::new(),
            warnings,
            auth_required: true,
            challenge_reason: reason,
        });
    }
    if !status.is_success() {
        bail!("Failed to inspect page: HTTP {}", status);
    }
    let page_title = extract_page_title(&html).unwrap_or_else(|| infer_title_from_url(&url));
    let extractor = extractor_name_for_host(page_url.domain());
    let mut collector = CandidateCollector::new(page_url.as_str(), &page_title, &extractor);

    match page_url.domain() {
        Some(domain) if domain.contains("youtube.com") || domain.contains("youtu.be") => {
            extract_youtube_candidates(&page_url, &html, &mut collector, &mut warnings)?;
        }
        Some(domain) if domain.contains("bilibili.com") || domain.contains("b23.tv") => {
            extract_bilibili_candidates(&page_url, &html, &mut collector, &mut warnings)?;
        }
        _ => {}
    }

    extract_generic_candidates(&page_url, &html, &mut collector)?;
    let candidates = score_candidates(collector.finish(), &request_context).await;

    if candidates.is_empty() {
        warnings.push("No downloadable media candidates were found in the current page source".to_string());
    }

    Ok(MediaInspectionResult {
        page_url: url,
        page_title,
        extractor,
        candidates,
        warnings,
        auth_required: false,
        challenge_reason: String::new(),
    })
}

#[flutter_rust_bridge::frb()]
pub async fn download_media_run(
    sink: StreamSink<ProgressUpdate>,
    page_url: String,
    media_url: String,
    audio_url: Option<String>,
    output: String,
    concurrency: i32,
    retries: i32,
    video_bitrate: i32,
    audio_bitrate: i32,
    keep_temp: bool,
) -> Result<()> {
    download_media_with_context(
        sink,
        page_url,
        media_url,
        audio_url,
        output,
        concurrency,
        retries,
        video_bitrate,
        audio_bitrate,
        keep_temp,
        RequestContext::default(),
    )
    .await
}

#[flutter_rust_bridge::frb()]
pub async fn download_media_with_context(
    sink: StreamSink<ProgressUpdate>,
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
) -> Result<()> {
    init_runtime_logging();

    if is_hls_like(&media_url) {
        return run_hls_pipeline(
            sink,
            &media_url,
            &request_context,
            concurrency,
            &output,
            retries,
            video_bitrate,
            audio_bitrate,
            keep_temp,
        )
        .await;
    }

    let _ = sink.add(ProgressUpdate {
        message: "Preparing direct media download...".to_string(),
        progress: 0.02,
    });

    let page_url = Url::parse(&page_url).ok();
    let client = create_http_client_for_context(page_url.as_ref().map(Url::as_str), &request_context)?;
    let output_path = PathBuf::from(&output);
    let temp_dir = output_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    if !temp_dir.exists() {
        std::fs::create_dir_all(&temp_dir)
            .with_context(|| format!("Failed to create output directory: {}", temp_dir.display()))?;
    }

    match audio_url {
        Some(audio) => {
            let video_temp = temp_dir.join("stream_video_input.bin");
            let audio_temp = temp_dir.join("stream_audio_input.bin");

            download_to_file(&client, &media_url, &video_temp, 0.42, &sink, "Downloading video stream")
                .await?;
            download_to_file(&client, &audio, &audio_temp, 0.78, &sink, "Downloading audio stream")
                .await?;

            merge_media_streams(
                &video_temp,
                &audio_temp,
                &output,
                video_bitrate.max(0) as u32,
                audio_bitrate.max(0) as u32,
            )
            .await?;

            if !keep_temp {
                let _ = fs::remove_file(video_temp).await;
                let _ = fs::remove_file(audio_temp).await;
            }
        }
        None => {
            let extension = container_from_url(&media_url);
            if extension == "mp4" {
                download_to_file(&client, &media_url, &output_path, 0.95, &sink, "Downloading media")
                    .await?;
            } else {
                let temp_input = temp_dir.join(format!("direct_input.{}", extension));
                download_to_file(&client, &media_url, &temp_input, 0.72, &sink, "Downloading media")
                    .await?;
                convert_to_mp4(
                    temp_input.to_string_lossy().as_ref(),
                    &output,
                    video_bitrate.max(0) as u32,
                    audio_bitrate.max(0) as u32,
                    &MultiProgress::new(),
                    select_transcoder_backend().await?,
                    sink.clone(),
                )
                .await?;
                if !keep_temp {
                    let _ = fs::remove_file(temp_input).await;
                }
            }
        }
    }

    let _ = sink.add(ProgressUpdate {
        message: "All tasks completed".to_string(),
        progress: 1.0,
    });

    Ok(())
}

async fn run_hls_pipeline(
    sink: StreamSink<ProgressUpdate>,
    url: &str,
    request_context: &RequestContext,
    concurrency: i32,
    output: &str,
    retries: i32,
    video_bitrate: i32,
    audio_bitrate: i32,
    keep_temp: bool,
) -> Result<()> {
    let _ = sink.add(ProgressUpdate {
        message: "Initializing...".to_string(),
        progress: 0.0,
    });

    init_runtime_logging();

    let concurrency = concurrency.max(1) as usize;
    let retries = retries.max(1) as u8;
    let video_bitrate = video_bitrate.max(0) as u32;
    let audio_bitrate = audio_bitrate.max(0) as u32;
    let multi_progress = MultiProgress::new();

    let check_pb = multi_progress.add(ProgressBar::new_spinner());
    check_pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg}")?
            .tick_strings(&["-", "\\", "|", "/"]),
    );
    check_pb.set_message("Selecting transcoder backend...");
    check_pb.enable_steady_tick(Duration::from_millis(100));

    let _ = sink.add(ProgressUpdate {
        message: "Selecting transcoder backend...".to_string(),
        progress: 0.01,
    });

    let backend = select_transcoder_backend().await?;
    match backend {
        TranscoderKind::Ffmpeg(accel) => {
            check_pb.finish_with_message(format!("Selected FFmpeg backend ({:?})", accel));
        }
        TranscoderKind::AndroidHardware => {
            check_pb.finish_with_message("Selected Android MediaCodec backend");
        }
    }

    info!("M3U8 URL: {}", url);

    let download_pb = multi_progress.add(ProgressBar::new_spinner());
    download_pb.set_style(
        ProgressStyle::with_template("{spinner:.blue} {msg}")?.tick_strings(&["-", "\\", "|", "/"]),
    );
    download_pb.set_message("Downloading M3U8 playlist...");
    download_pb.enable_steady_tick(Duration::from_millis(100));

    let _ = sink.add(ProgressUpdate {
        message: "Downloading M3U8 playlist...".to_string(),
        progress: 0.02,
    });

    let m3u8_content = download_playlist(url, request_context).await?;
    let (_, playlist) =
        parse_playlist(&m3u8_content).map_err(|e| anyhow!("Failed to parse M3U8: {:?}", e))?;
    download_pb.finish_with_message("Parsed M3U8 playlist");

    let base_url = if url.starts_with("http") {
        let mut parsed_url = Url::parse(url)?;
        parsed_url.set_query(None);
        let mut path = parsed_url.path().to_string();
        if let Some(pos) = path.rfind('/') {
            path.truncate(pos + 1);
            parsed_url.set_path(&path);
            Some(parsed_url)
        } else {
            None
        }
    } else {
        None
    };

    let temp_dir = if cfg!(target_os = "android") {
        #[cfg(target_os = "android")]
        {
            select_writable_temp_dir()?
        }
        #[cfg(not(target_os = "android"))]
        {
            unreachable!()
        }
    } else {
        PathBuf::from(".")
    };

    let temp_ts = temp_dir.join("temp_merged.ts");
    let temp_ts_str = temp_ts.to_string_lossy().to_string();

    info!("Temporary directory: {}", temp_dir.display());
    info!("Temporary TS file: {}", temp_ts_str);

    match playlist {
        Playlist::MasterPlaylist(master) => {
            info!("Master Playlist found, {} variants", master.variants.len());

            let best = master
                .variants
                .iter()
                .max_by_key(|v| {
                    let resolution_score = v
                        .resolution
                        .as_ref()
                        .map(|r| r.width * r.height)
                        .unwrap_or(0);
                    (resolution_score, v.bandwidth)
                })
                .ok_or_else(|| anyhow!("No usable variant found"))?;

            info!(
                "Selected variant: bandwidth {} , resolution {:?}",
                best.bandwidth,
                best.resolution
                    .as_ref()
                    .map(|r| format!("{}x{}", r.width, r.height))
            );

            let media_url = if let Some(base) = &base_url {
                base.join(&best.uri)?
            } else {
                bail!("Master playlist missing URL");
            };

            let media_content = download_playlist(media_url.as_str(), request_context).await?;
            let (_, media_pl) = parse_playlist(&media_content)
                .map_err(|e| anyhow!("Failed to parse m3u8: {:?}", e))?;

            if let Playlist::MediaPlaylist(mp) = media_pl {
                download_and_merge(
                    mp,
                    base_url,
                    concurrency,
                    retries,
                    &temp_ts_str,
                    &temp_dir,
                    &multi_progress,
                    sink.clone(),
                    request_context.clone(),
                )
                .await?;
            } else {
                bail!("Master playlist's referenced playlist is not a media playlist");
            }
        }
        Playlist::MediaPlaylist(mp) => {
            info!("Media Playlist found, {} segments", mp.segments.len());
            download_and_merge(
                mp,
                base_url,
                concurrency,
                retries,
                &temp_ts_str,
                &temp_dir,
                &multi_progress,
                sink.clone(),
                request_context.clone(),
            )
            .await?;
        }
    }

    convert_to_mp4(
        &temp_ts_str,
        output,
        video_bitrate,
        audio_bitrate,
        &multi_progress,
        backend,
        sink.clone(),
    )
    .await?;

    if !keep_temp {
        let _ = fs::remove_file(&temp_ts_str).await;
    }

    let _ = sink.add(ProgressUpdate {
        message: "All tasks completed".to_string(),
        progress: 1.0,
    });

    Ok(())
}

fn init_runtime_logging() {
    #[cfg(target_os = "android")]
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    #[cfg(not(target_os = "android"))]
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .try_init()
        .ok();
}

struct CandidateCollector {
    page_url: String,
    default_title: String,
    extractor: String,
    seen: HashSet<String>,
    candidates: Vec<MediaCandidate>,
}

impl CandidateCollector {
    fn new(page_url: &str, default_title: &str, extractor: &str) -> Self {
        Self {
            page_url: page_url.to_string(),
            default_title: default_title.to_string(),
            extractor: extractor.to_string(),
            seen: HashSet::new(),
            candidates: Vec::new(),
        }
    }

    fn push(
        &mut self,
        media_url: String,
        audio_url: Option<String>,
        title: Option<String>,
        quality_label: Option<String>,
        mime_type: Option<String>,
        width: Option<i32>,
        height: Option<i32>,
        extractor: Option<&str>,
    ) {
        if media_url.is_empty() {
            return;
        }
        let key = format!("{}|{}", media_url, audio_url.clone().unwrap_or_default());
        if !self.seen.insert(key) {
            return;
        }
        let protocol = protocol_from_url(&media_url);
        let container = container_from_url(&media_url);
        let resolved_title = title.unwrap_or_else(|| self.default_title.clone());
        self.candidates.push(MediaCandidate {
            id: uuid::Uuid::new_v4().to_string(),
            title: resolved_title,
            extractor: extractor.unwrap_or(&self.extractor).to_string(),
            page_url: self.page_url.clone(),
            media_url,
            audio_url: audio_url.clone(),
            container,
            protocol,
            mime_type: mime_type.unwrap_or_else(|| mime_from_urls(audio_url.is_some()).to_string()),
            quality_label: quality_label.unwrap_or_else(|| "Auto".to_string()),
            width: width.unwrap_or(0),
            height: height.unwrap_or(0),
            requires_ffmpeg: audio_url.is_some() && !cfg!(target_os = "android"),
            score: 0,
            segment_count: 0,
            duration_seconds: 0.0,
            primary: false,
            reason: String::new(),
        });
    }

    fn finish(mut self) -> Vec<MediaCandidate> {
        self.candidates.sort_by(|left, right| {
            let left_score = (left.height.max(0), left.width.max(0), left.quality_label.clone());
            let right_score = (right.height.max(0), right.width.max(0), right.quality_label.clone());
            right_score.cmp(&left_score)
        });
        self.candidates
    }
}

fn extractor_name_for_host(host: Option<&str>) -> String {
    match host {
        Some(domain) if domain.contains("youtube.com") || domain.contains("youtu.be") => "youtube".to_string(),
        Some(domain) if domain.contains("bilibili.com") || domain.contains("b23.tv") => "bilibili".to_string(),
        Some(domain) => domain.to_string(),
        None => "generic".to_string(),
    }
}

fn infer_title_from_url(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()
                .and_then(|segments| segments.filter(|segment| !segment.is_empty()).last().map(str::to_string))
        })
        .map(|name| name.replace(['-', '_'], " "))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Untitled download".to_string())
}

fn direct_media_candidate(page_url: &str, media_url: &str) -> Option<MediaCandidate> {
    if !is_supported_media_like(media_url) {
        return None;
    }

    Some(MediaCandidate {
        id: uuid::Uuid::new_v4().to_string(),
        title: infer_title_from_url(media_url),
        extractor: "direct".to_string(),
        page_url: page_url.to_string(),
        media_url: media_url.to_string(),
        audio_url: None,
        container: container_from_url(media_url),
        protocol: protocol_from_url(media_url),
        mime_type: mime_from_extension(&container_from_url(media_url)).to_string(),
        quality_label: "Direct".to_string(),
        width: 0,
        height: 0,
        requires_ffmpeg: false,
        score: 200,
        segment_count: 0,
        duration_seconds: 0.0,
        primary: true,
        reason: "direct media url".to_string(),
    })
}

fn is_supported_media_like(url: &str) -> bool {
    is_hls_like(url)
        || url.contains(".mp4")
        || url.contains(".webm")
        || url.contains(".mkv")
        || url.contains(".m4v")
        || url.contains(".mpd")
}

fn is_hls_like(url: &str) -> bool {
    url.contains(".m3u8") || url.contains("application/vnd.apple.mpegurl")
}

fn protocol_from_url(url: &str) -> String {
    if url.contains(".mpd") {
        "dash".to_string()
    } else if is_hls_like(url) {
        "hls".to_string()
    } else {
        "progressive".to_string()
    }
}

fn container_from_url(url: &str) -> String {
    let without_query = url.split('?').next().unwrap_or(url);
    Path::new(without_query)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .filter(|ext| !ext.is_empty())
        .unwrap_or_else(|| "bin".to_string())
}

fn mime_from_extension(extension: &str) -> &'static str {
    match extension {
        "m3u8" => "application/vnd.apple.mpegurl",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mpd" => "application/dash+xml",
        "m4a" => "audio/mp4",
        _ => "application/octet-stream",
    }
}

fn mime_from_urls(split_streams: bool) -> &'static str {
    if split_streams {
        "video/mp4"
    } else {
        "application/octet-stream"
    }
}

fn extract_page_title(html: &str) -> Option<String> {
    let patterns = [
        r#"<meta[^>]+property=[\"']og:title[\"'][^>]+content=[\"']([^\"']+)[\"']"#,
        r#"<meta[^>]+name=[\"']twitter:title[\"'][^>]+content=[\"']([^\"']+)[\"']"#,
        r#"<title>([^<]+)</title>"#,
    ];

    for pattern in patterns {
        let regex = Regex::new(pattern).ok()?;
        if let Some(caps) = regex.captures(html) {
            if let Some(value) = caps.get(1) {
                let cleaned = html_unescape(value.as_str()).trim().to_string();
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
        }
    }

    None
}

fn html_unescape(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", '"'.to_string().as_str())
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn normalize_exposed_media_url(page_url: &Url, raw: &str) -> Option<String> {
    let normalized = html_unescape(raw)
        .replace("\\u002F", "/")
        .replace("\\u002f", "/")
        .replace("\\u003A", ":")
        .replace("\\u003a", ":")
        .replace("\\u0026", "&")
        .replace("\\u0026", "&")
        .replace("\\/", "/")
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();

    if normalized.is_empty() {
        return None;
    }

    let normalized = if normalized.starts_with("//") {
        format!("https:{}", normalized)
    } else {
        normalized
    };

    let resolved = page_url
        .join(&normalized)
        .map(|url| url.to_string())
        .or_else(|_| Url::parse(&normalized).map(|url| url.to_string()))
        .ok()?;

    if is_supported_media_like(&resolved) {
        Some(resolved)
    } else {
        None
    }
}

fn push_generic_candidate(
    page_url: &Url,
    raw: &str,
    quality_label: &str,
    collector: &mut CandidateCollector,
) {
    if let Some(resolved) = normalize_exposed_media_url(page_url, raw) {
        collector.push(
            resolved,
            None,
            None,
            Some(quality_label.to_string()),
            None,
            None,
            None,
            Some("generic"),
        );
    }
}

fn extract_generic_candidates(
    page_url: &Url,
    html: &str,
    collector: &mut CandidateCollector,
) -> Result<()> {
    let absolute_url_regex = Regex::new(r#"https?:\\?/\\?/[^\"'<>\s]+?(?:m3u8|mp4|webm|m4v|m4a|mpd)(?:\?[^\"'<>\s]*)?"#)?;
    let relative_url_regex = Regex::new(r#"(?:src|href|content|data-src|data-url|data-video|data-hls|data-mp4)\s*=\s*[\"']([^\"']+?(?:m3u8|mp4|webm|m4v|m4a|mpd)(?:\?[^\"']*)?)[\"']"#)?;
    let content_url_regex = Regex::new(r#"\"(?:contentUrl|embedUrl|playbackUrl|streamUrl|videoUrl|video_url|playUrl|play_url|hlsUrl|hls_url|dashUrl|dash_url|mp4Url|mp4_url)\"\s*:\s*\"([^\"]+)\""#)?;
    let meta_url_regex = Regex::new(r#"<meta[^>]+(?:property|name)=[\"'](?:og:video|og:video:url|og:video:secure_url|twitter:player:stream|twitter:player:stream:content_type)[\"'][^>]+content=[\"']([^\"']+)[\"']"#)?;
    let source_tag_regex = Regex::new(r#"<source[^>]+src=[\"']([^\"']+?(?:m3u8|mp4|webm|m4v|m4a|mpd)(?:\?[^\"']*)?)[\"']"#)?;

    for capture in absolute_url_regex.captures_iter(html) {
        if let Some(raw) = capture.get(0) {
            push_generic_candidate(page_url, raw.as_str(), "Detected", collector);
        }
    }

    for capture in relative_url_regex.captures_iter(html) {
        if let Some(raw) = capture.get(1) {
            push_generic_candidate(page_url, raw.as_str(), "Detected", collector);
        }
    }

    for capture in content_url_regex.captures_iter(html) {
        if let Some(raw) = capture.get(1) {
            push_generic_candidate(page_url, raw.as_str(), "Content URL", collector);
        }
    }

    for capture in meta_url_regex.captures_iter(html) {
        if let Some(raw) = capture.get(1) {
            push_generic_candidate(page_url, raw.as_str(), "Open Graph", collector);
        }
    }

    for capture in source_tag_regex.captures_iter(html) {
        if let Some(raw) = capture.get(1) {
            push_generic_candidate(page_url, raw.as_str(), "Video Source", collector);
        }
    }

    Ok(())
}

fn extract_bilibili_candidates(
    page_url: &Url,
    html: &str,
    collector: &mut CandidateCollector,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let Some(json) = extract_json_object_after(html, "__playinfo__=") else {
        warnings.push("Bilibili playinfo JSON was not exposed in the page source".to_string());
        return Ok(());
    };

    let value: Value = serde_json::from_str(&json).context("Failed to parse bilibili playinfo JSON")?;
    let title = extract_page_title(html);

    if let Some(durl_list) = value.pointer("/data/durl").and_then(Value::as_array) {
        for (index, item) in durl_list.iter().enumerate() {
            if let Some(media_url) = item.get("url").and_then(Value::as_str) {
                collector.push(
                    media_url.to_string(),
                    None,
                    title.clone(),
                    Some(format!("Part {}", index + 1)),
                    Some("video/mp4".to_string()),
                    None,
                    None,
                    Some("bilibili"),
                );
            }
        }
    }

    let best_audio = value
        .pointer("/data/dash/audio")
        .and_then(Value::as_array)
        .and_then(|audios| {
            audios
                .iter()
                .filter_map(|audio| {
                    Some((
                        audio.get("bandwidth")?.as_i64().unwrap_or_default(),
                        audio.get("baseUrl")
                            .or_else(|| audio.get("base_url"))?
                            .as_str()?
                            .to_string(),
                    ))
                })
                .max_by_key(|entry| entry.0)
                .map(|entry| entry.1)
        });

    if let Some(videos) = value.pointer("/data/dash/video").and_then(Value::as_array) {
        for video in videos {
            let Some(media_url) = video
                .get("baseUrl")
                .or_else(|| video.get("base_url"))
                .and_then(Value::as_str)
            else {
                continue;
            };

            collector.push(
                media_url.to_string(),
                best_audio.clone(),
                title.clone(),
                video
                    .get("height")
                    .and_then(Value::as_i64)
                    .map(|height| format!("{}p", height)),
                video.get("mimeType").and_then(Value::as_str).map(str::to_string),
                video.get("width").and_then(Value::as_i64).map(|value| value as i32),
                video.get("height").and_then(Value::as_i64).map(|value| value as i32),
                Some("bilibili"),
            );
        }
    }

    Ok(())
}

fn extract_youtube_candidates(
    page_url: &Url,
    html: &str,
    collector: &mut CandidateCollector,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let json = extract_json_object_after(html, "ytInitialPlayerResponse = ")
        .or_else(|| extract_json_object_after(html, "var ytInitialPlayerResponse = "));

    let Some(json) = json else {
        warnings.push("YouTube player JSON was not exposed in the page source".to_string());
        return Ok(());
    };

    let value: Value = serde_json::from_str(&json).context("Failed to parse youtube player JSON")?;
    let title = value
        .pointer("/videoDetails/title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| extract_page_title(html))
        .or_else(|| Some(infer_title_from_url(page_url.as_str())));

    if let Some(manifest) = value
        .pointer("/streamingData/hlsManifestUrl")
        .and_then(Value::as_str)
    {
        collector.push(
            manifest.to_string(),
            None,
            title.clone(),
            Some("HLS".to_string()),
            Some("application/vnd.apple.mpegurl".to_string()),
            None,
            None,
            Some("youtube"),
        );
    }

    let best_audio = value
        .pointer("/streamingData/adaptiveFormats")
        .and_then(Value::as_array)
        .and_then(|formats| {
            formats
                .iter()
                .filter(|item| {
                    item.get("mimeType")
                        .and_then(Value::as_str)
                        .map(|mime| mime.starts_with("audio/"))
                        .unwrap_or(false)
                })
                .filter_map(|item| {
                    Some((
                        item.get("bitrate")?.as_i64().unwrap_or_default(),
                        item.get("url")?.as_str()?.to_string(),
                    ))
                })
                .max_by_key(|entry| entry.0)
                .map(|entry| entry.1)
        });

    let mut saw_cipher_only = false;

    for path in ["/streamingData/formats", "/streamingData/adaptiveFormats"] {
        if let Some(formats) = value.pointer(path).and_then(Value::as_array) {
            for item in formats {
                let mime_type = item.get("mimeType").and_then(Value::as_str).unwrap_or_default();
                let Some(media_url) = item.get("url").and_then(Value::as_str) else {
                    if item.get("signatureCipher").is_some() {
                        saw_cipher_only = true;
                    }
                    continue;
                };
                let is_video_only = mime_type.starts_with("video/") && path.ends_with("adaptiveFormats");
                collector.push(
                    media_url.to_string(),
                    if is_video_only { best_audio.clone() } else { None },
                    title.clone(),
                    item.get("qualityLabel").and_then(Value::as_str).map(str::to_string),
                    Some(mime_type.to_string()),
                    item.get("width").and_then(Value::as_i64).map(|value| value as i32),
                    item.get("height").and_then(Value::as_i64).map(|value| value as i32),
                    Some("youtube"),
                );
            }
        }
    }

    if saw_cipher_only {
        warnings.push("Some YouTube streams require signature resolution and were intentionally skipped".to_string());
    }

    Ok(())
}

fn extract_json_object_after(haystack: &str, marker: &str) -> Option<String> {
    let start = haystack.find(marker)? + marker.len();
    let remainder = &haystack[start..];
    let json_start = remainder.find('{')?;
    let bytes = remainder.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut end_index = None;

    for (offset, byte) in bytes.iter().enumerate().skip(json_start) {
        let ch = *byte as char;
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end_index = Some(offset + 1);
                    break;
                }
            }
            _ => {}
        }
    }

    end_index.map(|end| remainder[json_start..end].to_string())
}

async fn download_to_file(
    client: &Client,
    url: &str,
    path: &Path,
    progress: f64,
    sink: &StreamSink<ProgressUpdate>,
    label: &str,
) -> Result<()> {
    let _ = sink.add(ProgressUpdate {
        message: label.to_string(),
        progress,
    });
    let response = client.get(url).send().await?.error_for_status()?;
    let bytes = response.bytes().await?;
    fs::write(path, &bytes)
        .await
        .with_context(|| format!("Failed to write downloaded media: {}", path.display()))?;
    Ok(())
}

async fn merge_media_streams(
    video_path: &Path,
    audio_path: &Path,
    output_path: &str,
    video_bitrate: u32,
    audio_bitrate: u32,
) -> Result<()> {
    #[cfg(target_os = "android")]
    {
        if video_bitrate == 0 && audio_bitrate == 0 {
            if let Some(transcoder) = ANDROID_HW_TRANSCODER.get() {
                match transcoder
                    .mux(
                        video_path.to_string_lossy().as_ref(),
                        audio_path.to_string_lossy().as_ref(),
                        output_path,
                    )
                    .await
                {
                    Ok(_) => return Ok(()),
                    Err(e) => warn!("Android MediaMuxer merge failed, falling back if possible: {}", e),
                }
            }
        }
    }

    if !check_ffmpeg().await {
        bail!("FFmpeg is required to merge separated audio and video streams");
    }

    let mut args = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-i".to_string(),
        video_path.to_string_lossy().to_string(),
        "-i".to_string(),
        audio_path.to_string_lossy().to_string(),
    ];

    if video_bitrate == 0 && audio_bitrate == 0 {
        args.extend(["-c".to_string(), "copy".to_string()]);
    } else {
        args.extend([
            "-c:v".to_string(),
            "libx264".to_string(),
            "-c:a".to_string(),
            "aac".to_string(),
        ]);
        if video_bitrate > 0 {
            args.push("-b:v".to_string());
            args.push(format!("{}k", video_bitrate));
        }
        if audio_bitrate > 0 {
            args.push("-b:a".to_string());
            args.push(format!("{}k", audio_bitrate));
        }
    }

    args.push(output_path.to_string());

    let output = Command::new("ffmpeg")
        .args(&args)
        .output()
        .await
        .context("FFmpeg merge failed")?;

    if !output.status.success() {
        bail!("FFmpeg merge failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(())
}

fn detect_access_challenge(status: u16, body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    if status == 401 || status == 403 {
        return Some("The server requires authorization. Import cookies or headers from an account that is allowed to access this resource.".to_string());
    }
    if status == 429 {
        return Some("The server is rate limiting this client. Wait and retry with normal authorized traffic; this app will not bypass rate limits.".to_string());
    }
    if status == 503 && (lower.contains("cloudflare") || lower.contains("checking your browser")) {
        return Some("A browser verification page was detected. Complete verification in a browser, then import the resulting authorized cookies if you are allowed to access the content.".to_string());
    }
    if lower.contains("cf-chl-")
        || lower.contains("turnstile")
        || lower.contains("g-recaptcha")
        || lower.contains("hcaptcha")
        || lower.contains("captcha")
        || lower.contains("checking if the site connection is secure")
    {
        return Some("A human verification challenge was detected. Complete it in a browser and import the authorized session cookies; automated bypass is not supported.".to_string());
    }
    None
}

async fn score_candidates(
    mut candidates: Vec<MediaCandidate>,
    request_context: &RequestContext,
) -> Vec<MediaCandidate> {
    for candidate in &mut candidates {
        let mut score = 0;
        let mut reasons = Vec::new();
        let lower_url = candidate.media_url.to_ascii_lowercase();
        let lower_quality = candidate.quality_label.to_ascii_lowercase();

        if candidate.protocol == "hls" {
            score += 300;
            reasons.push("hls".to_string());
            if let Ok((segments, duration)) = inspect_hls_metadata(&candidate.media_url, request_context).await {
                candidate.segment_count = segments as i32;
                candidate.duration_seconds = duration;
                score += (duration.min(7200.0) / 6.0) as i32;
                score += (segments.min(2000) / 4) as i32;
                if duration >= 60.0 {
                    reasons.push(format!("{:.0}s", duration));
                }
            }
        }

        if candidate.protocol == "progressive" {
            score += 120;
            reasons.push("direct".to_string());
        }
        if candidate.audio_url.is_some() {
            score += 80;
            reasons.push("audio+video".to_string());
        }
        if candidate.height > 0 {
            score += candidate.height.min(2160) / 3;
            reasons.push(format!("{}p", candidate.height));
        }
        if candidate.width > 0 {
            score += candidate.width.min(3840) / 24;
        }
        if lower_quality.contains("1080") || lower_quality.contains("720") || lower_quality.contains("高") {
            score += 120;
        }
        for marker in ["preview", "试看", "trial", "sample", "ad", "ads", "promo", "trailer", "thumb", "sprite", "teaser"] {
            if lower_url.contains(marker) || lower_quality.contains(marker) {
                score -= 500;
                reasons.push(format!("deprioritized:{}", marker));
            }
        }
        if lower_url.contains("master") || lower_url.contains("index") || lower_url.contains("playlist") {
            score += 40;
        }
        candidate.score = score;
        candidate.reason = if reasons.is_empty() { "ranked by available metadata".to_string() } else { reasons.join(", ") };
    }

    candidates.sort_by(|left, right| {
        let left_score = (left.score, left.height.max(0), left.duration_seconds as i64, left.segment_count);
        let right_score = (right.score, right.height.max(0), right.duration_seconds as i64, right.segment_count);
        right_score.cmp(&left_score)
    });
    if let Some(first) = candidates.first_mut() {
        first.primary = true;
    }
    candidates
}

fn retry_backoff_delay(attempt: u8, status: Option<u16>) -> Duration {
    let base_ms = match status {
        Some(429) => 3500,
        Some(503) => 2800,
        Some(403) => 2200,
        _ => 1200,
    };
    let exponent = u32::from(attempt.saturating_sub(1).min(4));
    Duration::from_millis(base_ms * (1u64 << exponent))
}

async fn inspect_hls_metadata(url: &str, request_context: &RequestContext) -> Result<(usize, f64)> {
    let bytes = download_playlist(url, request_context).await?;
    let (_, playlist) = parse_playlist(&bytes).map_err(|e| anyhow!("Failed to parse HLS metadata: {:?}", e))?;
    match playlist {
        Playlist::MediaPlaylist(media) => {
            let duration = media.segments.iter().map(|segment| segment.duration as f64).sum();
            Ok((media.segments.len(), duration))
        }
        Playlist::MasterPlaylist(master) => Ok((master.variants.len(), 0.0)),
    }
}

fn create_http_client_for_context(source_url: Option<&str>, request_context: &RequestContext) -> Result<Client> {
    let mut headers = header::HeaderMap::new();
    let user_agent = if request_context.user_agent.trim().is_empty() {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"
    } else {
        request_context.user_agent.trim()
    };
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_str(user_agent)?,
    );
    headers.insert(header::ACCEPT, header::HeaderValue::from_static("*/*"));
    headers.insert(
        header::ACCEPT_LANGUAGE,
        header::HeaderValue::from_static("en-US,en;q=0.9,zh-CN;q=0.8"),
    );

    if let Some(source_url) = source_url {
        if let Ok(parsed_url) = Url::parse(source_url) {
            if let Some(domain) = parsed_url.domain() {
                let default_referer = format!("https://{}/", domain);
                let default_origin = format!("https://{}", domain);
                let referer = if request_context.referer.trim().is_empty() { default_referer.as_str() } else { request_context.referer.trim() };
                let origin = if request_context.origin.trim().is_empty() { default_origin.as_str() } else { request_context.origin.trim() };
                headers.insert(header::REFERER, header::HeaderValue::from_str(referer)?);
                headers.insert(header::ORIGIN, header::HeaderValue::from_str(origin)?);
            }
        }
    }

    if !request_context.cookie.trim().is_empty() {
        headers.insert(header::COOKIE, header::HeaderValue::from_str(request_context.cookie.trim())?);
    }

    for entry in &request_context.headers {
        let name = entry.name.trim();
        let value = entry.value.trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        let header_name = header::HeaderName::from_bytes(name.as_bytes())?;
        headers.insert(header_name, header::HeaderValue::from_str(value)?);
    }

    Ok(Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(45))
        .build()?)
}

async fn download_playlist(url: &str, request_context: &RequestContext) -> Result<Vec<u8>> {
    let client = create_http_client_for_context(Some(url), request_context)?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        bail!("Failed to download playlist: HTTP {}", response.status());
    }

    Ok(response.bytes().await?.to_vec())
}

async fn check_ffmpeg() -> bool {
    match Command::new("ffmpeg").arg("-version").output().await {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

async fn select_transcoder_backend() -> Result<TranscoderKind> {
    if check_ffmpeg().await {
        let accel = detect_acceleration().await.unwrap_or(AccelType::CPU);
        return Ok(TranscoderKind::Ffmpeg(accel));
    }

    if cfg!(target_os = "android") {
        #[cfg(target_os = "android")]
        {
            if ANDROID_HW_TRANSCODER.get().is_none() {
                bail!(
                    "Android MediaCodec transcoder not registered. Ensure System.loadLibrary(\"rust_lib_m3u8_downloader\") 
                    is called in your Android app before using this library."
                );
            }
        }
        return Ok(TranscoderKind::AndroidHardware);
    }

    bail!("FFmpeg not found and not running on Android; no available transcoder");
}

async fn download_and_merge(
    playlist: m3u8_rs::MediaPlaylist,
    base_url: Option<Url>,
    concurrency: usize,
    retries: u8,
    output_file: &str,
    temp_dir: &Path,
    multi_progress: &MultiProgress,
    sink: StreamSink<ProgressUpdate>,
    request_context: RequestContext,
) -> Result<()> {
    if !temp_dir.exists() {
        std::fs::create_dir_all(temp_dir)
            .with_context(|| format!("Failed to create temp dir: {}", temp_dir.display()))?;
    }

    let segments = playlist.segments;
    let total = segments.len();
    if total == 0 {
        bail!("MediaPlaylist contains no segments");
    }

    let download_pb = multi_progress.add(ProgressBar::new(total as u64));
    download_pb.set_style(
        ProgressStyle::with_template(
            "{msg} [{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} ({percent}%) {eta}",
        )?
        .progress_chars("##-"),
    );
    download_pb.set_message("Downloading segments");

    let key: Option<(Vec<u8>, Vec<u8>)> = {
        if let Some(first_seg) = segments.first() {
            if let Some(ref key_def) = first_seg.key {
                let key_uri = key_def
                    .uri
                    .clone()
                    .ok_or_else(|| anyhow!("Found encrypted stream but key.uri is empty"))?;
                let key_url = if let Some(base) = &base_url {
                    base.join(&key_uri)?
                } else {
                    Url::parse(&key_uri)?
                };
                let client = create_http_client_for_context(Some(key_url.as_str()), &request_context)?;
                let resp = client.get(key_url).send().await?.error_for_status()?;
                let key_bytes = resp.bytes().await?.to_vec();

                let iv_bytes = if let Some(iv_hex) = &key_def.iv {
                    hex::decode(iv_hex.trim_start_matches("0x")).context("IV hex decode failed")?
                } else {
                    bail!("AES-128 encrypted stream but IV not provided");
                };

                Some((key_bytes, iv_bytes))
            } else {
                None
            }
        } else {
            None
        }
    };

    let sem = Arc::new(Semaphore::new(concurrency));
    let client = Arc::new(create_http_client_for_context(base_url.as_ref().map(Url::as_str), &request_context)?);
    let completed = Arc::new(Mutex::new(0u64));

    let temp_dir = temp_dir.to_path_buf();

    let tasks = stream::iter(segments.into_iter().enumerate())
        .map(|(idx, seg)| {
            let seg_url = if let Some(base) = &base_url {
                base.join(&seg.uri).unwrap().to_string()
            } else {
                seg.uri.clone()
            };

            let client = client.clone();
            let sem = sem.clone();
            let key = key.clone();
            let pb = download_pb.clone();
            let completed = completed.clone();
            let sink = sink.clone();
            let temp_dir = temp_dir.clone();

            tokio::spawn(async move {
                let _permit = sem
                    .acquire()
                    .await
                    .map_err(|_| anyhow!("Semaphore acquire failed"))?;

                for attempt in 1..=retries {
                    match client.get(&seg_url).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            let data = resp.bytes().await?;
                            let buf = if let Some((ref k, ref iv)) = key {
                                if iv.len() != 16 {
                                    bail!("IV length is not 16 bytes");
                                }
                                let cipher = Aes128Cbc::new_from_slices(k, iv)?;
                                cipher.decrypt_vec(&data)?
                            } else {
                                data.to_vec()
                            };

                            let file_name = format!("seg_{:05}.ts", idx);
                            let tmp_path = temp_dir.join(file_name);
                            fs::write(&tmp_path, &buf).await.with_context(|| {
                                format!(
                                    "Failed to write segment: {} (url: {})",
                                    tmp_path.display(),
                                    seg_url
                                )
                            })?;

                            let mut count = completed.lock().await;
                            *count += 1;
                            pb.set_position(*count);
                            pb.set_message(format!("Downloading segments [{}/{}]", *count, total));
                            let _ = sink.add(ProgressUpdate {
                                message: format!("Downloading segments [{}/{}]", *count, total),
                                progress: (*count as f64) / (total as f64) * 0.9,
                            });

                            return Ok::<(), anyhow::Error>(());
                        }

                        Ok(r) => {
                            pb.set_message(format!("Retrying... ({}/{})", attempt, retries));
                            warn!(
                                "Attempt {} failed: {} HTTP {}",
                                attempt,
                                seg_url,
                                r.status()
                            );
                            if attempt < retries {
                                let delay = retry_backoff_delay(attempt, Some(r.status().as_u16()));
                                tokio::time::sleep(delay).await;
                            }
                        }

                        Err(e) => {
                            pb.set_message(format!("Retrying... ({}/{})", attempt, retries));
                            warn!("Attempt {} request error: {} - {}", attempt, seg_url, e);
                            if attempt < retries {
                                let delay = retry_backoff_delay(attempt, None);
                                tokio::time::sleep(delay).await;
                            }
                        }
                    }
                }

                bail!("Failed after {} attempts: {}", retries, seg_url)
            })
        })
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;

    for task in tasks {
        task??;
    }

    download_pb.finish_with_message("All segments downloaded");

    let merge_pb = multi_progress.add(ProgressBar::new(total as u64));
    merge_pb.set_style(
        ProgressStyle::with_template(
            "{msg} [{elapsed_precise}] {bar:40.green} {pos:>7}/{len:7} ({percent}%)",
        )?
        .progress_chars("##-"),
    );
    merge_pb.set_message("Merging segments");

    let mut output = fs::File::create(output_file)
        .await
        .with_context(|| format!("Failed to create output TS file: {}", output_file))?;

    for i in 0..total {
        let file_name = format!("seg_{:05}.ts", i);
        let tmp_path = temp_dir.join(&file_name);

        let mut segment = fs::File::open(&tmp_path)
            .await
            .with_context(|| format!("Failed to read segment: {}", tmp_path.display()))?;
        
        tokio::io::copy(&mut segment, &mut output)
            .await
            .with_context(|| format!("Failed to write to output TS: {}", output_file))?;

        let _ = fs::remove_file(&tmp_path).await;
        merge_pb.inc(1);
        merge_pb.set_message(format!("Merging segments [{}/{}]", i + 1, total));
    }

    merge_pb.finish_with_message("Merge complete");
    Ok(())
}

fn create_http_client() -> Result<Client> {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
        ),
    );
    headers.insert(header::ACCEPT, header::HeaderValue::from_static("*/*"));

    Ok(Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .build()?)
}

async fn detect_acceleration() -> Result<AccelType> {
    let output = Command::new("ffmpeg")
        .args(&["-hide_banner", "-encoders"])
        .output()
        .await
        .context("Failed to run ffmpeg")?;

    let list = String::from_utf8_lossy(&output.stdout);
    if list.contains("h264_nvenc") {
        Ok(AccelType::Nvidia)
    } else if list.contains("h264_amf") {
        Ok(AccelType::AMD)
    } else {
        Ok(AccelType::CPU)
    }
}

async fn convert_to_mp4(
    input_ts: &str,
    output_path: &str,
    video_bitrate: u32,
    audio_bitrate: u32,
    multi_progress: &MultiProgress,
    backend: TranscoderKind,
    sink: StreamSink<ProgressUpdate>,
) -> Result<()> {
    let convert_pb = multi_progress.add(ProgressBar::new_spinner());
    convert_pb.set_style(
        ProgressStyle::with_template("{spinner:.yellow} {msg}")?
            .tick_strings(&["-", "\\", "|", "/"]),
    );
    convert_pb.set_message("Converting to MP4...");
    convert_pb.enable_steady_tick(Duration::from_millis(120));

    let _ = sink.add(ProgressUpdate {
        message: "Converting to MP4...".to_string(),
        progress: 0.95,
    });

    match backend {
        TranscoderKind::Ffmpeg(accel) => {
            info!("Using FFmpeg backend: {:?}", accel);
            let mut ffmpeg_args: Vec<String> = vec![
                "-y".to_string(),
                "-hide_banner".to_string(),
                "-loglevel".to_string(),
                "info".to_string(),
            ];

            if video_bitrate == 0 && audio_bitrate == 0 {
                info!("Bitrates are 0, attempting to remux (copy streams) for high efficiency");
                ffmpeg_args.extend([
                    "-i".to_string(),
                    input_ts.to_string(),
                    "-c".to_string(),
                    "copy".to_string(),
                    "-bsf:a".to_string(),
                    "aac_adtstoasc".to_string(),
                ]);
            } else {
                match accel {
                    AccelType::Nvidia => {
                        info!("Detected NVIDIA GPU, using NVENC");
                        ffmpeg_args.extend([
                            "-hwaccel".to_string(), "cuda".to_string(),
                            "-hwaccel_output_format".to_string(), "cuda".to_string(),
                            "-c:v".to_string(), "h264_cuvid".to_string(),
                            "-i".to_string(), input_ts.to_string(),
                            "-c:a".to_string(), "aac".to_string(), "-b:a".to_string(), "320k".to_string(),
                            "-c:v".to_string(), "h264_nvenc".to_string(), "-preset".to_string(), "p3".to_string(), "-rc".to_string(), "vbr".to_string(),
                        ]);
                    }
                    AccelType::AMD => {
                        info!("Detected AMD GPU, using AMF");
                        ffmpeg_args.extend([
                            "-i".to_string(), input_ts.to_string(),
                            "-c:a".to_string(), "aac".to_string(), "-b:a".to_string(), "320k".to_string(),
                            "-c:v".to_string(), "h264_amf".to_string(), "-rc".to_string(), "vbr".to_string(),
                        ]);
                    }
                    AccelType::CPU => {
                        info!("No supported GPU found, using CPU (libx264)");
                        ffmpeg_args.extend([
                            "-i".to_string(), input_ts.to_string(),
                            "-c:a".to_string(), "aac".to_string(),
                            "-c:v".to_string(), "libx264".to_string(), "-preset".to_string(), "medium".to_string(),
                        ]);
                    }
                }

                if video_bitrate > 0 {
                    ffmpeg_args.push("-b:v".to_string());
                    ffmpeg_args.push(format!("{}k", video_bitrate));
                }

                if audio_bitrate > 0 {
                    ffmpeg_args.push("-b:a".to_string());
                    ffmpeg_args.push(format!("{}k", audio_bitrate));
                } else {
                    ffmpeg_args.push("-b:a".to_string());
                    ffmpeg_args.push("256k".to_string());
                }
            }

            ffmpeg_args.push(output_path.to_string());

            let output = Command::new("ffmpeg")
                .args(&ffmpeg_args)
                .output()
                .await
                .context("FFmpeg transcode failed")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                convert_pb.finish_with_message("MP4 transcode failed");
                error!("FFmpeg stderr:\n{}", stderr);
                bail!("MP4 transcode failed");
            }

            convert_pb.finish_with_message("MP4 transcode complete");
            info!("Output file: {}", output_path);

            let out_meta = std::fs::metadata(output_path)
                .context("Transcode output file not found after FFmpeg")?;
            if out_meta.len() < 1024 {
                bail!("Transcode output file is too small ({} bytes), likely corrupted", out_meta.len());
            }

            Ok(())
        }
        TranscoderKind::AndroidHardware => {
            info!("Using Android MediaCodec hardware transcoder");
            android_hardware_transcode(
                input_ts,
                output_path,
                video_bitrate,
                audio_bitrate,
                &convert_pb,
            )
            .await?;
            convert_pb.finish_with_message("Android hardware transcode complete");
            info!("Output file: {}", output_path);

            let out_meta = std::fs::metadata(output_path)
                .context("Transcode output file not found after Android hardware transcode")?;
            if out_meta.len() < 1024 {
                bail!("Transcode output file is too small ({} bytes), likely corrupted", out_meta.len());
            }

            Ok(())
        }
    }
}

async fn android_hardware_transcode(
    input_ts: &str,
    output_mp4: &str,
    video_bitrate: u32,
    audio_bitrate: u32,
    _pb: &ProgressBar,
) -> Result<()> {
    #[cfg(target_os = "android")]
    {
        let transcoder = ANDROID_HW_TRANSCODER.get().ok_or_else(|| {
            error!("   CRITICAL: Android MediaCodec transcoder not registered!");
            error!("   This means JNI_OnLoad was not executed by Android runtime.");
            error!("   Possible causes:");
            error!("   1. Your Rust library (.so) was not loaded as a JNI library");
            error!("   2. Cargo.toml [lib] crate-type is not [\"cdylib\"]");
            error!("   3. Check logcat for any JNI loading errors");
            anyhow!("Android MediaCodec transcoder not registered; JNI_OnLoad failed")
        })?;

        transcoder
            .transcode(input_ts, output_mp4, video_bitrate, audio_bitrate)
            .await
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = input_ts;
        let _ = output_mp4;
        let _ = video_bitrate;
        let _ = audio_bitrate;
        bail!("Android hardware transcoding is only available on Android");
    }
}

#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();

    #[cfg(target_os = "android")]
    {
        match init_android_transcoder_check() {
            Ok(msg) => {
                info!("   ℹ️ {}", msg);
            }
            Err(e) => {
                warn!("Transcoder check result: {}", e);
            }
        }
    }
}

#[flutter_rust_bridge::frb()]
#[cfg(target_os = "android")]
pub fn init_android_transcoder_check() -> Result<String> {
    if ANDROID_HW_TRANSCODER.get().is_some() {
        return Ok("Android MediaCodec transcoder is registered".to_string());
    }

    Err(anyhow!(
        "Android MediaCodec transcoder not registered. Make sure System.loadLibrary(\"rust_lib_m3u8_downloader\") 
        is called in your Android code so that JNI_OnLoad runs."
    ))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn JNI_OnLoad(
    vm: *mut jni::sys::JavaVM,
    _reserved: *mut std::os::raw::c_void,
) -> i32 {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    let jvm = match unsafe { jni::JavaVM::from_raw(vm) } {
        Ok(vm) => Arc::new(vm),
        Err(e) => {
            eprintln!("Failed to create JavaVM in JNI_OnLoad: {:?}", e);
            return jni::sys::JNI_VERSION_1_6 as i32;
        }
    };

    match register_android_mediacodec_transcoder(jvm.clone()) {
        Ok(_) => {
            info!("Android MediaCodec transcoder registered successfully in JNI_OnLoad");
        }
        Err(e) => {
            error!("Failed to register transcoder in JNI_OnLoad: {}", e);
        }
    }

    match jvm.attach_current_thread() {
        Ok(mut env) => {
            // NOTE: We can NOT cache MediaTranscoder class here because JNI_OnLoad runs
            // during System.loadLibrary before the app classloader has loaded the class.
            // The class will be cached later via registerMediaTranscoderClass() called from Kotlin.

            if let Ok(thread_class) = env.find_class("android/app/ActivityThread") {
                if let Ok(app_obj) = env.call_static_method(
                    thread_class,
                    "currentApplication",
                    "()Landroid/app/Application;",
                    &[],
                ) {
                    if let Ok(app) = app_obj.l() {
                        if !app.is_null() {
                            if let Ok(global) = env.new_global_ref(app) {
                                if let Err(e) = init_android_context(jvm.clone(), global) {
                                    warn!("⚠️ Failed to init Android Context: {}", e);
                                }
                            }
                        } else {
                            info!("ℹ️ currentApplication() returned null");
                        }
                    }
                }
            }
        }
        Err(e) => {
            warn!("⚠️ Failed to attach thread in JNI_OnLoad: {}", e);
        }
    }

    jni::sys::JNI_VERSION_1_6 as i32
}

/*
#[flutter_rust_bridge::frb()]
#[cfg(target_os = "android")]
pub fn init_android_context_from_dart(jvm_ptr: i64, context_ptr: i64) -> Result<()> {
    use jni::objects::JObject;
    use jni::sys::jobject;

    let jvm = unsafe { jni::JavaVM::from_raw(jvm_ptr as *mut jni::sys::JavaVM) }?;
    let jvm = Arc::new(jvm);

    let global_context = {
        let mut env = jvm.attach_current_thread()?;
        let context_obj = unsafe { JObject::from_raw(context_ptr as jobject) };
        env.new_global_ref(context_obj)?
    };

    init_android_context(jvm, global_context)?;
    info!("鉁� Android Context initialized from Dart");
    Ok(())
}
*/