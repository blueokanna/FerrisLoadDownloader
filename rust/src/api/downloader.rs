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
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(target_os = "android")]
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::Duration;
#[cfg(target_os = "android")]
use std::env;
use tokio::sync::Semaphore;
use tokio::{fs, process::Command, sync::Mutex};
use url::Url;
use crate::frb_generated::StreamSink;
use crate::api::site_adapters::{extractor_name_for_host, inspect_page_candidates, SiteWarning};

#[cfg(target_os = "android")]
use jni::objects::{GlobalRef, JClass, JObject, JValue};
#[cfg(target_os = "android")]
use jni::JNIEnv;
#[cfg(target_os = "android")]
use jni::JavaVM;

type Aes128Cbc = Cbc<Aes128, Pkcs7>;
pub(crate) type ProgressReporter = Arc<dyn Fn(ProgressUpdate) + Send + Sync>;

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

pub(crate) fn sink_progress_reporter(sink: StreamSink<ProgressUpdate>) -> ProgressReporter {
    Arc::new(move |update| {
        let _ = sink.add(update);
    })
}

pub(crate) fn noop_progress_reporter() -> ProgressReporter {
    Arc::new(|_| {})
}

pub(crate) fn emit_progress(reporter: &ProgressReporter, message: impl Into<String>, progress: f64) {
    reporter(ProgressUpdate {
        message: message.into(),
        progress,
    });
}

#[cfg(target_os = "android")]
macro_rules! jni_string {
    ($env:expr, $value:expr, $label:literal) => {
        $env.new_string($value)
            .map_err(|e| anyhow!("JNI new_string failed for {}: {}", $label, e))?
    };
}

#[cfg(target_os = "android")]
static ANDROID_HW_TRANSCODER: OnceLock<Arc<AndroidMediaCodecTranscoder>> = OnceLock::new();

#[cfg(target_os = "android")]
static ANDROID_TRANSCODE_GATE: OnceLock<Arc<StdMutex<()>>> = OnceLock::new();

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

    async fn with_media_transcoder<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut JNIEnv, JClass) -> Result<T> + Send + 'static,
    {
        let jvm = self.jvm.clone();
        let gate = ANDROID_TRANSCODE_GATE
            .get_or_init(|| Arc::new(StdMutex::new(())))
            .clone();
        tokio::task::spawn_blocking(move || {
            let _guard = gate
                .lock()
                .map_err(|_| anyhow!("Android transcode gate acquire failed"))?;
            let mut env = jvm
                .attach_current_thread()
                .map_err(|e| anyhow!("JNI attach thread failed: {}", e))?;
            let class = media_transcoder_class(&mut env)?;
            operation(&mut env, class)
        })
        .await
        .map_err(|e| anyhow!("tokio spawn_blocking failed: {}", e))?
    }

    pub async fn transcode(
        &self,
        input_ts: &str,
        output_mp4: &str,
        video_bitrate: u32,
        audio_bitrate: u32,
    ) -> Result<()> {
        let input_ts = input_ts.to_string();
        let output_mp4 = output_mp4.to_string();

        self.with_media_transcoder(move |env, class| {
            let input_ts_jstring = jni_string!(env, &input_ts, "input_ts");
            let output_mp4_jstring = jni_string!(env, &output_mp4, "output_mp4");

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
    }

    pub async fn mux(
        &self,
        video_path: &str,
        audio_path: &str,
        output_mp4: &str,
    ) -> Result<()> {
        let video_path = video_path.to_string();
        let audio_path = audio_path.to_string();
        let output_mp4 = output_mp4.to_string();

        self.with_media_transcoder(move |env, class| {
            let video_jstring = jni_string!(env, &video_path, "video_path");
            let audio_jstring = jni_string!(env, &audio_path, "audio_path");
            let output_jstring = jni_string!(env, &output_mp4, "output_mp4");

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
    }
}

#[cfg(target_os = "android")]
fn media_transcoder_class<'local>(env: &mut JNIEnv<'local>) -> Result<JClass<'local>> {
    if let Some(class_ref) = MEDIA_TRANSCODER_CLASS.get() {
        let local_ref = env
            .new_local_ref(class_ref.as_obj())
            .map_err(|e| anyhow!("Failed to create local MediaTranscoder ref: {}", e))?;
        return Ok(JClass::from(local_ref));
    }

    let ctx = get_android_context().map_err(|e| anyhow!("Failed to get Android context: {}", e))?;
    let class_loader = env
        .call_method(
            ctx.app_context.as_obj(),
            "getClassLoader",
            "()Ljava/lang/ClassLoader;",
            &[],
        )
        .map_err(|e| anyhow!("Failed to get ClassLoader: {:?}", e))?
        .l()
        .map_err(|e| anyhow!("ClassLoader is not an object: {:?}", e))?;
    let class_name = jni_string!(env, "com.bluevale.m3u8_downloader.MediaTranscoder", "class_name");
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

    Ok(JClass::from(loaded_class))
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

    info!("Android MediaCodec transcoder registered");
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
    hls2mp4_core(
        sink_progress_reporter(sink),
        url,
        concurrency,
        output,
        retries,
        video_bitrate,
        audio_bitrate,
        keep_temp,
    )
    .await
}

pub(crate) async fn hls2mp4_core(
    reporter: ProgressReporter,
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
        reporter,
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
        warnings.push(SiteWarning::auth("challenge-detected", reason.clone()));
        return Ok(MediaInspectionResult {
            page_url: url,
            page_title: crate::api::site_adapters::extract_page_title(&html)
                .unwrap_or_else(|| "Authorization required".to_string()),
            extractor: extractor_name_for_host(page_url.domain()),
            candidates: Vec::new(),
            warnings: warnings.into_iter().map(SiteWarning::into_display).collect(),
            auth_required: true,
            challenge_reason: reason,
        });
    }
    if !status.is_success() {
        bail!("Failed to inspect page: HTTP {}", status);
    }
    let page_title = crate::api::site_adapters::extract_page_title(&html)
        .unwrap_or_else(|| infer_title_from_url(&url));
    let extractor = extractor_name_for_host(page_url.domain());
    let mut collector = CandidateCollector::new(page_url.as_str(), &page_title, &extractor);

    inspect_page_candidates(&page_url, &html, &mut collector, &mut warnings)?;
    let candidates = score_candidates(collector.finish(), &request_context).await;

    if candidates.is_empty() {
        if let Some(auth_warning) = warnings.iter().find(|warning| warning.scope() == "auth") {
            let challenge_reason = auth_warning.message().to_string();
            return Ok(MediaInspectionResult {
                page_url: url,
                page_title,
                extractor,
                candidates,
                warnings: warnings.into_iter().map(SiteWarning::into_display).collect(),
                auth_required: true,
                challenge_reason,
            });
        }
    }

    if candidates.is_empty() {
        warnings.push(SiteWarning::media(
            "no-candidates",
            "No downloadable media candidates were found in the current page source",
        ));
    }

    Ok(MediaInspectionResult {
        page_url: url,
        page_title,
        extractor,
        candidates,
        warnings: warnings.into_iter().map(SiteWarning::into_display).collect(),
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
    download_media_with_context_core(
        sink_progress_reporter(sink),
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
    download_media_with_context_core(
        sink_progress_reporter(sink),
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
    .await
}

pub(crate) async fn download_media_with_context_core(
    reporter: ProgressReporter,
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

    if should_auto_inspect_download_target(&page_url, &media_url, audio_url.as_deref()) {
        emit_progress(&reporter, "Resolving page media candidate...", 0.01);

        let inspection = inspect_media_with_context(page_url.clone(), request_context.clone()).await?;
        if inspection.auth_required {
            let reason = if inspection.challenge_reason.trim().is_empty() {
                "Authorization required before download".to_string()
            } else {
                inspection.challenge_reason.clone()
            };
            bail!("Authorization required before download: {}", reason);
        }

        let selected_candidate = inspection
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("No downloadable media candidates were found for the page URL"))?;

        if selected_candidate.media_url == media_url && selected_candidate.audio_url == audio_url {
            bail!("Resolved page candidate did not expose a downloadable media stream");
        }

        return Box::pin(download_media_with_context_core(
            reporter,
            selected_candidate.page_url,
            selected_candidate.media_url,
            selected_candidate.audio_url,
            output,
            concurrency,
            retries,
            video_bitrate,
            audio_bitrate,
            keep_temp,
            request_context,
        ))
        .await;
    }

    if is_hls_like(&media_url) {
        return run_hls_pipeline(
            reporter,
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

    if protocol_from_url(&media_url) == "dash" {
        return run_dash_pipeline(
            reporter,
            &media_url,
            &request_context,
            &output,
            retries,
            video_bitrate,
            audio_bitrate,
            keep_temp,
        )
        .await;
    }

    emit_progress(&reporter, "Preparing direct media download...", 0.02);

    let page_url = Url::parse(&page_url).ok();
    let client = create_http_client_for_context(page_url.as_ref().map(Url::as_str), &request_context)?;
    let (output_path, temp_dir) = prepare_output_path(&output)?;

    match audio_url {
        Some(audio) => {
            let video_temp = temp_dir.join("stream_video_input.bin");
            let audio_temp = temp_dir.join("stream_audio_input.bin");

            download_to_file(&client, &media_url, &video_temp, 0.42, &reporter, "Downloading video stream")
                .await?;
            download_to_file(&client, &audio, &audio_temp, 0.78, &reporter, "Downloading audio stream")
                .await?;

            merge_media_streams(
                &video_temp,
                &audio_temp,
                &output,
                video_bitrate.max(0) as u32,
                audio_bitrate.max(0) as u32,
            )
            .await?;

            cleanup_temp_files(keep_temp, [&video_temp, &audio_temp]).await;
        }
        None => {
            let extension = container_from_url(&media_url);
            if extension == "mp4" {
                download_to_file(&client, &media_url, &output_path, 0.95, &reporter, "Downloading media")
                    .await?;
            } else {
                let temp_input = temp_dir.join(format!("direct_input.{}", extension));
                download_to_file(&client, &media_url, &temp_input, 0.72, &reporter, "Downloading media")
                    .await?;
                transcode_input_to_output(
                    &temp_input,
                    &output,
                    video_bitrate.max(0) as u32,
                    audio_bitrate.max(0) as u32,
                    reporter.clone(),
                )
                .await?;
                cleanup_temp_files(keep_temp, [&temp_input]).await;
            }
        }
    }

    ensure_output_file_ready(&output_path)?;
    emit_progress(&reporter, "All tasks completed", 1.0);

    Ok(())
}

async fn run_dash_pipeline(
    reporter: ProgressReporter,
    manifest_url: &str,
    request_context: &RequestContext,
    output: &str,
    retries: i32,
    video_bitrate: i32,
    audio_bitrate: i32,
    keep_temp: bool,
) -> Result<()> {
    emit_progress(&reporter, "Resolving DASH manifest...", 0.02);

    let plan = resolve_dash_download_plan(manifest_url, request_context).await?;
    let (output_path, temp_dir) = prepare_output_path(output)?;

    let client = create_http_client_for_context(Some(&plan.video_url), request_context)?;
    let retries = retries.max(1) as u8;

    let video_extension = container_from_url(&plan.video_url);
    let video_temp = temp_dir.join(format!("dash_video_input.{}", video_extension));
    download_with_retries(
        &client,
        &plan.video_url,
        &video_temp,
        retries,
        0.46,
        &reporter,
        "Downloading DASH video",
    )
    .await?;

    match plan.audio_url {
        Some(audio_url) => {
            let audio_extension = container_from_url(&audio_url);
            let audio_temp = temp_dir.join(format!("dash_audio_input.{}", audio_extension));

            download_with_retries(
                &client,
                &audio_url,
                &audio_temp,
                retries,
                0.78,
                &reporter,
                "Downloading DASH audio",
            )
            .await?;

            merge_media_streams(
                &video_temp,
                &audio_temp,
                output,
                video_bitrate.max(0) as u32,
                audio_bitrate.max(0) as u32,
            )
            .await?;

            cleanup_temp_files(keep_temp, [&video_temp, &audio_temp]).await;
        }
        None => {
            if video_extension == "mp4" && video_bitrate <= 0 && audio_bitrate <= 0 {
                if fs::rename(&video_temp, &output_path).await.is_err() {
                    fs::copy(&video_temp, &output_path)
                        .await
                        .with_context(|| format!("Failed to copy DASH output to {}", output_path.display()))?;
                }
                cleanup_temp_files(keep_temp, [&video_temp]).await;
            } else {
                transcode_input_to_output(
                    &video_temp,
                    output,
                    video_bitrate.max(0) as u32,
                    audio_bitrate.max(0) as u32,
                    reporter.clone(),
                )
                .await?;
                cleanup_temp_files(keep_temp, [&video_temp]).await;
            }
        }
    }

    ensure_output_file_ready(&output_path)?;
    emit_progress(&reporter, "All tasks completed", 1.0);
    Ok(())
}

pub(crate) async fn run_hls_pipeline(
    reporter: ProgressReporter,
    url: &str,
    request_context: &RequestContext,
    concurrency: i32,
    output: &str,
    retries: i32,
    video_bitrate: i32,
    audio_bitrate: i32,
    keep_temp: bool,
) -> Result<()> {
    emit_progress(&reporter, "Initializing...", 0.0);

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

    emit_progress(&reporter, "Selecting transcoder backend...", 0.01);

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

    emit_progress(&reporter, "Downloading M3U8 playlist...", 0.02);

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
                    reporter.clone(),
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
                reporter.clone(),
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
        reporter.clone(),
    )
    .await?;

    if !keep_temp {
        let _ = fs::remove_file(&temp_ts_str).await;
    }

    ensure_output_file_ready(Path::new(output))?;
    emit_progress(&reporter, "All tasks completed", 1.0);

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

pub(crate) struct CandidateCollector {
    page_url: String,
    default_title: String,
    extractor: String,
    seen: HashSet<String>,
    candidates: Vec<MediaCandidate>,
}

impl CandidateCollector {
    pub(crate) fn new(page_url: &str, default_title: &str, extractor: &str) -> Self {
        Self {
            page_url: page_url.to_string(),
            default_title: default_title.to_string(),
            extractor: extractor.to_string(),
            seen: HashSet::new(),
            candidates: Vec::new(),
        }
    }

    pub(crate) fn push(
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

    pub(crate) fn finish(mut self) -> Vec<MediaCandidate> {
        self.candidates.sort_by(|left, right| {
            let left_score = (left.height.max(0), left.width.max(0), left.quality_label.clone());
            let right_score = (right.height.max(0), right.width.max(0), right.quality_label.clone());
            right_score.cmp(&left_score)
        });
        self.candidates
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

fn should_auto_inspect_download_target(
    page_url: &str,
    media_url: &str,
    audio_url: Option<&str>,
) -> bool {
    audio_url.is_none() && page_url == media_url && !is_supported_media_like(media_url)
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

async fn download_to_file(
    client: &Client,
    url: &str,
    path: &Path,
    progress: f64,
    reporter: &ProgressReporter,
    label: &str,
) -> Result<()> {
    emit_progress(reporter, label, progress);
    let response = client.get(url).send().await?.error_for_status()?;
    let bytes = response.bytes().await?;
    fs::write(path, &bytes)
        .await
        .with_context(|| format!("Failed to write downloaded media: {}", path.display()))?;
    Ok(())
}

fn prepare_output_path(output: &str) -> Result<(PathBuf, PathBuf)> {
    let output_path = PathBuf::from(output);
    let temp_dir = output_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    if !temp_dir.exists() {
        std::fs::create_dir_all(&temp_dir)
            .with_context(|| format!("Failed to create output directory: {}", temp_dir.display()))?;
    }

    Ok((output_path, temp_dir))
}

async fn cleanup_temp_files<'a, I>(keep_temp: bool, paths: I)
where
    I: IntoIterator<Item = &'a PathBuf>,
{
    if keep_temp {
        return;
    }

    for path in paths {
        let _ = fs::remove_file(path).await;
    }
}

async fn transcode_input_to_output(
    input_path: &Path,
    output: &str,
    video_bitrate: u32,
    audio_bitrate: u32,
    reporter: ProgressReporter,
) -> Result<()> {
    convert_to_mp4(
        input_path.to_string_lossy().as_ref(),
        output,
        video_bitrate,
        audio_bitrate,
        &MultiProgress::new(),
        select_transcoder_backend().await?,
        reporter,
    )
    .await
}

fn ensure_output_file_ready(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Output file not found after download: {}", path.display()))?;
    if metadata.len() == 0 {
        bail!("Output file was created but empty: {}", path.display());
    }
    Ok(())
}

async fn download_with_retries(
    client: &Client,
    url: &str,
    path: &Path,
    retries: u8,
    progress: f64,
    reporter: &ProgressReporter,
    label: &str,
) -> Result<()> {
    emit_progress(reporter, label, progress);

    for attempt in 1..=retries {
        match client.get(url).send().await {
            Ok(response) => {
                let status = response.status();
                if !status.is_success() {
                    if attempt == retries {
                        return Err(anyhow!("Failed to download media: HTTP {}", status));
                    }
                    let delay = retry_backoff_delay(attempt, Some(status.as_u16()));
                    tokio::time::sleep(delay).await;
                    continue;
                }

                let bytes = response.bytes().await?;
                fs::write(path, &bytes)
                    .await
                    .with_context(|| format!("Failed to write downloaded media: {}", path.display()))?;
                return Ok(());
            }
            Err(error) => {
                if attempt == retries {
                    return Err(error).with_context(|| format!("Failed to download media: {}", url));
                }
                let delay = retry_backoff_delay(attempt, None);
                tokio::time::sleep(delay).await;
            }
        }
    }

    bail!("Failed to download media after retries: {}", url)
}

#[derive(Debug, Deserialize)]
struct DashMpd {
    #[serde(rename = "BaseURL", default)]
    base_urls: Vec<String>,
    #[serde(rename = "Period", default)]
    periods: Vec<DashPeriod>,
}

#[derive(Debug, Deserialize)]
struct DashPeriod {
    #[serde(rename = "AdaptationSet", default)]
    adaptation_sets: Vec<DashAdaptationSet>,
}

#[derive(Debug, Deserialize)]
struct DashAdaptationSet {
    #[serde(rename = "@mimeType")]
    mime_type: Option<String>,
    #[serde(rename = "@contentType")]
    content_type: Option<String>,
    #[serde(rename = "@lang")]
    lang: Option<String>,
    #[serde(rename = "BaseURL", default)]
    base_urls: Vec<String>,
    #[serde(rename = "Representation", default)]
    representations: Vec<DashRepresentation>,
}

#[derive(Debug, Deserialize)]
struct DashRepresentation {
    #[serde(rename = "@id")]
    id: Option<String>,
    #[serde(rename = "@mimeType")]
    mime_type: Option<String>,
    #[serde(rename = "@bandwidth")]
    bandwidth: Option<i64>,
    #[serde(rename = "@width")]
    width: Option<i32>,
    #[serde(rename = "@height")]
    height: Option<i32>,
    #[serde(rename = "BaseURL", default)]
    base_urls: Vec<String>,
}

struct DashDownloadPlan {
    video_url: String,
    audio_url: Option<String>,
}

#[derive(Clone)]
struct DashRepresentationCandidate {
    url: String,
    bandwidth: i64,
    width: i32,
    height: i32,
}

async fn resolve_dash_download_plan(
    manifest_url: &str,
    request_context: &RequestContext,
) -> Result<DashDownloadPlan> {
    let manifest_bytes = download_playlist(manifest_url, request_context).await?;
    let manifest = String::from_utf8(manifest_bytes).context("DASH manifest is not valid UTF-8")?;
    resolve_dash_download_plan_from_manifest(manifest_url, &manifest)
}

fn resolve_dash_download_plan_from_manifest(
    manifest_url: &str,
    manifest: &str,
) -> Result<DashDownloadPlan> {
    let mpd: DashMpd = quick_xml::de::from_str(manifest).context("Failed to parse DASH manifest")?;
    let manifest_base = Url::parse(manifest_url).context("Invalid DASH manifest URL")?;

    let mut best_video: Option<DashRepresentationCandidate> = None;
    let mut best_audio: Option<DashRepresentationCandidate> = None;

    for period in &mpd.periods {
        for adaptation in &period.adaptation_sets {
            let content_type = adaptation
                .content_type
                .as_deref()
                .or(adaptation.mime_type.as_deref())
                .unwrap_or_default()
                .to_ascii_lowercase();

            for representation in &adaptation.representations {
                let representation_type = representation
                    .mime_type
                    .as_deref()
                    .unwrap_or(&content_type)
                    .to_ascii_lowercase();
                let Some(url) = resolve_dash_representation_url(
                    &manifest_base,
                    &mpd.base_urls,
                    &adaptation.base_urls,
                    &representation.base_urls,
                )
                else {
                    continue;
                };

                let candidate = DashRepresentationCandidate {
                    url,
                    bandwidth: representation.bandwidth.unwrap_or_default(),
                    width: representation.width.unwrap_or_default(),
                    height: representation.height.unwrap_or_default(),
                };

                if representation_type.starts_with("video/") || content_type.starts_with("video") {
                    let replace = best_video.as_ref().map(|current| {
                        (candidate.height, candidate.width, candidate.bandwidth)
                            > (current.height, current.width, current.bandwidth)
                    }).unwrap_or(true);
                    if replace {
                        best_video = Some(candidate);
                    }
                } else if representation_type.starts_with("audio/") || content_type.starts_with("audio") {
                    let replace = best_audio
                        .as_ref()
                        .map(|current| candidate.bandwidth > current.bandwidth)
                        .unwrap_or(true);
                    if replace {
                        best_audio = Some(candidate);
                    }
                }
            }
        }
    }

    let video_url = best_video
        .map(|candidate| candidate.url)
        .ok_or_else(|| anyhow!("DASH manifest did not expose a reusable video representation"))?;

    Ok(DashDownloadPlan {
        video_url,
        audio_url: best_audio.map(|candidate| candidate.url),
    })
}

fn resolve_dash_representation_url(
    manifest_base: &Url,
    mpd_base_urls: &[String],
    adaptation_base_urls: &[String],
    representation_base_urls: &[String],
) -> Option<String> {
    representation_base_urls
        .iter()
        .find_map(|base| join_manifest_url(manifest_base, mpd_base_urls, adaptation_base_urls, base))
        .or_else(|| {
            adaptation_base_urls
                .iter()
                .find_map(|base| join_manifest_url(manifest_base, mpd_base_urls, &[], base))
        })
}

fn join_manifest_url(
    manifest_base: &Url,
    mpd_base_urls: &[String],
    adaptation_base_urls: &[String],
    leaf: &str,
) -> Option<String> {
    let mut current = manifest_base.clone();

    for base in mpd_base_urls {
        current = current.join(base).ok()?;
    }
    for base in adaptation_base_urls {
        current = current.join(base).ok()?;
    }

    current.join(leaf).ok().map(|url| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::{resolve_dash_download_plan_from_manifest, should_auto_inspect_download_target};

    #[test]
    fn resolves_dash_download_plan_from_youtube_style_manifest() {
        let manifest = r#"
<MPD>
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <Representation id="135" bandwidth="800000" width="854" height="480">
        <BaseURL>https://video.example/480.mp4</BaseURL>
      </Representation>
      <Representation id="137" bandwidth="2500000" width="1920" height="1080">
        <BaseURL>https://video.example/1080.mp4</BaseURL>
      </Representation>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <Representation id="140" bandwidth="128000">
        <BaseURL>https://audio.example/128.m4a</BaseURL>
      </Representation>
      <Representation id="141" bandwidth="256000">
        <BaseURL>https://audio.example/256.m4a</BaseURL>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>
"#;

        let plan = resolve_dash_download_plan_from_manifest(
            "https://manifest.example/video.mpd",
            manifest,
        )
        .expect("dash plan should resolve");

        assert_eq!(plan.video_url, "https://video.example/1080.mp4");
        assert_eq!(plan.audio_url.as_deref(), Some("https://audio.example/256.m4a"));
    }

    #[test]
    fn resolves_dash_relative_base_urls() {
        let manifest = r#"
<MPD>
  <BaseURL>media/</BaseURL>
  <Period>
    <AdaptationSet contentType="video">
      <BaseURL>video/</BaseURL>
      <Representation id="v1" bandwidth="900000" width="1280" height="720">
        <BaseURL>stream.mp4</BaseURL>
      </Representation>
    </AdaptationSet>
    <AdaptationSet contentType="audio">
      <BaseURL>audio/</BaseURL>
      <Representation id="a1" bandwidth="128000">
        <BaseURL>track.m4a</BaseURL>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>
"#;

        let plan = resolve_dash_download_plan_from_manifest(
            "https://manifest.example/root/index.mpd",
            manifest,
        )
        .expect("relative dash plan should resolve");

        assert_eq!(plan.video_url, "https://manifest.example/root/media/video/stream.mp4");
        assert_eq!(plan.audio_url.as_deref(), Some("https://manifest.example/root/media/audio/track.m4a"));
    }

    #[test]
    fn auto_inspects_plain_page_urls_before_download() {
        assert!(should_auto_inspect_download_target(
            "https://youtu.be/H3R9dQHhQXs",
            "https://youtu.be/H3R9dQHhQXs",
            None,
        ));
        assert!(!should_auto_inspect_download_target(
            "https://cdn.example/video.m3u8",
            "https://cdn.example/video.m3u8",
            None,
        ));
        assert!(!should_auto_inspect_download_target(
            "https://example.com/watch",
            "https://cdn.example/video.mp4",
            None,
        ));
        assert!(!should_auto_inspect_download_target(
            "https://example.com/watch",
            "https://cdn.example/video.mp4",
            Some("https://cdn.example/audio.m4a"),
        ));
    }
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
    candidates: Vec<MediaCandidate>,
    request_context: &RequestContext,
) -> Vec<MediaCandidate> {
    let request_context = request_context.clone();
    let mut candidates = stream::iter(candidates.into_iter().map(|candidate| {
        let request_context = request_context.clone();
        async move { score_candidate(candidate, &request_context).await }
    }))
    .buffer_unordered(6)
    .collect::<Vec<_>>()
    .await;

    candidates.sort_by(|left, right| {
        let left_score = (left.score, left.height.max(0), left.duration_seconds as i64, left.segment_count);
        let right_score = (right.score, right.height.max(0), right.duration_seconds as i64, right.segment_count);
        right_score.cmp(&left_score)
    });
    for candidate in &mut candidates {
        candidate.primary = false;
    }
    if let Some(first) = candidates.first_mut() {
        first.primary = true;
    }
    candidates
}

async fn score_candidate(
    mut candidate: MediaCandidate,
    request_context: &RequestContext,
) -> MediaCandidate {
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
    candidate.reason = if reasons.is_empty() {
        "ranked by available metadata".to_string()
    } else {
        reasons.join(", ")
    };
    candidate
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
    #[cfg(target_os = "android")]
    if ANDROID_HW_TRANSCODER.get().is_some() {
        return Ok(TranscoderKind::AndroidHardware);
    }

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
    reporter: ProgressReporter,
    request_context: RequestContext,
) -> Result<()> {
    if concurrency > 1 {
        match download_and_merge_once(
            playlist.clone(),
            base_url.clone(),
            concurrency,
            retries,
            output_file,
            temp_dir,
            multi_progress,
            reporter.clone(),
            request_context.clone(),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) => {
                warn!(
                    "Multi-thread segment download failed with concurrency {}. Retrying single-threaded: {}",
                    concurrency,
                    error
                );
                cleanup_segment_temp_files(temp_dir, playlist.segments.len(), output_file).await;
                emit_progress(
                    &reporter,
                    "Multi-thread download failed. Retrying in single-thread mode...",
                    0.12,
                );
            }
        }
    }

    download_and_merge_once(
        playlist,
        base_url,
        1,
        retries,
        output_file,
        temp_dir,
        multi_progress,
        reporter,
        request_context,
    )
    .await
}

async fn download_and_merge_once(
    playlist: m3u8_rs::MediaPlaylist,
    base_url: Option<Url>,
    concurrency: usize,
    retries: u8,
    output_file: &str,
    temp_dir: &Path,
    multi_progress: &MultiProgress,
    reporter: ProgressReporter,
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
            let reporter = reporter.clone();
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
                            emit_progress(
                                &reporter,
                                format!("Downloading segments [{}/{}]", *count, total),
                                (*count as f64) / (total as f64) * 0.9,
                            );

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

async fn cleanup_segment_temp_files(temp_dir: &Path, total: usize, output_file: &str) {
    for index in 0..total {
        let file_name = format!("seg_{:05}.ts", index);
        let _ = fs::remove_file(temp_dir.join(file_name)).await;
    }
    let _ = fs::remove_file(output_file).await;
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
    reporter: ProgressReporter,
) -> Result<()> {
    let convert_pb = multi_progress.add(ProgressBar::new_spinner());
    convert_pb.set_style(
        ProgressStyle::with_template("{spinner:.yellow} {msg}")?
            .tick_strings(&["-", "\\", "|", "/"]),
    );
    convert_pb.set_message("Converting to MP4...");
    convert_pb.enable_steady_tick(Duration::from_millis(120));

    emit_progress(&reporter, "Converting to MP4...", 0.95);

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