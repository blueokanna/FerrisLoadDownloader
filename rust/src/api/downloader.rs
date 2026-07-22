#![allow(dead_code)]
#![allow(unused_imports, unused_variables)]

use crate::api::site_adapters::{extractor_name_for_host, inspect_page_candidates, SiteWarning};
use crate::frb_generated::StreamSink;
use aes::Aes128;
use anyhow::{anyhow, bail, Context, Result};
use block_modes::block_padding::Pkcs7;
use block_modes::{BlockMode, Cbc};
use futures::stream::{self, StreamExt};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::{error, info, warn};
use m3u8_rs::{
    parse_playlist, AlternativeMedia, AlternativeMediaType, ByteRange, KeyMethod, MasterPlaylist,
    Playlist, VariantStream,
};
use regex::Regex;
use reqwest::{header, Client, Response};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
#[cfg(target_os = "android")]
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Semaphore;
use tokio::{fs, process::Command, sync::Mutex};
use url::Url;

#[cfg(target_os = "android")]
use jni::objects::{GlobalRef, JClass, JObject, JValue};
#[cfg(target_os = "android")]
use jni::JNIEnv;
#[cfg(target_os = "android")]
use jni::JavaVM;

type Aes128Cbc = Cbc<Aes128, Pkcs7>;
pub(crate) type ProgressReporter = Arc<dyn Fn(ProgressUpdate) + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccelType {
    Nvidia,
    AMD,
    IntelQuickSync,
    LinuxVaapi,
    AppleVideoToolbox,
    CPU,
}

impl AccelType {
    fn label(self) -> &'static str {
        match self {
            Self::Nvidia => "NVIDIA NVENC",
            Self::AMD => "AMD AMF",
            Self::IntelQuickSync => "Intel Quick Sync",
            Self::LinuxVaapi => "Linux VAAPI",
            Self::AppleVideoToolbox => "Apple VideoToolbox",
            Self::CPU => "CPU libx264",
        }
    }

    fn encoder(self) -> &'static str {
        match self {
            Self::Nvidia => "h264_nvenc",
            Self::AMD => "h264_amf",
            Self::IntelQuickSync => "h264_qsv",
            Self::LinuxVaapi => "h264_vaapi",
            Self::AppleVideoToolbox => "h264_videotoolbox",
            Self::CPU => "libx264",
        }
    }

    fn hardware_accelerated(self) -> bool {
        self != Self::CPU
    }
}

#[derive(Clone, Copy, Debug)]
enum TranscoderKind {
    Ffmpeg(AccelType),
    AndroidHardware,
}

#[derive(Clone, Debug)]
struct ExternalCommandSpec {
    program: PathBuf,
    prefix_args: Vec<String>,
}

impl ExternalCommandSpec {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.prefix_args);
        command
    }
}

async fn external_command_works(spec: &ExternalCommandSpec) -> bool {
    tokio::time::timeout(
        Duration::from_secs(8),
        spec.command().arg("--version").output(),
    )
    .await
    .ok()
    .and_then(|result| result.ok())
    .is_some_and(|output| output.status.success())
}

async fn ffmpeg_command_works(path: &Path) -> bool {
    tokio::time::timeout(
        Duration::from_secs(8),
        Command::new(path).arg("-version").output(),
    )
    .await
    .ok()
    .and_then(|result| result.ok())
    .is_some_and(|output| output.status.success())
}

async fn resolve_ffmpeg_path() -> Option<PathBuf> {
    let executable_name = if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let mut candidates = Vec::new();

    if let Some(configured) = env::var_os("FERRISLOAD_FFMPEG_PATH") {
        candidates.push(PathBuf::from(configured));
    }
    if let Ok(current_exe) = env::current_exe() {
        if let Some(directory) = current_exe.parent() {
            candidates.push(directory.join("tools").join(executable_name));
            candidates.push(directory.join(executable_name));
        }
    }
    candidates.push(PathBuf::from(executable_name));

    for candidate in candidates {
        if ffmpeg_command_works(&candidate).await {
            return Some(candidate);
        }
    }
    None
}

async fn resolve_ytdlp_command() -> Option<ExternalCommandSpec> {
    let executable_name = if cfg!(target_os = "windows") {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    };
    let mut candidates = Vec::new();

    if let Some(configured) = env::var_os("FERRISLOAD_YTDLP_PATH") {
        candidates.push(ExternalCommandSpec {
            program: PathBuf::from(configured),
            prefix_args: Vec::new(),
        });
    }
    if let Ok(current_exe) = env::current_exe() {
        if let Some(directory) = current_exe.parent() {
            candidates.push(ExternalCommandSpec {
                program: directory.join("tools").join(executable_name),
                prefix_args: Vec::new(),
            });
            candidates.push(ExternalCommandSpec {
                program: directory.join(executable_name),
                prefix_args: Vec::new(),
            });
        }
    }
    if let Ok(cached) = ytdlp_cache_path() {
        candidates.push(ExternalCommandSpec {
            program: cached,
            prefix_args: Vec::new(),
        });
    }
    candidates.push(ExternalCommandSpec {
        program: PathBuf::from(executable_name),
        prefix_args: Vec::new(),
    });

    let python_launchers: &[&str] = if cfg!(target_os = "windows") {
        &["py", "python"]
    } else {
        &["python3", "python"]
    };
    candidates.extend(python_launchers.iter().map(|launcher| ExternalCommandSpec {
        program: PathBuf::from(launcher),
        prefix_args: vec!["-m".to_string(), "yt_dlp".to_string()],
    }));

    for candidate in candidates {
        if external_command_works(&candidate).await {
            return Some(candidate);
        }
    }
    None
}

fn official_ytdlp_asset_name() -> Result<&'static str> {
    if cfg!(target_os = "windows") {
        return if cfg!(target_arch = "aarch64") {
            Ok("yt-dlp_arm64.exe")
        } else {
            Ok("yt-dlp.exe")
        };
    }
    if cfg!(target_os = "macos") {
        return Ok("yt-dlp_macos");
    }
    if cfg!(target_os = "linux") {
        return if cfg!(target_arch = "aarch64") {
            Ok("yt-dlp_linux_aarch64")
        } else if cfg!(target_arch = "arm") {
            Ok("yt-dlp_linux_armv7l")
        } else {
            Ok("yt-dlp_linux")
        };
    }
    bail!("Automatic yt-dlp installation is not available on this platform")
}

fn ytdlp_cache_path() -> Result<PathBuf> {
    let cache_root = if cfg!(target_os = "windows") {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("FerrisLoad")
    } else if cfg!(target_os = "macos") {
        env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("Library")
            .join("Application Support")
            .join("FerrisLoad")
    } else {
        env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
            })
            .unwrap_or_else(env::temp_dir)
            .join("ferrisload")
    };
    let executable = if cfg!(target_os = "windows") {
        "yt-dlp.exe"
    } else {
        "yt-dlp"
    };
    Ok(cache_root.join("tools").join(executable))
}

fn checksum_for_release_asset(checksums: &str, asset_name: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let name = fields.next()?.trim_start_matches('*');
        if name == asset_name
            && hash.len() == 64
            && hash.chars().all(|character| character.is_ascii_hexdigit())
        {
            Some(hash.to_ascii_lowercase())
        } else {
            None
        }
    })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn provision_ytdlp_command(reporter: &ProgressReporter) -> Result<ExternalCommandSpec> {
    let asset_name = official_ytdlp_asset_name()?;
    let target_path = ytdlp_cache_path()?;
    let target_directory = target_path
        .parent()
        .ok_or_else(|| anyhow!("yt-dlp cache path has no parent directory"))?;
    fs::create_dir_all(target_directory)
        .await
        .with_context(|| {
            format!(
                "Failed to create yt-dlp cache directory: {}",
                target_directory.display()
            )
        })?;

    emit_progress(reporter, "Preparing verified yt-dlp engine...", 0.003);
    let client = Client::builder()
        .user_agent("FerrisLoad/1.0 yt-dlp-bootstrap")
        .timeout(Duration::from_secs(300))
        .build()?;
    let release_base = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
    let checksums = client
        .get(format!("{}/SHA2-256SUMS", release_base))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let expected_hash = checksum_for_release_asset(&checksums, asset_name).ok_or_else(|| {
        anyhow!(
            "Official yt-dlp checksum list did not contain {}",
            asset_name
        )
    })?;

    let response = client
        .get(format!("{}/{}", release_base, asset_name))
        .send()
        .await?
        .error_for_status()?;
    let total = response.content_length();
    let temporary_path = target_directory.join(format!(
        ".yt-dlp-download-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let mut output = fs::File::create(&temporary_path).await?;
    let mut stream = response.bytes_stream();
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    let mut last_reported = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        output.write_all(&chunk).await?;
        hasher.update(&chunk);
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded.saturating_sub(last_reported) >= 512 * 1024 {
            let progress = total
                .filter(|total| *total > 0)
                .map(|total| 0.003 + (downloaded as f64 / total as f64) * 0.015)
                .unwrap_or(0.003);
            let detail = total
                .map(|total| format!("{} / {}", human_bytes(downloaded), human_bytes(total)))
                .unwrap_or_else(|| human_bytes(downloaded));
            emit_progress(
                reporter,
                format!("Installing verified yt-dlp engine [{}]", detail),
                progress,
            );
            last_reported = downloaded;
        }
    }
    output.flush().await?;
    drop(output);

    let actual_hash = hex::encode(hasher.finalize());
    if actual_hash != expected_hash {
        let _ = fs::remove_file(&temporary_path).await;
        bail!(
            "yt-dlp SHA-256 verification failed (expected {}, received {})",
            expected_hash,
            actual_hash
        );
    }

    if target_path.exists() {
        let _ = fs::remove_file(&target_path).await;
    }
    fs::rename(&temporary_path, &target_path)
        .await
        .with_context(|| {
            format!(
                "Failed to install verified yt-dlp engine at {}",
                target_path.display()
            )
        })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&target_path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&target_path, permissions)?;
    }

    let command = ExternalCommandSpec {
        program: target_path,
        prefix_args: Vec::new(),
    };
    if !external_command_works(&command).await {
        bail!("Verified yt-dlp engine was installed but could not be executed");
    }
    emit_progress(reporter, "Verified yt-dlp engine is ready", 0.02);
    Ok(command)
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

pub(crate) fn emit_progress(
    reporter: &ProgressReporter,
    message: impl Into<String>,
    progress: f64,
) {
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

    pub async fn mux(&self, video_path: &str, audio_path: &str, output_mp4: &str) -> Result<()> {
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
    let class_name = jni_string!(
        env,
        "com.bluevale.m3u8_downloader.MediaTranscoder",
        "class_name"
    );
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

    let url = normalize_source_url(&url)?;

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

    let requested_page_url = Url::parse(&url).context("Invalid inspection URL")?;
    let client = create_http_client_for_context(Some(&url), &request_context)?;
    let response = client.get(requested_page_url).send().await?;
    let status = response.status();
    let page_url = response.url().clone();
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
            warnings: warnings
                .into_iter()
                .map(SiteWarning::into_display)
                .collect(),
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

    if extractor == "bilibili" {
        let had_candidates = !collector.candidates.is_empty();
        if let Err(error) = augment_bilibili_candidates_with_playurl(
            &page_url,
            &html,
            &request_context,
            &mut collector,
            &mut warnings,
        )
        .await
        {
            if !had_candidates {
                warnings.push(SiteWarning::site(
                    "bilibili-playurl-fallback-failed",
                    format!(
                        "Bilibili playurl API could not resolve this video: {}",
                        error
                    ),
                ));
            }
        }
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    if extractor == "youtube" {
        match augment_youtube_candidates_with_ytdlp(
            page_url.as_str(),
            &request_context,
            &mut collector,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => warnings.push(SiteWarning::site(
                "youtube-ytdlp-no-formats",
                "yt-dlp ran successfully but did not expose a reusable video format",
            )),
            Err(error) => warnings.push(SiteWarning::site(
                "youtube-ytdlp-fallback-failed",
                format!("The yt-dlp engine could not resolve this video: {}", error),
            )),
        }
    }

    let candidates = score_candidates(collector.finish(), &request_context).await;

    if candidates.is_empty() {
        if let Some(auth_warning) = warnings.iter().find(|warning| warning.scope() == "auth") {
            let challenge_reason = auth_warning.message().to_string();
            return Ok(MediaInspectionResult {
                page_url: url,
                page_title,
                extractor,
                candidates,
                warnings: warnings
                    .into_iter()
                    .map(SiteWarning::into_display)
                    .collect(),
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
        page_url: page_url.to_string(),
        page_title,
        extractor,
        candidates,
        warnings: warnings
            .into_iter()
            .map(SiteWarning::into_display)
            .collect(),
        auth_required: false,
        challenge_reason: String::new(),
    })
}

async fn augment_bilibili_candidates_with_playurl(
    page_url: &Url,
    html: &str,
    request_context: &RequestContext,
    collector: &mut CandidateCollector,
    warnings: &mut Vec<SiteWarning>,
) -> Result<bool> {
    let Some(api_url) = bilibili_playurl_api_url(page_url, html)? else {
        return Ok(false);
    };

    let client = create_http_client_for_context(Some(page_url.as_str()), request_context)?;
    let response = client.get(api_url.clone()).send().await?;
    let status = response.status();
    if !status.is_success() {
        bail!("Bilibili playurl API returned HTTP {}", status);
    }
    let body = response.text().await?;
    let payload: Value =
        serde_json::from_str(&body).context("Failed to parse Bilibili playurl API response")?;
    let code = payload.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = payload
            .get("message")
            .or_else(|| payload.get("msg"))
            .and_then(Value::as_str)
            .unwrap_or("unknown Bilibili API error");
        bail!("Bilibili playurl API error {}: {}", code, message);
    }

    let before = collector.candidates.len();
    let synthetic_page = format!("window.__playinfo__={}", body);
    inspect_page_candidates(page_url, &synthetic_page, collector, warnings)?;
    Ok(collector.candidates.len() > before)
}

fn bilibili_playurl_api_url(page_url: &Url, html: &str) -> Result<Option<Url>> {
    let episode_pattern = Regex::new(r"/bangumi/play/ep(\d+)")?;
    let episode_id = episode_pattern
        .captures(page_url.path())
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_string())
        .or_else(|| {
            Regex::new(r#""ep_id"\s*:\s*(\d+)"#)
                .ok()?
                .captures(html)?
                .get(1)
                .map(|value| value.as_str().to_string())
        });
    if let Some(episode_id) = episode_id {
        let mut url = Url::parse("https://api.bilibili.com/pgc/player/web/playurl")?;
        url.query_pairs_mut()
            .append_pair("ep_id", &episode_id)
            .append_pair("qn", "127")
            .append_pair("fnval", "4048")
            .append_pair("fourk", "1");
        return Ok(Some(url));
    }

    let bvid_pattern = Regex::new(r"(?i)(BV[0-9A-Za-z]{10})")?;
    let bvid = bvid_pattern
        .captures(page_url.path())
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_string())
        .or_else(|| {
            Regex::new(r#""bvid"\s*:\s*"(BV[0-9A-Za-z]{10})""#)
                .ok()?
                .captures(html)?
                .get(1)
                .map(|value| value.as_str().to_string())
        });
    let cid = Regex::new(r#""cid"\s*:\s*(\d+)"#)?
        .captures(html)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_string());

    let (Some(bvid), Some(cid)) = (bvid, cid) else {
        return Ok(None);
    };
    let mut url = Url::parse("https://api.bilibili.com/x/player/playurl")?;
    url.query_pairs_mut()
        .append_pair("bvid", &bvid)
        .append_pair("cid", &cid)
        .append_pair("qn", "127")
        .append_pair("fnval", "4048")
        .append_pair("fourk", "1");
    Ok(Some(url))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn augment_youtube_candidates_with_ytdlp(
    page_url: &str,
    request_context: &RequestContext,
    collector: &mut CandidateCollector,
) -> Result<bool> {
    let command_spec = resolve_ytdlp_command()
        .await
        .ok_or_else(|| anyhow!("yt-dlp executable or Python module was not found"))?;
    let mut command = command_spec.command();
    command.args([
        "--dump-single-json",
        "--skip-download",
        "--no-playlist",
        "--no-warnings",
    ]);
    if !request_context.user_agent.trim().is_empty() {
        command.args(["--user-agent", request_context.user_agent.trim()]);
    }
    if !request_context.referer.trim().is_empty() {
        command.args(["--referer", request_context.referer.trim()]);
    }
    if !request_context.origin.trim().is_empty() {
        command.args([
            "--add-header",
            &format!("Origin:{}", request_context.origin.trim()),
        ]);
    }
    if !request_context.cookie.trim().is_empty() {
        command.args([
            "--add-header",
            &format!("Cookie:{}", request_context.cookie.trim()),
        ]);
    }
    for header in &request_context.headers {
        let name = header.name.trim();
        let value = header.value.trim();
        if !name.is_empty() && !value.is_empty() {
            command.args(["--add-header", &format!("{}:{}", name, value)]);
        }
    }
    command.arg(page_url);

    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .map_err(|_| anyhow!("yt-dlp inspection timed out"))?
        .context("Failed to start yt-dlp")?;
    if !output.status.success() {
        bail!(
            "{}",
            String::from_utf8_lossy(&output.stderr).trim().to_string()
        );
    }

    let metadata: Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse yt-dlp metadata")?;
    let title = metadata
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(formats) = metadata.get("formats").and_then(Value::as_array) else {
        return Ok(false);
    };

    let best_audio = formats
        .iter()
        .filter(|format| {
            format.get("vcodec").and_then(Value::as_str) == Some("none")
                && format
                    .get("acodec")
                    .and_then(Value::as_str)
                    .is_some_and(|codec| codec != "none")
                && format
                    .get("url")
                    .and_then(Value::as_str)
                    .is_some_and(|url| !url.is_empty())
                && !format
                    .get("has_drm")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .max_by_key(|format| {
            format
                .get("abr")
                .or_else(|| format.get("tbr"))
                .and_then(Value::as_f64)
                .unwrap_or_default() as i64
        })
        .and_then(|format| format.get("url").and_then(Value::as_str))
        .map(str::to_string);

    let mut video_formats = formats
        .iter()
        .filter(|format| {
            format
                .get("url")
                .and_then(Value::as_str)
                .is_some_and(|url| !url.is_empty())
                && format
                    .get("vcodec")
                    .and_then(Value::as_str)
                    .is_some_and(|codec| codec != "none")
                && !format
                    .get("has_drm")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    video_formats.sort_by(|left, right| {
        let rank = |format: &Value| {
            (
                format
                    .get("height")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
                format
                    .get("tbr")
                    .and_then(Value::as_f64)
                    .unwrap_or_default() as i64,
            )
        };
        rank(right).cmp(&rank(left))
    });

    let before = collector.candidates.len();
    for format in video_formats.into_iter().take(32) {
        let Some(media_url) = format.get("url").and_then(Value::as_str) else {
            continue;
        };
        let has_audio = format
            .get("acodec")
            .and_then(Value::as_str)
            .is_some_and(|codec| codec != "none");
        let quality = format
            .get("format_note")
            .or_else(|| format.get("resolution"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                format
                    .get("height")
                    .and_then(Value::as_i64)
                    .map(|height| format!("{}p", height))
            });
        let extension = format.get("ext").and_then(Value::as_str).unwrap_or("mp4");
        collector.push(
            media_url.to_string(),
            if has_audio { None } else { best_audio.clone() },
            title.clone(),
            quality,
            Some(mime_from_extension(extension).to_string()),
            format
                .get("width")
                .and_then(Value::as_i64)
                .map(|value| value as i32),
            format
                .get("height")
                .and_then(Value::as_i64)
                .map(|value| value as i32),
            Some("youtube/yt-dlp"),
        );
    }

    Ok(collector.candidates.len() > before)
}

fn youtube_itag_from_media_url(media_url: &str) -> Option<String> {
    let url = Url::parse(media_url).ok()?;
    url.query_pairs()
        .find(|(name, value)| {
            name == "itag" && value.chars().all(|character| character.is_ascii_digit())
        })
        .map(|(_, value)| value.into_owned())
}

fn parse_ytdlp_progress(line: &str) -> Option<(String, f64)> {
    let payload = line.strip_prefix("FERRISLOAD_PROGRESS|")?;
    let mut fields = payload.split('|');
    let percent_text = fields.next()?.trim().trim_end_matches('%').trim();
    let percent = percent_text.parse::<f64>().ok()?.clamp(0.0, 100.0);
    let speed = fields
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let eta = fields
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut message = format!("yt-dlp download {:.1}%", percent);
    if let Some(speed) = speed {
        message.push_str(" | ");
        message.push_str(speed);
    }
    if let Some(eta) = eta {
        message.push_str(" | ETA ");
        message.push_str(eta);
    }
    Some((message, 0.05 + (percent / 100.0) * 0.80))
}

fn ytdlp_error_tail(stderr: &str) -> String {
    let tail = stderr
        .chars()
        .rev()
        .take(2400)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    tail.trim().to_string()
}

fn find_ytdlp_output(temp_dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(temp_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("site_input.") && !name.ends_with(".part"))
        })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn run_ytdlp_site_pipeline(
    reporter: ProgressReporter,
    command_spec: ExternalCommandSpec,
    page_url: &str,
    media_url: &str,
    output: &str,
    concurrency: i32,
    retries: i32,
    video_bitrate: i32,
    audio_bitrate: i32,
    keep_temp: bool,
    request_context: &RequestContext,
) -> Result<()> {
    emit_progress(&reporter, "Selected site engine: yt-dlp", 0.02);

    let ffmpeg_path = resolve_ffmpeg_path().await;
    let has_ffmpeg = ffmpeg_path.is_some();
    let requires_reencode = video_bitrate > 0 || audio_bitrate > 0;
    if requires_reencode && !has_ffmpeg {
        bail!("FFmpeg is required when bitrate controls request re-encoding");
    }

    let (output_path, temp_dir) = prepare_output_path(output)?;
    let download_template = if requires_reencode {
        temp_dir.join("site_input.%(ext)s")
    } else {
        output_path.clone()
    };

    let format_selector = if has_ffmpeg {
        youtube_itag_from_media_url(media_url)
            .map(|itag| format!("{}+bestaudio/{}/best[ext=mp4]/best", itag, itag))
            .unwrap_or_else(|| {
                "bv*[ext=mp4][vcodec^=avc1]+ba[ext=m4a]/b[ext=mp4]/bv*+ba/b".to_string()
            })
    } else {
        "best[ext=mp4][vcodec!=none][acodec!=none]".to_string()
    };

    let retries = retries.max(1).to_string();
    let concurrency = concurrency.max(1).to_string();
    let mut command = command_spec.command();
    command.args([
        "--no-playlist",
        "--newline",
        "--progress",
        "--no-colors",
        "--no-warnings",
        "--force-overwrites",
        "--socket-timeout",
        "30",
        "--retries",
        &retries,
        "--fragment-retries",
        &retries,
        "--extractor-retries",
        &retries,
        "--concurrent-fragments",
        &concurrency,
        "--progress-template",
        "download:FERRISLOAD_PROGRESS|%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s",
        "--print",
        "after_move:FERRISLOAD_OUTPUT:%(filepath)s",
        "--format",
        &format_selector,
        "--output",
        download_template.to_string_lossy().as_ref(),
    ]);
    if has_ffmpeg {
        command.args(["--merge-output-format", "mp4"]);
        if let Some(path) = ffmpeg_path.as_ref() {
            command.args(["--ffmpeg-location", path.to_string_lossy().as_ref()]);
        }
    }
    if !request_context.user_agent.trim().is_empty() {
        command.args(["--user-agent", request_context.user_agent.trim()]);
    }
    if !request_context.referer.trim().is_empty() {
        command.args(["--referer", request_context.referer.trim()]);
    }
    if !request_context.origin.trim().is_empty() {
        command.args([
            "--add-headers",
            &format!("Origin:{}", request_context.origin.trim()),
        ]);
    }
    if !request_context.cookie.trim().is_empty() {
        command.args([
            "--add-headers",
            &format!("Cookie:{}", request_context.cookie.trim()),
        ]);
    }
    for entry in &request_context.headers {
        let name = entry.name.trim();
        let value = entry.value.trim();
        if !name.is_empty() && !value.is_empty() {
            command.args(["--add-headers", &format!("{}:{}", name, value)]);
        }
    }
    command
        .arg(page_url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().context("Failed to start yt-dlp")?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("yt-dlp stdout was not captured"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("yt-dlp stderr was not captured"))?;
    let stderr_task = tokio::spawn(async move {
        let mut text = String::new();
        stderr.read_to_string(&mut text).await.map(|_| text)
    });

    let mut lines = BufReader::new(stdout).lines();
    let mut reported_output = None;
    while let Some(line) = lines.next_line().await? {
        if let Some((message, progress)) = parse_ytdlp_progress(line.trim()) {
            emit_progress(&reporter, message, progress);
        } else if let Some(path) = line.trim().strip_prefix("FERRISLOAD_OUTPUT:") {
            reported_output = Some(PathBuf::from(path.trim()));
        }
    }

    let status = child.wait().await?;
    let stderr = stderr_task.await.context("yt-dlp stderr task failed")??;
    if !status.success() {
        bail!("yt-dlp download failed: {}", ytdlp_error_tail(&stderr));
    }

    let downloaded_path = reported_output
        .filter(|path| path.exists())
        .or_else(|| {
            if output_path.exists() {
                Some(output_path.clone())
            } else {
                find_ytdlp_output(&temp_dir)
            }
        })
        .ok_or_else(|| anyhow!("yt-dlp completed without producing an output file"))?;

    if requires_reencode {
        emit_progress(
            &reporter,
            "yt-dlp finished; preparing hardware transcode",
            0.88,
        );
        transcode_input_to_output(
            &downloaded_path,
            output,
            video_bitrate.max(0) as u32,
            audio_bitrate.max(0) as u32,
            reporter.clone(),
        )
        .await?;
        cleanup_temp_files(keep_temp, [&downloaded_path]).await;
    } else if downloaded_path != output_path {
        if fs::rename(&downloaded_path, &output_path).await.is_err() {
            fs::copy(&downloaded_path, &output_path)
                .await
                .with_context(|| {
                    format!("Failed to move yt-dlp output to {}", output_path.display())
                })?;
            cleanup_temp_files(keep_temp, [&downloaded_path]).await;
        }
    }

    if !keep_temp {
        let _ = fs::remove_dir(&temp_dir).await;
    }
    ensure_output_file_ready(&output_path).await?;
    emit_progress(&reporter, "All tasks completed", 1.0);
    Ok(())
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

    let page_url = normalize_source_url(&page_url)?;
    let media_url = normalize_source_url(&media_url)?;
    let audio_url = audio_url
        .map(|url| normalize_source_url(&url))
        .transpose()?;

    let auto_inspect =
        should_auto_inspect_download_target(&page_url, &media_url, audio_url.as_deref());

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let extractor = Url::parse(&page_url)
            .ok()
            .map(|url| extractor_name_for_host(url.domain()))
            .unwrap_or_default();
        let should_use_ytdlp = extractor == "youtube"
            || (extractor == "bilibili" && auto_inspect && check_ffmpeg().await);
        if should_use_ytdlp {
            let command = if let Some(command) = resolve_ytdlp_command().await {
                Some(command)
            } else {
                match provision_ytdlp_command(&reporter).await {
                    Ok(command) => Some(command),
                    Err(error) => {
                        warn!("Verified yt-dlp installation failed: {}", error);
                        emit_progress(
                            &reporter,
                            format!(
                                "yt-dlp engine unavailable; using native resolver: {}",
                                error
                            ),
                            0.02,
                        );
                        None
                    }
                }
            };
            if let Some(command) = command {
                return run_ytdlp_site_pipeline(
                    reporter,
                    command,
                    &page_url,
                    &media_url,
                    &output,
                    concurrency,
                    retries,
                    video_bitrate,
                    audio_bitrate,
                    keep_temp,
                    &request_context,
                )
                .await;
            }
        }
    }

    if auto_inspect {
        emit_progress(&reporter, "Resolving page media candidate...", 0.01);

        let inspection =
            inspect_media_with_context(page_url.clone(), request_context.clone()).await?;
        if inspection.auth_required {
            let reason = if inspection.challenge_reason.trim().is_empty() {
                "Authorization required before download".to_string()
            } else {
                inspection.challenge_reason.clone()
            };
            bail!("Authorization required before download: {}", reason);
        }

        let selected_candidate = inspection.candidates.into_iter().next().ok_or_else(|| {
            anyhow!("No downloadable media candidates were found for the page URL")
        })?;

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
    let client =
        create_http_client_for_context(page_url.as_ref().map(Url::as_str), &request_context)?;
    let (output_path, temp_dir) = prepare_output_path(&output)?;

    match audio_url {
        Some(audio) => {
            let video_temp = temp_dir.join("stream_video_input.bin");
            let audio_temp = temp_dir.join("stream_audio_input.bin");

            download_with_retries(
                &client,
                &media_url,
                &video_temp,
                retries.max(1) as u8,
                0.04,
                0.44,
                &reporter,
                "Downloading video stream",
            )
            .await?;
            download_with_retries(
                &client,
                &audio,
                &audio_temp,
                retries.max(1) as u8,
                0.46,
                0.80,
                &reporter,
                "Downloading audio stream",
            )
            .await?;

            merge_media_streams(
                &video_temp,
                &audio_temp,
                &output,
                video_bitrate.max(0) as u32,
                audio_bitrate.max(0) as u32,
                reporter.clone(),
            )
            .await?;

            cleanup_temp_files(keep_temp, [&video_temp, &audio_temp]).await;
        }
        None => {
            let extension = container_from_url(&media_url);
            if extension == "mp4" && video_bitrate <= 0 && audio_bitrate <= 0 {
                download_with_retries(
                    &client,
                    &media_url,
                    &output_path,
                    retries.max(1) as u8,
                    0.04,
                    0.92,
                    &reporter,
                    "Downloading media",
                )
                .await?;
            } else {
                let temp_input = temp_dir.join(format!("direct_input.{}", extension));
                download_with_retries(
                    &client,
                    &media_url,
                    &temp_input,
                    retries.max(1) as u8,
                    0.04,
                    0.72,
                    &reporter,
                    "Downloading media",
                )
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

    let _ = fs::remove_dir(&temp_dir).await;
    ensure_output_file_ready(&output_path).await?;
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
        0.04,
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
                0.48,
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
                reporter.clone(),
            )
            .await?;

            cleanup_temp_files(keep_temp, [&video_temp, &audio_temp]).await;
        }
        None => {
            if video_extension == "mp4" && video_bitrate <= 0 && audio_bitrate <= 0 {
                if fs::rename(&video_temp, &output_path).await.is_err() {
                    fs::copy(&video_temp, &output_path).await.with_context(|| {
                        format!("Failed to copy DASH output to {}", output_path.display())
                    })?;
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

    let _ = fs::remove_dir(&temp_dir).await;
    ensure_output_file_ready(&output_path).await?;
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
    let requires_reencode = video_bitrate > 0 || audio_bitrate > 0;
    match backend {
        TranscoderKind::Ffmpeg(accel) if requires_reencode => {
            check_pb.finish_with_message(format!("Selected FFmpeg backend ({})", accel.label()));
            emit_progress(
                &reporter,
                format!("Selected transcoder backend: {}", accel.label()),
                0.015,
            );
        }
        TranscoderKind::Ffmpeg(_) => {
            check_pb.finish_with_message("Selected FFmpeg stream-copy backend");
            emit_progress(
                &reporter,
                "Selected FFmpeg stream copy (no re-encoding)",
                0.015,
            );
        }
        TranscoderKind::AndroidHardware if requires_reencode => {
            check_pb.finish_with_message("Selected Android MediaCodec backend");
            emit_progress(
                &reporter,
                "Selected hardware encoder: Android MediaCodec",
                0.015,
            );
        }
        TranscoderKind::AndroidHardware => {
            check_pb.finish_with_message("Selected Android MediaMuxer pipeline");
            emit_progress(
                &reporter,
                "Selected Android MediaMuxer (MediaCodec fallback if remux fails)",
                0.015,
            );
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

    let (m3u8_content, effective_playlist_url) =
        download_playlist_with_url(url, request_context).await?;
    let (_, playlist) =
        parse_playlist(&m3u8_content).map_err(|e| anyhow!("Failed to parse M3U8: {:?}", e))?;
    download_pb.finish_with_message("Parsed M3U8 playlist");

    let base_url = Some(playlist_base_url(&effective_playlist_url));

    let temp_root = if cfg!(target_os = "android") {
        #[cfg(target_os = "android")]
        {
            select_writable_temp_dir()?
        }
        #[cfg(not(target_os = "android"))]
        {
            unreachable!()
        }
    } else {
        PathBuf::from(output)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };
    let temp_dir = create_unique_temp_dir(&temp_root, Path::new(output))?;

    let temp_primary = temp_dir.join("temp_primary.ts");
    let temp_primary_str = temp_primary.to_string_lossy().to_string();
    let temp_audio = temp_dir.join("temp_audio.ts");
    let temp_audio_str = temp_audio.to_string_lossy().to_string();

    info!("Temporary directory: {}", temp_dir.display());
    info!("Temporary primary stream: {}", temp_primary_str);

    let mut external_audio_plan: Option<(m3u8_rs::MediaPlaylist, Url, String)> = None;

    match playlist {
        Playlist::MasterPlaylist(master) => {
            info!("Master Playlist found, {} variants", master.variants.len());

            let best = select_best_hls_variant(&master)
                .cloned()
                .ok_or_else(|| anyhow!("No usable non-I-frame variant found"))?;

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

            let (media_content, effective_media_url) =
                download_playlist_with_url(media_url.as_str(), request_context).await?;
            let (_, media_pl) = parse_playlist(&media_content)
                .map_err(|e| anyhow!("Failed to parse m3u8: {:?}", e))?;

            let Playlist::MediaPlaylist(media_playlist) = media_pl else {
                bail!("Master playlist's referenced playlist is not a media playlist");
            };

            if let Some(audio_group) = best.audio.as_deref() {
                let rendition =
                    select_hls_audio_rendition(&master, Some(audio_group)).ok_or_else(|| {
                        anyhow!(
                            "Selected HLS variant references missing audio group: {}",
                            audio_group
                        )
                    })?;
                info!(
                    "Selected HLS audio rendition: {} ({})",
                    rendition.name,
                    rendition.language.as_deref().unwrap_or("und")
                );

                if let Some(audio_uri) = rendition.uri.as_deref() {
                    let audio_url = base_url
                        .as_ref()
                        .ok_or_else(|| anyhow!("Master playlist missing URL"))?
                        .join(audio_uri)?;
                    let (audio_content, effective_audio_url) =
                        download_playlist_with_url(audio_url.as_str(), request_context).await?;
                    let (_, audio_playlist) = parse_playlist(&audio_content)
                        .map_err(|e| anyhow!("Failed to parse HLS audio rendition: {:?}", e))?;
                    let Playlist::MediaPlaylist(audio_playlist) = audio_playlist else {
                        bail!("HLS audio rendition does not reference a media playlist");
                    };
                    external_audio_plan = Some((
                        audio_playlist,
                        playlist_base_url(&effective_audio_url),
                        rendition.name.clone(),
                    ));
                } else {
                    info!("Selected HLS audio rendition is carried in the variant stream");
                }
            }

            let video_reporter = if external_audio_plan.is_some() {
                staged_progress_reporter(reporter.clone(), "Video", 0.02, 0.48)
            } else {
                reporter.clone()
            };
            download_and_merge(
                media_playlist,
                Some(playlist_base_url(&effective_media_url)),
                concurrency,
                retries,
                &temp_primary_str,
                &temp_dir,
                &multi_progress,
                video_reporter,
                request_context.clone(),
            )
            .await?;
        }
        Playlist::MediaPlaylist(mp) => {
            info!("Media Playlist found, {} segments", mp.segments.len());
            download_and_merge(
                mp,
                base_url,
                concurrency,
                retries,
                &temp_primary_str,
                &temp_dir,
                &multi_progress,
                reporter.clone(),
                request_context.clone(),
            )
            .await?;
        }
    }

    if let Some((audio_playlist, audio_base_url, rendition_name)) = external_audio_plan {
        emit_progress(
            &reporter,
            format!("Downloading HLS audio rendition: {}", rendition_name),
            0.48,
        );
        download_and_merge(
            audio_playlist,
            Some(audio_base_url),
            concurrency,
            retries,
            &temp_audio_str,
            &temp_dir,
            &multi_progress,
            staged_progress_reporter(reporter.clone(), "Audio", 0.48, 0.92),
            request_context.clone(),
        )
        .await?;
        merge_media_streams(
            &temp_primary,
            &temp_audio,
            output,
            video_bitrate,
            audio_bitrate,
            reporter.clone(),
        )
        .await?;
    } else {
        convert_to_mp4(
            &temp_primary_str,
            output,
            video_bitrate,
            audio_bitrate,
            &multi_progress,
            backend,
            reporter.clone(),
        )
        .await?;
    }

    if !keep_temp {
        let _ = fs::remove_file(&temp_primary).await;
        let _ = fs::remove_file(&temp_audio).await;
        let _ = fs::remove_dir(&temp_dir).await;
    }

    ensure_output_file_ready(Path::new(output)).await?;
    emit_progress(&reporter, "All tasks completed", 1.0);

    Ok(())
}

fn playlist_base_url(playlist_url: &Url) -> Url {
    let mut base = playlist_url.clone();
    base.set_query(None);
    base.set_fragment(None);
    let mut path = base.path().to_string();
    if let Some(position) = path.rfind('/') {
        path.truncate(position + 1);
        base.set_path(&path);
    }
    base
}

fn select_best_hls_variant(master: &MasterPlaylist) -> Option<&VariantStream> {
    master
        .variants
        .iter()
        .filter(|variant| !variant.is_i_frame && !variant.uri.trim().is_empty())
        .max_by_key(|variant| {
            let resolution_score = variant
                .resolution
                .as_ref()
                .map(|resolution| resolution.width * resolution.height)
                .unwrap_or(0);
            (resolution_score, variant.bandwidth)
        })
}

fn select_hls_audio_rendition<'a>(
    master: &'a MasterPlaylist,
    audio_group: Option<&str>,
) -> Option<&'a AlternativeMedia> {
    let audio_group = audio_group?;
    let candidates = || {
        master.alternatives.iter().filter(|rendition| {
            rendition.media_type == AlternativeMediaType::Audio && rendition.group_id == audio_group
        })
    };

    candidates()
        .find(|rendition| rendition.default)
        .or_else(|| candidates().find(|rendition| rendition.autoselect))
        .or_else(|| candidates().next())
}

fn staged_progress_reporter(
    parent: ProgressReporter,
    stage: &'static str,
    start: f64,
    end: f64,
) -> ProgressReporter {
    let span = (end - start).max(0.0);
    Arc::new(move |update| {
        parent(ProgressUpdate {
            message: format!("{}: {}", stage, update.message),
            progress: start + update.progress.clamp(0.0, 1.0) * span,
        });
    })
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
            let left_score = (
                left.height.max(0),
                left.width.max(0),
                left.quality_label.clone(),
            );
            let right_score = (
                right.height.max(0),
                right.width.max(0),
                right.quality_label.clone(),
            );
            right_score.cmp(&left_score)
        });
        self.candidates
    }
}

fn infer_title_from_url(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed.path_segments().and_then(|segments| {
                segments
                    .filter(|segment| !segment.is_empty())
                    .last()
                    .map(str::to_string)
            })
        })
        .map(|name| name.replace(['-', '_'], " "))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Untitled download".to_string())
}

fn normalize_source_url(input: &str) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        bail!("Source URL is empty");
    }

    if let Ok(url) = Url::parse(input) {
        if matches!(url.scheme(), "http" | "https") {
            return Ok(url.to_string());
        }
    }

    let url_pattern = Regex::new(r#"https?://[^\s<>\"']+"#)?;
    let raw = url_pattern
        .find(input)
        .map(|value| value.as_str())
        .ok_or_else(|| anyhow!("No HTTP or HTTPS URL was found in the shared text"))?;
    let candidate = raw.trim_end_matches(|character: char| {
        matches!(
            character,
            ',' | '.'
                | ';'
                | ':'
                | '!'
                | '?'
                | ')'
                | ']'
                | '}'
                | '，'
                | '。'
                | '；'
                | '：'
                | '！'
                | '？'
                | '）'
                | '】'
                | '》'
        )
    });
    let url = Url::parse(candidate).context("Invalid URL found in shared text")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("Only HTTP and HTTPS media sources are supported");
    }
    Ok(url.to_string())
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
    if let Ok(parsed) = Url::parse(url) {
        if let Some((_, mime)) = parsed
            .query_pairs()
            .find(|(name, _)| name == "mime" || name == "type")
        {
            let mime = mime.to_ascii_lowercase();
            if mime.contains("mp4") {
                return "mp4".to_string();
            }
            if mime.contains("webm") {
                return "webm".to_string();
            }
        }
    }
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

fn prepare_output_path(output: &str) -> Result<(PathBuf, PathBuf)> {
    let output_path = PathBuf::from(output);
    let output_dir = output_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    if !output_dir.exists() {
        std::fs::create_dir_all(&output_dir).with_context(|| {
            format!(
                "Failed to create output directory: {}",
                output_dir.display()
            )
        })?;
    }

    let temp_dir = create_unique_temp_dir(&output_dir, &output_path)?;
    Ok((output_path, temp_dir))
}

fn create_unique_temp_dir(root: &Path, output_path: &Path) -> Result<PathBuf> {
    if !root.exists() {
        std::fs::create_dir_all(root)
            .with_context(|| format!("Failed to create temporary root: {}", root.display()))?;
    }

    let stem = output_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let safe_stem = stem
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(48)
        .collect::<String>();
    let temp_dir = root.join(format!(
        ".ferrisload-{}-{}",
        safe_stem,
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&temp_dir).with_context(|| {
        format!(
            "Failed to create temporary directory: {}",
            temp_dir.display()
        )
    })?;
    Ok(temp_dir)
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

fn has_mp4_signature(prefix: &[u8]) -> bool {
    prefix
        .windows(4)
        .take(24)
        .any(|window| window == b"ftyp" || window == b"styp")
}

async fn ensure_output_file_ready(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Output file not found after download: {}", path.display()))?;
    if metadata.len() < 1024 {
        bail!(
            "Output file is too small to be valid media ({} bytes): {}",
            metadata.len(),
            path.display()
        );
    }

    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
    {
        let mut file = fs::File::open(path).await?;
        let mut prefix = [0u8; 64];
        let bytes_read = file.read(&mut prefix).await?;
        if !has_mp4_signature(&prefix[..bytes_read]) {
            bail!(
                "Output does not contain an MP4 file signature: {}",
                path.display()
            );
        }
    }

    let ffprobe_available = Command::new("ffprobe")
        .arg("-version")
        .output()
        .await
        .map(|output| output.status.success())
        .unwrap_or(false);
    if ffprobe_available {
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_type",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(path)
            .output()
            .await
            .context("Failed to run ffprobe media validation")?;
        let stream_type = String::from_utf8_lossy(&probe.stdout);
        if !probe.status.success() || !stream_type.lines().any(|line| line.trim() == "video") {
            bail!(
                "ffprobe did not find a playable video track in {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

async fn stream_media_response_to_file(
    response: Response,
    path: &Path,
    progress_start: f64,
    progress_end: f64,
    reporter: &ProgressReporter,
    label: &str,
) -> Result<()> {
    if let Some(content_type) = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase())
    {
        if content_type.starts_with("text/html")
            || content_type.starts_with("application/json")
            || content_type.starts_with("text/json")
        {
            bail!(
                "Media request returned non-media content type {}",
                content_type
            );
        }
    }

    let total = response.content_length();
    let mut stream = response.bytes_stream();
    let mut output = fs::File::create(path)
        .await
        .with_context(|| format!("Failed to create media file: {}", path.display()))?;
    let mut downloaded = 0u64;
    let mut last_reported = 0u64;
    emit_progress(reporter, label, progress_start);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        output.write_all(&chunk).await?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);

        if downloaded.saturating_sub(last_reported) >= 512 * 1024 {
            let progress = total
                .filter(|total| *total > 0)
                .map(|total| {
                    progress_start
                        + (downloaded as f64 / total as f64).clamp(0.0, 1.0)
                            * (progress_end - progress_start)
                })
                .unwrap_or(progress_start);
            let detail = total
                .map(|total| format!("{} / {}", human_bytes(downloaded), human_bytes(total)))
                .unwrap_or_else(|| human_bytes(downloaded));
            emit_progress(reporter, format!("{} [{}]", label, detail), progress);
            last_reported = downloaded;
        }
    }
    output.flush().await?;
    if downloaded == 0 {
        bail!("Media response was empty");
    }
    emit_progress(
        reporter,
        format!("{} [{}]", label, human_bytes(downloaded)),
        progress_end,
    );
    Ok(())
}

async fn download_with_retries(
    client: &Client,
    url: &str,
    path: &Path,
    retries: u8,
    progress_start: f64,
    progress_end: f64,
    reporter: &ProgressReporter,
    label: &str,
) -> Result<()> {
    for attempt in 1..=retries {
        let _ = fs::remove_file(path).await;
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

                match stream_media_response_to_file(
                    response,
                    path,
                    progress_start,
                    progress_end,
                    reporter,
                    label,
                )
                .await
                {
                    Ok(()) => return Ok(()),
                    Err(error) if attempt < retries => {
                        warn!(
                            "Media stream attempt {} failed for {}: {}",
                            attempt, url, error
                        );
                        let delay = retry_backoff_delay(attempt, None);
                        tokio::time::sleep(delay).await;
                    }
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("Failed to download media: {}", url));
                    }
                }
            }
            Err(error) => {
                if attempt == retries {
                    return Err(error)
                        .with_context(|| format!("Failed to download media: {}", url));
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
    let (manifest_bytes, effective_manifest_url) =
        download_playlist_with_url(manifest_url, request_context).await?;
    let manifest = String::from_utf8(manifest_bytes).context("DASH manifest is not valid UTF-8")?;
    resolve_dash_download_plan_from_manifest(effective_manifest_url.as_str(), &manifest)
}

fn resolve_dash_download_plan_from_manifest(
    manifest_url: &str,
    manifest: &str,
) -> Result<DashDownloadPlan> {
    let mpd: DashMpd =
        quick_xml::de::from_str(manifest).context("Failed to parse DASH manifest")?;
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
                ) else {
                    continue;
                };

                let candidate = DashRepresentationCandidate {
                    url,
                    bandwidth: representation.bandwidth.unwrap_or_default(),
                    width: representation.width.unwrap_or_default(),
                    height: representation.height.unwrap_or_default(),
                };

                if representation_type.starts_with("video/") || content_type.starts_with("video") {
                    let replace = best_video
                        .as_ref()
                        .map(|current| {
                            (candidate.height, candidate.width, candidate.bandwidth)
                                > (current.height, current.width, current.bandwidth)
                        })
                        .unwrap_or(true);
                    if replace {
                        best_video = Some(candidate);
                    }
                } else if representation_type.starts_with("audio/")
                    || content_type.starts_with("audio")
                {
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
        .find_map(|base| {
            join_manifest_url(manifest_base, mpd_base_urls, adaptation_base_urls, base)
        })
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
    use super::{
        bilibili_playurl_api_url, canonical_site_context, checksum_for_release_asset,
        has_mp4_signature, hls_response_bytes, normalize_source_url, parse_ytdlp_progress,
        playlist_base_url, resolve_dash_download_plan_from_manifest, resolve_hls_byte_range,
        select_best_hls_variant, select_hls_audio_rendition, should_auto_inspect_download_target,
        youtube_itag_from_media_url,
    };
    use m3u8_rs::{parse_playlist, ByteRange, Playlist};
    use reqwest::StatusCode;
    use url::Url;

    #[test]
    fn extracts_url_from_shared_text() {
        let source = "【视频分享】看看这个 https://b23.tv/AbC123 复制后打开";
        assert_eq!(
            normalize_source_url(source).expect("shared URL should resolve"),
            "https://b23.tv/AbC123"
        );
    }

    #[test]
    fn uses_effective_playlist_directory_as_base() {
        let url = Url::parse("https://cdn.example/live/quality/index.m3u8?token=abc")
            .expect("playlist URL should parse");
        assert_eq!(
            playlist_base_url(&url).as_str(),
            "https://cdn.example/live/quality/"
        );
    }

    #[test]
    fn builds_bilibili_playurl_api_requests_from_page_state() {
        let page_url = Url::parse("https://www.bilibili.com/video/BV1ab411c7mD?p=2")
            .expect("page URL should parse");
        let api_url = bilibili_playurl_api_url(
            &page_url,
            r#"window.__INITIAL_STATE__={"videoData":{"bvid":"BV1ab411c7mD","cid":7654321}}"#,
        )
        .expect("API URL should resolve")
        .expect("video metadata should be sufficient");
        let query = api_url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(api_url.path(), "/x/player/playurl");
        assert_eq!(
            query.get("bvid").map(|value| value.as_ref()),
            Some("BV1ab411c7mD")
        );
        assert_eq!(
            query.get("cid").map(|value| value.as_ref()),
            Some("7654321")
        );
        assert_eq!(query.get("fnval").map(|value| value.as_ref()), Some("4048"));

        let episode_url = Url::parse("https://www.bilibili.com/bangumi/play/ep987654")
            .expect("episode URL should parse");
        let episode_api = bilibili_playurl_api_url(&episode_url, "")
            .expect("episode API URL should resolve")
            .expect("episode id should be sufficient");
        assert_eq!(episode_api.path(), "/pgc/player/web/playurl");
        assert!(episode_api
            .query_pairs()
            .any(|(name, value)| { name == "ep_id" && value == "987654" }));
    }

    #[test]
    fn parses_real_ytdlp_progress_and_format_ids() {
        let (message, progress) = parse_ytdlp_progress("FERRISLOAD_PROGRESS| 37.5%|4.2MiB/s|00:12")
            .expect("progress line should parse");
        assert!(message.contains("37.5%"));
        assert!(message.contains("4.2MiB/s"));
        assert!((progress - 0.35).abs() < f64::EPSILON);
        assert_eq!(
            youtube_itag_from_media_url("https://video.example/playback?expire=1&itag=137"),
            Some("137".to_string())
        );
        assert!(
            youtube_itag_from_media_url("https://video.example/playback?itag=137%2B140").is_none()
        );
    }

    #[test]
    fn recognizes_mp4_file_signatures_instead_of_only_file_size() {
        assert!(has_mp4_signature(b"\0\0\0\x18ftypisom\0\0\0\0isom"));
        assert!(has_mp4_signature(b"\0\0\0\x18stypmsdh\0\0\0\0msdh"));
        assert!(!has_mp4_signature(b"<!doctype html><html>server error"));
    }

    #[test]
    fn accepts_only_the_requested_official_release_checksum() {
        let checksums =
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  yt-dlp\n\
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb *yt-dlp.exe\n";
        assert_eq!(
            checksum_for_release_asset(checksums, "yt-dlp.exe").as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert!(checksum_for_release_asset(checksums, "yt-dlp_macos").is_none());
    }

    #[test]
    fn selects_best_video_variant_and_default_audio_rendition() {
        let source = br#"#EXTM3U
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="main-audio",NAME="English",AUTOSELECT=YES,LANGUAGE="en",URI="en.m3u8"
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="main-audio",NAME="Original",DEFAULT=YES,AUTOSELECT=YES,LANGUAGE="ja",URI="original.m3u8"
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="commentary",NAME="Commentary",DEFAULT=YES,URI="commentary.m3u8"
#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=9000000,RESOLUTION=3840x2160,URI="iframe.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=854x480,AUDIO="main-audio"
480p.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1920x1080,AUDIO="main-audio"
1080p.m3u8
"#;
        let (_, playlist) = parse_playlist(source).expect("master playlist should parse");
        let Playlist::MasterPlaylist(master) = playlist else {
            panic!("expected a master playlist");
        };

        let variant = select_best_hls_variant(&master).expect("variant should be selected");
        assert_eq!(variant.uri, "1080p.m3u8");
        let rendition = select_hls_audio_rendition(&master, variant.audio.as_deref())
            .expect("audio rendition should be selected");
        assert_eq!(rendition.name, "Original");
        assert_eq!(rendition.uri.as_deref(), Some("original.m3u8"));
        assert!(select_hls_audio_rendition(&master, Some("missing")).is_none());
    }

    #[test]
    fn resolves_explicit_and_implicit_hls_byte_ranges() {
        let mut cursor = None;
        assert_eq!(
            resolve_hls_byte_range(
                "media.mp4",
                Some(&ByteRange {
                    length: 4,
                    offset: Some(2),
                }),
                &mut cursor,
            )
            .expect("explicit range should resolve"),
            Some((2, 5))
        );
        assert_eq!(
            resolve_hls_byte_range(
                "media.mp4",
                Some(&ByteRange {
                    length: 3,
                    offset: None,
                }),
                &mut cursor,
            )
            .expect("implicit range should continue from the prior range"),
            Some((6, 8))
        );
        assert!(resolve_hls_byte_range(
            "other.mp4",
            Some(&ByteRange {
                length: 2,
                offset: None,
            }),
            &mut cursor,
        )
        .is_err());
    }

    #[test]
    fn slices_full_responses_when_a_server_ignores_range() {
        assert_eq!(
            hls_response_bytes(StatusCode::OK, &[0, 1, 2, 3, 4, 5], Some((2, 4)))
                .expect("full response should be sliced"),
            vec![2, 3, 4]
        );
        assert_eq!(
            hls_response_bytes(StatusCode::PARTIAL_CONTENT, &[7, 8, 9], Some((100, 102)),)
                .expect("partial response should already represent the range"),
            vec![7, 8, 9]
        );
    }

    #[test]
    fn maps_video_cdns_back_to_their_site_context() {
        assert_eq!(
            canonical_site_context("upos-sz-mirrorcos.bilivideo.com"),
            (
                "https://www.bilibili.com/".to_string(),
                "https://www.bilibili.com".to_string()
            )
        );
        assert_eq!(
            canonical_site_context("rr1---sn.example.googlevideo.com").0,
            "https://www.youtube.com/"
        );
    }

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
        assert_eq!(
            plan.audio_url.as_deref(),
            Some("https://audio.example/256.m4a")
        );
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

        assert_eq!(
            plan.video_url,
            "https://manifest.example/root/media/video/stream.mp4"
        );
        assert_eq!(
            plan.audio_url.as_deref(),
            Some("https://manifest.example/root/media/audio/track.m4a")
        );
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
    reporter: ProgressReporter,
) -> Result<()> {
    let requires_reencode = video_bitrate > 0 || audio_bitrate > 0;

    #[cfg(target_os = "android")]
    {
        if let Some(transcoder) = ANDROID_HW_TRANSCODER.get() {
            if !requires_reencode {
                emit_progress(
                    &reporter,
                    "Merging streams with Android MediaMuxer (no re-encoding)",
                    0.9,
                );
                match transcoder
                    .mux(
                        video_path.to_string_lossy().as_ref(),
                        audio_path.to_string_lossy().as_ref(),
                        output_path,
                    )
                    .await
                {
                    Ok(_) => return Ok(()),
                    Err(e) => warn!(
                        "Android MediaMuxer merge failed, falling back if possible: {}",
                        e
                    ),
                }
            } else {
                let mux_input = video_path.with_file_name(format!(
                    "merged_transcode_input_{}.mp4",
                    uuid::Uuid::new_v4().simple()
                ));
                emit_progress(
                    &reporter,
                    "Preparing separate streams with Android MediaMuxer",
                    0.88,
                );
                let mux_result = transcoder
                    .mux(
                        video_path.to_string_lossy().as_ref(),
                        audio_path.to_string_lossy().as_ref(),
                        mux_input.to_string_lossy().as_ref(),
                    )
                    .await;

                if let Err(error) = mux_result {
                    warn!(
                        "Android MediaMuxer preparation failed, falling back if possible: {}",
                        error
                    );
                } else {
                    emit_progress(&reporter, "Using Android MediaCodec hardware encoder", 0.93);
                    let transcode_result = transcoder
                        .transcode(
                            mux_input.to_string_lossy().as_ref(),
                            output_path,
                            video_bitrate,
                            audio_bitrate,
                        )
                        .await;
                    let _ = fs::remove_file(&mux_input).await;
                    match transcode_result {
                        Ok(()) => return Ok(()),
                        Err(error) => {
                            warn!(
                                "Android MediaCodec hardware encode failed, falling back if possible: {}",
                                error
                            );
                            let _ = fs::remove_file(output_path).await;
                        }
                    }
                }
                let _ = fs::remove_file(&mux_input).await;
            }
        }
    }

    let ffmpeg_path = resolve_ffmpeg_path()
        .await
        .ok_or_else(|| anyhow!("FFmpeg is required to merge separated audio and video streams"))?;

    let mut selected_accel = if requires_reencode {
        detect_acceleration(&ffmpeg_path).await?
    } else {
        AccelType::CPU
    };
    if requires_reencode {
        emit_progress(
            &reporter,
            format!(
                "Using {} to merge and encode streams",
                selected_accel.label()
            ),
            0.92,
        );
    } else {
        emit_progress(
            &reporter,
            "Merging streams with FFmpeg stream copy (no re-encoding)",
            0.92,
        );
    }

    let mut output = run_ffmpeg_merge(
        video_path,
        audio_path,
        output_path,
        video_bitrate,
        audio_bitrate,
        selected_accel,
        &ffmpeg_path,
    )
    .await?;

    if !output.status.success() && requires_reencode && selected_accel != AccelType::CPU {
        warn!(
            "{} merge encode failed, retrying with CPU libx264: {}",
            selected_accel.label(),
            String::from_utf8_lossy(&output.stderr)
        );
        emit_progress(
            &reporter,
            format!(
                "{} unavailable; retrying merge with CPU libx264",
                selected_accel.label()
            ),
            0.94,
        );
        selected_accel = AccelType::CPU;
        let _ = fs::remove_file(output_path).await;
        output = run_ffmpeg_merge(
            video_path,
            audio_path,
            output_path,
            video_bitrate,
            audio_bitrate,
            selected_accel,
            &ffmpeg_path,
        )
        .await?;
    }

    if !output.status.success() {
        bail!(
            "FFmpeg merge failed with {}: {}",
            selected_accel.label(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(())
}

async fn run_ffmpeg_merge(
    video_path: &Path,
    audio_path: &Path,
    output_path: &str,
    video_bitrate: u32,
    audio_bitrate: u32,
    accel: AccelType,
    ffmpeg_path: &Path,
) -> Result<std::process::Output> {
    let requires_reencode = video_bitrate > 0 || audio_bitrate > 0;

    let mut args = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-i".to_string(),
        video_path.to_string_lossy().to_string(),
        "-i".to_string(),
        audio_path.to_string_lossy().to_string(),
        "-map".to_string(),
        "0:v:0".to_string(),
        "-map".to_string(),
        "1:a:0".to_string(),
    ];

    if !requires_reencode {
        args.extend(["-c".to_string(), "copy".to_string()]);
    } else {
        args.extend([
            "-c:v".to_string(),
            accel.encoder().to_string(),
            "-c:a".to_string(),
            "aac".to_string(),
        ]);
        match accel {
            AccelType::Nvidia => args.extend([
                "-preset".to_string(),
                "p4".to_string(),
                "-rc".to_string(),
                "vbr".to_string(),
            ]),
            AccelType::CPU => args.extend(["-preset".to_string(), "medium".to_string()]),
            AccelType::LinuxVaapi => args.extend([
                "-vaapi_device".to_string(),
                "/dev/dri/renderD128".to_string(),
                "-vf".to_string(),
                "format=nv12,hwupload".to_string(),
            ]),
            AccelType::AMD | AccelType::IntelQuickSync | AccelType::AppleVideoToolbox => {}
        }
        if video_bitrate > 0 {
            args.push("-b:v".to_string());
            args.push(format!("{}k", video_bitrate));
        }
        if audio_bitrate > 0 {
            args.push("-b:a".to_string());
            args.push(format!("{}k", audio_bitrate));
        }
    }

    args.extend(["-movflags".to_string(), "+faststart".to_string()]);
    args.push(output_path.to_string());

    Command::new(ffmpeg_path)
        .args(&args)
        .output()
        .await
        .context("FFmpeg merge process failed")
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
        let left_score = (
            left.score,
            left.height.max(0),
            left.duration_seconds as i64,
            left.segment_count,
        );
        let right_score = (
            right.score,
            right.height.max(0),
            right.duration_seconds as i64,
            right.segment_count,
        );
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
        if let Ok((segments, duration)) =
            inspect_hls_metadata(&candidate.media_url, request_context).await
        {
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
    if lower_quality.contains("1080")
        || lower_quality.contains("720")
        || lower_quality.contains("高")
    {
        score += 120;
    }
    for marker in [
        "preview", "试看", "trial", "sample", "ad", "ads", "promo", "trailer", "thumb", "sprite",
        "teaser",
    ] {
        if lower_url.contains(marker) || lower_quality.contains(marker) {
            score -= 500;
            reasons.push(format!("deprioritized:{}", marker));
        }
    }
    if lower_url.contains("master") || lower_url.contains("index") || lower_url.contains("playlist")
    {
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
    let (_, playlist) =
        parse_playlist(&bytes).map_err(|e| anyhow!("Failed to parse HLS metadata: {:?}", e))?;
    match playlist {
        Playlist::MediaPlaylist(media) => {
            let duration = media
                .segments
                .iter()
                .map(|segment| segment.duration as f64)
                .sum();
            Ok((media.segments.len(), duration))
        }
        Playlist::MasterPlaylist(master) => Ok((master.variants.len(), 0.0)),
    }
}

fn create_http_client_for_context(
    source_url: Option<&str>,
    request_context: &RequestContext,
) -> Result<Client> {
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
                let (default_referer, default_origin) = canonical_site_context(domain);
                let referer = if request_context.referer.trim().is_empty() {
                    default_referer.as_str()
                } else {
                    request_context.referer.trim()
                };
                let origin = if request_context.origin.trim().is_empty() {
                    default_origin.as_str()
                } else {
                    request_context.origin.trim()
                };
                headers.insert(header::REFERER, header::HeaderValue::from_str(referer)?);
                headers.insert(header::ORIGIN, header::HeaderValue::from_str(origin)?);
            }
        }
    }

    if !request_context.cookie.trim().is_empty() {
        headers.insert(
            header::COOKIE,
            header::HeaderValue::from_str(request_context.cookie.trim())?,
        );
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

fn canonical_site_context(domain: &str) -> (String, String) {
    let lower = domain.to_ascii_lowercase();
    if lower.contains("bilibili")
        || lower.contains("b23.tv")
        || lower.contains("bilivideo")
        || lower.contains("biliapi")
    {
        return (
            "https://www.bilibili.com/".to_string(),
            "https://www.bilibili.com".to_string(),
        );
    }
    if lower.contains("youtube") || lower.contains("youtu.be") || lower.contains("googlevideo") {
        return (
            "https://www.youtube.com/".to_string(),
            "https://www.youtube.com".to_string(),
        );
    }

    (
        format!("https://{}/", domain),
        format!("https://{}", domain),
    )
}

async fn download_playlist(url: &str, request_context: &RequestContext) -> Result<Vec<u8>> {
    let (bytes, _) = download_playlist_with_url(url, request_context).await?;
    Ok(bytes)
}

async fn download_playlist_with_url(
    url: &str,
    request_context: &RequestContext,
) -> Result<(Vec<u8>, Url)> {
    let client = create_http_client_for_context(Some(url), request_context)?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        bail!("Failed to download playlist: HTTP {}", response.status());
    }
    let effective_url = response.url().clone();
    Ok((response.bytes().await?.to_vec(), effective_url))
}

async fn check_ffmpeg() -> bool {
    resolve_ffmpeg_path().await.is_some()
}

async fn select_transcoder_backend() -> Result<TranscoderKind> {
    #[cfg(target_os = "android")]
    if ANDROID_HW_TRANSCODER.get().is_some() {
        return Ok(TranscoderKind::AndroidHardware);
    }

    if let Some(ffmpeg_path) = resolve_ffmpeg_path().await {
        let accel = detect_acceleration(&ffmpeg_path).await?;
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

#[derive(Clone, Debug)]
struct HlsResourceRequest {
    url: String,
    byte_range: Option<(u64, u64)>,
}

fn resolve_hls_byte_range(
    uri: &str,
    byte_range: Option<&ByteRange>,
    cursor: &mut Option<(String, u64)>,
) -> Result<Option<(u64, u64)>> {
    let Some(byte_range) = byte_range else {
        *cursor = None;
        return Ok(None);
    };
    if byte_range.length == 0 {
        bail!("HLS byte range length must be greater than zero");
    }

    let start = match byte_range.offset {
        Some(offset) => offset,
        None => cursor
            .as_ref()
            .filter(|(previous_uri, _)| previous_uri == uri)
            .map(|(_, next_offset)| *next_offset)
            .ok_or_else(|| {
                anyhow!(
                    "HLS byte range for {} omitted its offset without a compatible previous range",
                    uri
                )
            })?,
    };
    let end_exclusive = start
        .checked_add(byte_range.length)
        .ok_or_else(|| anyhow!("HLS byte range overflow for {}", uri))?;
    *cursor = Some((uri.to_string(), end_exclusive));
    Ok(Some((start, end_exclusive - 1)))
}

fn hls_response_bytes(
    status: reqwest::StatusCode,
    data: &[u8],
    byte_range: Option<(u64, u64)>,
) -> Result<Vec<u8>> {
    let Some((start, end)) = byte_range else {
        return Ok(data.to_vec());
    };
    let expected_length = end
        .checked_sub(start)
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| anyhow!("Invalid HLS byte range {}-{}", start, end))?;
    let expected_length = usize::try_from(expected_length)
        .context("HLS byte range is too large for this platform")?;

    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        if data.len() < expected_length {
            bail!(
                "HLS range response was truncated: expected {} bytes, received {}",
                expected_length,
                data.len()
            );
        }
        return Ok(data[..expected_length].to_vec());
    }

    let start = usize::try_from(start).context("HLS byte range offset is too large")?;
    let end_exclusive = start
        .checked_add(expected_length)
        .ok_or_else(|| anyhow!("HLS byte range slice overflow"))?;
    if data.len() < end_exclusive {
        bail!(
            "Server ignored the HLS Range request and returned only {} bytes for range {}-{}",
            data.len(),
            start,
            end
        );
    }
    Ok(data[start..end_exclusive].to_vec())
}

async fn download_hls_resource(
    client: &Client,
    request: &HlsResourceRequest,
    retries: u8,
) -> Result<Vec<u8>> {
    for attempt in 1..=retries {
        let mut builder = client.get(&request.url);
        if let Some((start, end)) = request.byte_range {
            builder = builder.header(header::RANGE, format!("bytes={}-{}", start, end));
        }

        match builder.send().await {
            Ok(response) if response.status().is_success() => {
                let status = response.status();
                let bytes = response.bytes().await?;
                match hls_response_bytes(status, &bytes, request.byte_range) {
                    Ok(data) => return Ok(data),
                    Err(error) => {
                        warn!(
                            "Attempt {} returned an invalid HLS resource {}: {}",
                            attempt, request.url, error
                        );
                    }
                }
            }
            Ok(response) => {
                warn!(
                    "Attempt {} failed: {} HTTP {}",
                    attempt,
                    request.url,
                    response.status()
                );
                if attempt < retries {
                    let delay = retry_backoff_delay(attempt, Some(response.status().as_u16()));
                    tokio::time::sleep(delay).await;
                }
                continue;
            }
            Err(error) => {
                warn!(
                    "Attempt {} request error: {} - {}",
                    attempt, request.url, error
                );
            }
        }

        if attempt < retries {
            let delay = retry_backoff_delay(attempt, None);
            tokio::time::sleep(delay).await;
        }
    }

    bail!("Failed after {} attempts: {}", retries, request.url)
}

fn decrypt_hls_resource(data: Vec<u8>, crypto: Option<&(Vec<u8>, Vec<u8>)>) -> Result<Vec<u8>> {
    let Some((key, iv)) = crypto else {
        return Ok(data);
    };
    if key.len() != 16 || iv.len() != 16 {
        bail!("AES-128 key and IV must contain exactly 16 bytes");
    }
    let cipher = Aes128Cbc::new_from_slices(key, iv)?;
    Ok(cipher.decrypt_vec(&data)?)
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

    let media_sequence = playlist.media_sequence;
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

    let key_client =
        create_http_client_for_context(base_url.as_ref().map(Url::as_str), &request_context)?;
    let mut key_cache = HashMap::<String, Vec<u8>>::new();
    let mut segment_crypto = Vec::with_capacity(total);
    let mut segment_has_explicit_iv = Vec::with_capacity(total);
    for (index, segment) in segments.iter().enumerate() {
        let Some(key_definition) = segment.key.as_ref() else {
            segment_crypto.push(None);
            segment_has_explicit_iv.push(false);
            continue;
        };
        match &key_definition.method {
            KeyMethod::None => {
                segment_crypto.push(None);
                segment_has_explicit_iv.push(false);
                continue;
            }
            KeyMethod::AES128 => {}
            method => bail!("Unsupported HLS encryption method: {:?}", method),
        }
        if key_definition
            .keyformat
            .as_deref()
            .is_some_and(|format| !format.eq_ignore_ascii_case("identity"))
        {
            bail!(
                "Unsupported HLS AES-128 key format: {}",
                key_definition.keyformat.as_deref().unwrap_or_default()
            );
        }
        let key_uri = key_definition
            .uri
            .as_deref()
            .ok_or_else(|| anyhow!("AES-128 HLS key is missing its URI"))?;
        let key_url = if let Some(base) = &base_url {
            base.join(key_uri)?
        } else {
            Url::parse(key_uri)?
        };
        let key_url_string = key_url.to_string();
        let key_bytes = if let Some(cached) = key_cache.get(&key_url_string) {
            cached.clone()
        } else {
            let bytes = download_hls_resource(
                &key_client,
                &HlsResourceRequest {
                    url: key_url_string.clone(),
                    byte_range: None,
                },
                retries,
            )
            .await?;
            if bytes.len() != 16 {
                bail!("AES-128 key must contain exactly 16 bytes");
            }
            key_cache.insert(key_url_string, bytes.clone());
            bytes
        };

        let iv_bytes = if let Some(iv_hex) = &key_definition.iv {
            let raw = iv_hex.trim_start_matches("0x");
            let padded = if raw.len() < 32 {
                format!("{:0>32}", raw)
            } else {
                raw.to_string()
            };
            let decoded = hex::decode(&padded).context("IV hex decode failed")?;
            if decoded.len() != 16 {
                bail!("AES-128 IV must contain exactly 16 bytes");
            }
            decoded
        } else {
            ((media_sequence as u128) + (index as u128))
                .to_be_bytes()
                .to_vec()
        };

        segment_crypto.push(Some((key_bytes, iv_bytes)));
        segment_has_explicit_iv.push(key_definition.iv.is_some());
    }

    let resolve_url = |uri: &str| -> Result<String> {
        if let Some(base) = &base_url {
            Ok(base.join(uri)?.to_string())
        } else {
            Ok(Url::parse(uri)?.to_string())
        }
    };
    let mut segment_range_cursor = None;
    let mut map_range_cursor = None;
    let mut previous_map = None;
    let mut segment_requests = Vec::with_capacity(total);
    let mut init_requests = Vec::with_capacity(total);

    for (index, segment) in segments.iter().enumerate() {
        segment_requests.push(HlsResourceRequest {
            url: resolve_url(&segment.uri)?,
            byte_range: resolve_hls_byte_range(
                &segment.uri,
                segment.byte_range.as_ref(),
                &mut segment_range_cursor,
            )?,
        });

        let map_changed = segment.map != previous_map;
        let init_request = if map_changed {
            match segment.map.as_ref() {
                Some(map) => {
                    if segment_crypto[index].is_some() && !segment_has_explicit_iv[index] {
                        bail!("Encrypted EXT-X-MAP requires an explicit AES-128 IV");
                    }
                    Some(HlsResourceRequest {
                        url: resolve_url(&map.uri)?,
                        byte_range: resolve_hls_byte_range(
                            &map.uri,
                            map.byte_range.as_ref(),
                            &mut map_range_cursor,
                        )?,
                    })
                }
                None => {
                    map_range_cursor = None;
                    None
                }
            }
        } else {
            None
        };
        previous_map = segment.map.clone();
        init_requests.push(init_request);
    }

    let sem = Arc::new(Semaphore::new(concurrency));
    let client = Arc::new(create_http_client_for_context(
        base_url.as_ref().map(Url::as_str),
        &request_context,
    )?);
    let completed = Arc::new(Mutex::new(0u64));

    let temp_dir = temp_dir.to_path_buf();

    let tasks = stream::iter(0..total)
        .map(|idx| {
            let segment_request = segment_requests[idx].clone();
            let init_request = init_requests[idx].clone();
            let client = client.clone();
            let sem = sem.clone();
            let key = segment_crypto.get(idx).cloned().flatten();
            let pb = download_pb.clone();
            let completed = completed.clone();
            let reporter = reporter.clone();
            let temp_dir = temp_dir.clone();

            tokio::spawn(async move {
                let _permit = sem
                    .acquire()
                    .await
                    .map_err(|_| anyhow!("Semaphore acquire failed"))?;

                let mut buffer = Vec::new();
                if let Some(init_request) = init_request {
                    let init_data = download_hls_resource(&client, &init_request, retries).await?;
                    let init_data = decrypt_hls_resource(init_data, key.as_ref())?;
                    buffer.extend_from_slice(&init_data);
                }
                let segment_data =
                    download_hls_resource(&client, &segment_request, retries).await?;
                let segment_data = decrypt_hls_resource(segment_data, key.as_ref())?;
                buffer.extend_from_slice(&segment_data);

                let file_name = format!("seg_{:05}.part", idx);
                let tmp_path = temp_dir.join(file_name);
                fs::write(&tmp_path, &buffer).await.with_context(|| {
                    format!(
                        "Failed to write segment: {} (url: {})",
                        tmp_path.display(),
                        segment_request.url
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

                Ok::<(), anyhow::Error>(())
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
        let file_name = format!("seg_{:05}.part", i);
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
        let file_name = format!("seg_{:05}.part", index);
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

async fn detect_acceleration(ffmpeg_path: &Path) -> Result<AccelType> {
    let output = Command::new(ffmpeg_path)
        .args(["-hide_banner", "-encoders"])
        .output()
        .await
        .context("Failed to run ffmpeg")?;

    let list = String::from_utf8_lossy(&output.stdout);
    let candidates = [
        AccelType::Nvidia,
        AccelType::AMD,
        AccelType::IntelQuickSync,
        AccelType::LinuxVaapi,
        AccelType::AppleVideoToolbox,
    ];
    for candidate in candidates {
        if list.contains(candidate.encoder()) && probe_ffmpeg_encoder(ffmpeg_path, candidate).await
        {
            return Ok(candidate);
        }
    }
    if list.contains(AccelType::CPU.encoder())
        && probe_ffmpeg_encoder(ffmpeg_path, AccelType::CPU).await
    {
        return Ok(AccelType::CPU);
    }
    bail!("FFmpeg is installed but no usable H.264 encoder passed the runtime probe")
}

async fn probe_ffmpeg_encoder(ffmpeg_path: &Path, accel: AccelType) -> bool {
    let mut args = vec![
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=64x64:r=1",
        "-frames:v",
        "1",
        "-an",
        "-c:v",
        accel.encoder(),
    ];
    if accel == AccelType::LinuxVaapi {
        args.extend([
            "-vaapi_device",
            "/dev/dri/renderD128",
            "-vf",
            "format=nv12,hwupload",
        ]);
    }
    args.extend(["-f", "null", "-"]);

    match Command::new(ffmpeg_path).args(args).output().await {
        Ok(output) => output.status.success(),
        Err(_) => false,
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
            let requires_reencode = video_bitrate > 0 || audio_bitrate > 0;
            if requires_reencode {
                info!("Using FFmpeg backend: {}", accel.label());
                emit_progress(&reporter, format!("Using {} encoder", accel.label()), 0.955);
            } else {
                info!("Using FFmpeg stream copy without re-encoding");
                emit_progress(
                    &reporter,
                    "Remuxing with FFmpeg stream copy (no hardware encoder)",
                    0.955,
                );
            }

            let ffmpeg_path = resolve_ffmpeg_path()
                .await
                .ok_or_else(|| anyhow!("FFmpeg became unavailable before conversion"))?;
            let mut selected_accel = accel;
            let mut output = run_ffmpeg_conversion(
                input_ts,
                output_path,
                video_bitrate,
                audio_bitrate,
                selected_accel,
                &ffmpeg_path,
            )
            .await?;

            if !output.status.success() && selected_accel != AccelType::CPU && requires_reencode {
                warn!(
                    "{} failed, retrying with CPU libx264: {}",
                    selected_accel.label(),
                    String::from_utf8_lossy(&output.stderr)
                );
                emit_progress(
                    &reporter,
                    format!(
                        "{} unavailable; retrying with CPU libx264",
                        selected_accel.label()
                    ),
                    0.96,
                );
                selected_accel = AccelType::CPU;
                output = run_ffmpeg_conversion(
                    input_ts,
                    output_path,
                    video_bitrate,
                    audio_bitrate,
                    selected_accel,
                    &ffmpeg_path,
                )
                .await?;
            }

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                convert_pb.finish_with_message("MP4 transcode failed");
                error!("FFmpeg stderr:\n{}", stderr);
                bail!("MP4 transcode failed with {}", selected_accel.label());
            }

            convert_pb.finish_with_message("MP4 transcode complete");
            info!("Output file: {}", output_path);

            let out_meta = std::fs::metadata(output_path)
                .context("Transcode output file not found after FFmpeg")?;
            if out_meta.len() < 1024 {
                bail!(
                    "Transcode output file is too small ({} bytes), likely corrupted",
                    out_meta.len()
                );
            }

            Ok(())
        }
        TranscoderKind::AndroidHardware => {
            if video_bitrate > 0 || audio_bitrate > 0 {
                info!("Using Android MediaCodec hardware encoder");
                emit_progress(
                    &reporter,
                    "Using Android MediaCodec hardware encoder",
                    0.955,
                );
            } else {
                info!("Using Android MediaMuxer with MediaCodec fallback");
                emit_progress(
                    &reporter,
                    "Remuxing with Android MediaMuxer (MediaCodec fallback if needed)",
                    0.955,
                );
            }
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
                bail!(
                    "Transcode output file is too small ({} bytes), likely corrupted",
                    out_meta.len()
                );
            }

            Ok(())
        }
    }
}

async fn run_ffmpeg_conversion(
    input_path: &str,
    output_path: &str,
    video_bitrate: u32,
    audio_bitrate: u32,
    accel: AccelType,
    ffmpeg_path: &Path,
) -> Result<std::process::Output> {
    let mut args = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "info".to_string(),
        "-i".to_string(),
        input_path.to_string(),
    ];

    if video_bitrate == 0 && audio_bitrate == 0 {
        info!("Bitrates are 0, attempting a lossless stream remux");
        args.extend([
            "-c".to_string(),
            "copy".to_string(),
            "-bsf:a".to_string(),
            "aac_adtstoasc".to_string(),
        ]);
    } else {
        args.extend([
            "-c:v".to_string(),
            accel.encoder().to_string(),
            "-c:a".to_string(),
            "aac".to_string(),
        ]);
        match accel {
            AccelType::Nvidia => args.extend([
                "-preset".to_string(),
                "p4".to_string(),
                "-rc".to_string(),
                "vbr".to_string(),
            ]),
            AccelType::CPU => args.extend(["-preset".to_string(), "medium".to_string()]),
            AccelType::LinuxVaapi => args.extend([
                "-vaapi_device".to_string(),
                "/dev/dri/renderD128".to_string(),
                "-vf".to_string(),
                "format=nv12,hwupload".to_string(),
            ]),
            AccelType::AMD | AccelType::IntelQuickSync | AccelType::AppleVideoToolbox => {}
        }
        if video_bitrate > 0 {
            args.extend(["-b:v".to_string(), format!("{}k", video_bitrate)]);
        }
        args.extend([
            "-b:a".to_string(),
            format!(
                "{}k",
                if audio_bitrate > 0 {
                    audio_bitrate
                } else {
                    256
                }
            ),
        ]);
    }

    args.extend([
        "-movflags".to_string(),
        "+faststart".to_string(),
        output_path.to_string(),
    ]);

    Command::new(ffmpeg_path)
        .args(&args)
        .output()
        .await
        .context("FFmpeg conversion process failed")
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
