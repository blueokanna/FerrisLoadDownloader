#![allow(unused_imports, unused_variables, dead_code)]
use crate::api::site_adapters::{extractor_name_for_host, inspect_page_candidates, SiteWarning};
use crate::crypto::{aes_128_cbc_decrypt, sha256, Sha256};
use crate::frb_generated::StreamSink;
use crate::hls::{
    parse_playlist, AlternativeMedia, AlternativeMediaType, ByteRange, KeyMethod, Map,
    MasterPlaylist, MediaPlaylist, Playlist, VariantStream,
};
use crate::net::SyncHttpClient;
use crate::xml::{self, Element};
use anyhow::{anyhow, bail, Context, Result};
use ferrisload_core::{DownloadPlan, DOWNLOAD_PLAN_VERSION};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::{error, info, warn};
use nextjson::Value;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(target_os = "android")]
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::Duration;
use url::Url;
use uuid::Uuid;

#[cfg(target_os = "android")]
use jni::objects::{GlobalRef, JClass, JObject, JValue};
#[cfg(target_os = "android")]
use jni::JNIEnv;
#[cfg(target_os = "android")]
use jni::JavaVM;

pub(crate) type ProgressReporter = Arc<dyn Fn(ProgressUpdate) + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccelType {
    Nvidia,
    Amd,
    IntelQuickSync,
    LinuxVaapi,
    AppleVideoToolbox,
    Cpu,
}

impl AccelType {
    fn label(self) -> &'static str {
        match self {
            Self::Nvidia => "NVIDIA NVENC",
            Self::Amd => "AMD AMF",
            Self::IntelQuickSync => "Intel Quick Sync",
            Self::LinuxVaapi => "Linux VAAPI",
            Self::AppleVideoToolbox => "Apple VideoToolbox",
            Self::Cpu => "CPU libx264",
        }
    }

    fn encoder(self) -> &'static str {
        match self {
            Self::Nvidia => "h264_nvenc",
            Self::Amd => "h264_amf",
            Self::IntelQuickSync => "h264_qsv",
            Self::LinuxVaapi => "h264_vaapi",
            Self::AppleVideoToolbox => "h264_videotoolbox",
            Self::Cpu => "libx264",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum TranscoderKind {
    Ffmpeg(AccelType),
    AndroidHardware,
    /// Native iOS AVFoundation / VideoToolbox hardware H.264 pipeline.
    /// AVAssetWriter selects the hardware encoder automatically on every
    /// Apple device (A-series and M-series chips); there is no software
    /// H.264 encoder on iOS, so this is the only correct backend there.
    IosVideoToolbox,
}

// FFI into the native iOS transcoder (`ios/Runner/VideoToolboxBridge.m`).
//
// The symbols are provided by the Runner app target, which links the Rust
// static library; they only exist on iOS, hence the `cfg` gate.
#[cfg(target_os = "ios")]
extern "C" {
    fn ferrisload_videotoolbox_available() -> i32;
    fn ferrisload_videotoolbox_transcode(
        input: *const std::os::raw::c_char,
        output: *const std::os::raw::c_char,
        video_bitrate: i32,
        audio_bitrate: i32,
        expected_ms: i64,
        errbuf: *mut std::os::raw::c_char,
        errbuf_len: usize,
    ) -> i32;
    fn ferrisload_videotoolbox_mux(
        video: *const std::os::raw::c_char,
        audio: *const std::os::raw::c_char,
        output: *const std::os::raw::c_char,
        expected_ms: i64,
        errbuf: *mut std::os::raw::c_char,
        errbuf_len: usize,
    ) -> i32;
}

/// Thin Rust wrappers around the iOS VideoToolbox FFI. They mirror the
/// Android JNI helpers: bound the operation with a wall-clock timeout on the
/// Rust side (defense in depth — the Objective-C side also has internal
/// bounded waits) and surface the native error string.
#[cfg(target_os = "ios")]
mod ios_videotoolbox {
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;
    use std::time::Duration;

    use anyhow::{anyhow, Context, Result};

    // The `extern "C"` items live in the parent module; bring them into scope
    // explicitly (they do not resolve through parent-module lookup).
    use super::{
        ferrisload_videotoolbox_available, ferrisload_videotoolbox_mux,
        ferrisload_videotoolbox_transcode,
    };

    const ERROR_BUF_LEN: usize = 512;

    pub fn available() -> bool {
        // SAFETY: no arguments; a plain boolean probe into a fixed symbol.
        unsafe { ferrisload_videotoolbox_available() != 0 }
    }

    fn take_error(errbuf: &[c_char]) -> String {
        // SAFETY: errbuf is always NUL-terminated by the C side on failure.
        unsafe { CStr::from_ptr(errbuf.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    pub fn transcode(
        input: &str,
        output: &str,
        video_bitrate: u32,
        audio_bitrate: u32,
        expected_duration: Option<f64>,
        timeout: Duration,
    ) -> Result<()> {
        let input_c = CString::new(input).context("iOS input path contains NUL")?;
        let output_c = CString::new(output).context("iOS output path contains NUL")?;
        let expected_ms = (expected_duration.unwrap_or(0.0) * 1000.0).round() as i64;
        let (tx, rx) = std::sync::mpsc::channel::<Result<()>>();
        std::thread::Builder::new()
            .name("ferrisload-ios-videotoolbox".into())
            .spawn(move || {
                let result = (|| {
                    let mut errbuf = [0 as c_char; ERROR_BUF_LEN];
                    // SAFETY: all pointers are valid for the call; errbuf is a
                    // writable buffer of the declared length.
                    let ok = unsafe {
                        ferrisload_videotoolbox_transcode(
                            input_c.as_ptr(),
                            output_c.as_ptr(),
                            video_bitrate as i32,
                            audio_bitrate as i32,
                            expected_ms,
                            errbuf.as_mut_ptr(),
                            errbuf.len(),
                        )
                    };
                    if ok != 0 {
                        Ok(())
                    } else {
                        Err(anyhow!(
                            "iOS VideoToolbox transcode failed: {}",
                            take_error(&errbuf)
                        ))
                    }
                })();
                let _ = tx.send(result);
            })
            .context("Failed to spawn iOS transcoder thread")?;
        rx.recv_timeout(timeout).map_err(|_| {
            anyhow!(
                "iOS VideoToolbox transcode timed out after {}s",
                timeout.as_secs()
            )
        })?
    }

    pub fn mux(
        video: &str,
        audio: &str,
        output: &str,
        expected_duration: Option<f64>,
        timeout: Duration,
    ) -> Result<()> {
        let video_c = CString::new(video).context("iOS video path contains NUL")?;
        let audio_c = CString::new(audio).context("iOS audio path contains NUL")?;
        let output_c = CString::new(output).context("iOS output path contains NUL")?;
        let expected_ms = (expected_duration.unwrap_or(0.0) * 1000.0).round() as i64;
        let (tx, rx) = std::sync::mpsc::channel::<Result<()>>();
        std::thread::Builder::new()
            .name("ferrisload-ios-videotoolbox-mux".into())
            .spawn(move || {
                let result = (|| {
                    let mut errbuf = [0 as c_char; ERROR_BUF_LEN];
                    // SAFETY: see `transcode`.
                    let ok = unsafe {
                        ferrisload_videotoolbox_mux(
                            video_c.as_ptr(),
                            audio_c.as_ptr(),
                            output_c.as_ptr(),
                            expected_ms,
                            errbuf.as_mut_ptr(),
                            errbuf.len(),
                        )
                    };
                    if ok != 0 {
                        Ok(())
                    } else {
                        Err(anyhow!(
                            "iOS VideoToolbox mux failed: {}",
                            take_error(&errbuf)
                        ))
                    }
                })();
                let _ = tx.send(result);
            })
            .context("Failed to spawn iOS mux thread")?;
        rx.recv_timeout(timeout).map_err(|_| {
            anyhow!(
                "iOS VideoToolbox mux timed out after {}s",
                timeout.as_secs()
            )
        })?
    }
}

/// Wall-clock cap for a single iOS VideoToolbox operation, derived from the
/// expected media duration (same philosophy as the Android JNI guard).
#[cfg(target_os = "ios")]
fn ios_videotoolbox_timeout(expected_duration: Option<f64>) -> Duration {
    let estimated = expected_duration.unwrap_or(0.0) * 4.0 + 300.0;
    Duration::from_secs_f64(estimated.clamp(1800.0, 4.0 * 3600.0))
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

/// Run a `std::process::Command` with a wall-clock timeout.
///
/// The command runs on a worker thread; if it does not finish within
/// `timeout` the process is killed and `None` is returned. This mirrors
/// the previous `tokio::time::timeout` semantics for external tools.
fn command_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Option<std::io::Result<std::process::Output>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let child = Arc::new(Mutex::new(Some(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?,
    )));
    let child_for_thread = child.clone();
    let handle = std::thread::Builder::new()
        .name("ferrisload-command".into())
        .spawn(move || {
            let child = child_for_thread.lock().unwrap().take();
            match child {
                Some(child) => {
                    let _ = tx.send(child.wait_with_output());
                }
                None => {
                    let _ = tx.send(Err(std::io::Error::other(
                        "child was taken by the timeout path",
                    )));
                }
            }
        })
        .ok()?;
    match rx.recv_timeout(timeout) {
        Ok(output) => {
            let _ = handle.join();
            Some(output)
        }
        Err(_) => {
            // Timeout: kill the child if it still exists, then wait for
            // the worker to finish draining pipes.
            if let Ok(mut guard) = child.lock() {
                if let Some(child) = guard.as_mut() {
                    let _ = child.kill();
                }
            }
            let _ = handle.join();
            None
        }
    }
}

/// A fake non-zero exit status used to signal "timed out" to callers that
/// only inspect `output.status.success()`.
const FFMPEG_TIMEOUT_EXIT: i32 = 124;

/// Cached synthetic non-successful `ExitStatus`. `ExitStatus::from_raw` is
/// unstable, so we obtain a genuine non-zero status by running one
/// guaranteed-to-fail child the first time a timeout occurs, then reuse it.
fn failed_exit_status() -> std::process::ExitStatus {
    static FAILED: std::sync::OnceLock<std::process::ExitStatus> = std::sync::OnceLock::new();
    *FAILED.get_or_init(|| {
        std::process::Command::new("ferrisload-ffmpeg-timeout-sentinel")
            .status()
            .ok()
            .filter(|status| !status.success())
            .unwrap_or_else(|| {
                // Extremely unlikely fallback: a command that always fails.
                let mut child = std::process::Command::new("cmd")
                    .arg("/c")
                    .arg("exit 1")
                    .spawn()
                    .expect("failed to spawn timeout status probe");
                child
                    .wait()
                    .expect("failed to wait for timeout status probe")
            })
    })
}

/// Run `ffmpeg`/`ffprobe` with a wall-clock timeout so a hung hardware
/// encoder or a wedged driver can never stall a download forever.
///
/// On timeout the process is killed and a synthetic `Output` with a
/// non-zero status (and a descriptive `stderr`) is returned, so callers
/// that check `status.success()` treat it exactly like a failed run —
/// including falling back to the CPU encoder.
fn run_ffmpeg(
    ffmpeg_path: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<std::process::Output> {
    let mut command = Command::new(ffmpeg_path);
    command.args(args);
    match command_with_timeout(&mut command, timeout) {
        Some(result) => result.context("FFmpeg process failed"),
        None => Ok(std::process::Output {
            status: failed_exit_status(),
            stdout: Vec::new(),
            stderr: format!(
                "FFmpeg timed out after {} seconds (encoder may be hung)",
                timeout.as_secs()
            )
            .into_bytes(),
        }),
    }
}

/// Derive a sane conversion/merge timeout from the input media duration.
///
/// - Hardware-accelerated encode is typically faster than realtime, so a
///   hung GPU encoder is aborted after a short cap (15 minutes).
/// - CPU (software) encode can legitimately take many times the source
///   length, so it gets a generous cap (6 hours) with a 30 minute floor.
///   Unknown durations fall back to those floors.
fn ffmpeg_transcode_timeout(input_duration: Option<f64>, hardware: bool) -> Duration {
    let estimated = input_duration.unwrap_or(0.0) * 3.0 + 60.0;
    let seconds = if hardware {
        estimated.clamp(120.0, 15.0 * 60.0)
    } else {
        estimated.clamp(1800.0, 6.0 * 3600.0)
    };
    Duration::from_secs_f64(seconds)
}

fn external_command_works(spec: &ExternalCommandSpec) -> bool {
    command_with_timeout(spec.command().arg("--version"), Duration::from_secs(8))
        .and_then(|result| result.ok())
        .is_some_and(|output| output.status.success())
}

fn ffmpeg_command_works(path: &Path) -> bool {
    command_with_timeout(Command::new(path).arg("-version"), Duration::from_secs(8))
        .and_then(|result| result.ok())
        .is_some_and(|output| output.status.success())
}

fn resolve_ffmpeg_path() -> Option<PathBuf> {
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

    candidates
        .into_iter()
        .find(|candidate| ffmpeg_command_works(candidate))
}

fn resolve_ytdlp_command() -> Option<ExternalCommandSpec> {
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

    candidates.into_iter().find(external_command_works)
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
fn provision_ytdlp_command(reporter: &ProgressReporter) -> Result<ExternalCommandSpec> {
    let asset_name = official_ytdlp_asset_name()?;
    let target_path = ytdlp_cache_path()?;
    let target_directory = target_path
        .parent()
        .ok_or_else(|| anyhow!("yt-dlp cache path has no parent directory"))?;
    std::fs::create_dir_all(target_directory).with_context(|| {
        format!(
            "Failed to create yt-dlp cache directory: {}",
            target_directory.display()
        )
    })?;

    emit_progress(reporter, "Preparing verified yt-dlp engine...", 0.003);
    let client = SyncHttpClient::with_timeouts(Duration::from_secs(30), Duration::from_secs(300))?;
    let release_base = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
    let checksums_url = format!("{}/SHA2-256SUMS", release_base);
    let (checksums_status, _, checksums_body) = client.get(&checksums_url, &[])?;
    if !(200..300).contains(&checksums_status) {
        bail!(
            "Failed to download yt-dlp checksums: HTTP {}",
            checksums_status
        );
    }
    let checksums = String::from_utf8_lossy(&checksums_body).into_owned();
    let expected_hash = checksum_for_release_asset(&checksums, asset_name).ok_or_else(|| {
        anyhow!(
            "Official yt-dlp checksum list did not contain {}",
            asset_name
        )
    })?;

    let asset_url = format!("{}/{}", release_base, asset_name);
    let (asset_status, asset_headers, asset_body) = client.get(&asset_url, &[])?;
    if !(200..300).contains(&asset_status) {
        bail!("Failed to download yt-dlp binary: HTTP {}", asset_status);
    }
    let total = asset_headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok());
    let temporary_path =
        target_directory.join(format!(".yt-dlp-download-{}", Uuid::new_v4().simple()));
    let mut output = std::fs::File::create(&temporary_path)?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    let mut last_reported = 0u64;
    for chunk in asset_body.chunks(64 * 1024) {
        output.write_all(chunk)?;
        hasher.update(chunk);
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
    output.flush()?;
    drop(output);

    let actual_hash = hex::encode(hasher.finalize());
    if actual_hash != expected_hash {
        let _ = std::fs::remove_file(&temporary_path);
        bail!(
            "yt-dlp SHA-256 verification failed (expected {}, received {})",
            expected_hash,
            actual_hash
        );
    }

    if target_path.exists() {
        let _ = std::fs::remove_file(&target_path);
    }
    std::fs::rename(&temporary_path, &target_path).with_context(|| {
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
    if !external_command_works(&command) {
        bail!("Verified yt-dlp engine was installed but could not be executed");
    }
    emit_progress(reporter, "Verified yt-dlp engine is ready", 0.02);
    Ok(command)
}

#[derive(Clone)]
pub struct ProgressUpdate {
    pub message: String,
    pub progress: f64,
    pub error: Option<String>,
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
        error: None,
    });
}

/// Report a terminal failure through the progress stream.
///
/// Must be called while the `StreamSink` backing `reporter` is still alive;
/// the Dart side treats a non-empty `error` as the task ending in failure.
pub(crate) fn emit_terminal_error(reporter: &ProgressReporter, error: &anyhow::Error) {
    reporter(ProgressUpdate {
        message: String::new(),
        progress: 0.0,
        error: Some(format!("{error:#}")),
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

    fn with_media_transcoder<T, F>(&self, timeout: Duration, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut JNIEnv, JClass) -> Result<T> + Send + 'static,
    {
        let jvm = self.jvm.clone();
        let gate = ANDROID_TRANSCODE_GATE
            .get_or_init(|| Arc::new(StdMutex::new(())))
            .clone();
        let (tx, rx) = std::sync::mpsc::channel::<Result<T>>();
        std::thread::Builder::new()
            .name("ferrisload-jni".into())
            .spawn(move || {
                let result = (|| {
                    let _guard = gate
                        .lock()
                        .map_err(|_| anyhow!("Android transcode gate acquire failed"))?;
                    let mut env = jvm
                        .attach_current_thread()
                        .map_err(|e| anyhow!("JNI attach thread failed: {}", e))?;
                    let class = media_transcoder_class(&mut env)?;
                    operation(&mut env, class)
                })();
                let _ = tx.send(result);
            })
            .map_err(|e| anyhow!("JNI thread spawn failed: {}", e))?;
        // A wedged MediaCodec/MediaMuxer native call must never hang the
        // download forever: bound the whole JNI operation with a hard cap.
        // (Kotlin already has its own per-step stall detection; this is the
        // last-resort guard on the Rust side.)
        rx.recv_timeout(timeout).map_err(|_| {
            anyhow!(
                "Android media transcoder timed out after {}s",
                timeout.as_secs()
            )
        })?
    }

    /// Derive a hard wall-clock cap for a single JNI media operation from the
    /// expected media duration (falling back to a 30-minute floor). Android
    /// hardware transcode is typically faster than realtime, so this is a
    /// generous ceiling that only fires when the pipeline is genuinely stuck.
    fn jni_operation_timeout(expected_duration: Option<f64>) -> Duration {
        let estimated = expected_duration.unwrap_or(0.0) * 4.0 + 300.0;
        Duration::from_secs_f64(estimated.clamp(1800.0, 4.0 * 3600.0))
    }

    pub fn transcode(
        &self,
        input_ts: &str,
        output_mp4: &str,
        video_bitrate: u32,
        audio_bitrate: u32,
        expected_duration: Option<f64>,
    ) -> Result<()> {
        if audio_bitrate > 0 {
            bail!(
                "Android MediaCodec does not currently support audio bitrate conversion; use 0 to preserve AAC audio"
            );
        }
        let input_ts = input_ts.to_string();
        let output_mp4 = output_mp4.to_string();
        let expected_duration_ms = (expected_duration.unwrap_or(0.0) * 1000.0).round() as i64;
        let timeout = Self::jni_operation_timeout(expected_duration);

        self.with_media_transcoder(timeout, move |env, class| {
            let input_ts_jstring = jni_string!(env, &input_ts, "input_ts");
            let output_mp4_jstring = jni_string!(env, &output_mp4, "output_mp4");

            let result = env
                .call_static_method(
                    class,
                    "transcode",
                    "(Ljava/lang/String;Ljava/lang/String;IIJ)Z",
                    &[
                        JValue::Object(&input_ts_jstring),
                        JValue::Object(&output_mp4_jstring),
                        JValue::Int(video_bitrate as i32),
                        JValue::Int(audio_bitrate as i32),
                        JValue::Long(expected_duration_ms),
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
    }

    pub fn mux(
        &self,
        video_path: &str,
        audio_path: &str,
        output_mp4: &str,
        expected_duration: Option<f64>,
    ) -> Result<()> {
        let video_path = video_path.to_string();
        let audio_path = audio_path.to_string();
        let output_mp4 = output_mp4.to_string();
        let expected_duration_ms = (expected_duration.unwrap_or(0.0) * 1000.0).round() as i64;
        let timeout = Self::jni_operation_timeout(expected_duration);

        self.with_media_transcoder(timeout, move |env, class| {
            let video_jstring = jni_string!(env, &video_path, "video_path");
            let audio_jstring = jni_string!(env, &audio_path, "audio_path");
            let output_jstring = jni_string!(env, &output_mp4, "output_mp4");

            let result = env
                .call_static_method(
                    class,
                    "mux",
                    "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;J)Z",
                    &[
                        JValue::Object(&video_jstring),
                        JValue::Object(&audio_jstring),
                        JValue::Object(&output_jstring),
                        JValue::Long(expected_duration_ms),
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
    }

    /// Segment-based transcode: feed every `{prefix}_{i:05}.part` file in
    /// `segment_dir` into a single decode→encode→mux pipeline with a
    /// continuous, re-based PTS timeline (the equivalent of feeding ffmpeg's
    /// concat demuxer). This is the root-cause fix for HLS streams whose
    /// naively concatenated TS resets PTS at every segment boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn transcode_segments(
        &self,
        segment_dir: &str,
        prefix: &str,
        total_segments: usize,
        output_mp4: &str,
        video_bitrate: u32,
        audio_bitrate: u32,
        expected_duration: Option<f64>,
    ) -> Result<()> {
        if audio_bitrate > 0 {
            bail!(
                "Android MediaCodec does not currently support audio bitrate conversion; use 0 to preserve AAC audio"
            );
        }
        let segment_dir = segment_dir.to_string();
        let prefix = prefix.to_string();
        let total_segments = u32::try_from(total_segments)
            .map_err(|_| anyhow!("too many HLS segments for the Android transcoder"))?;
        let output_mp4 = output_mp4.to_string();
        let expected_duration_ms = (expected_duration.unwrap_or(0.0) * 1000.0).round() as i64;
        let timeout = Self::jni_operation_timeout(expected_duration);

        self.with_media_transcoder(timeout, move |env, class| {
            let dir_jstring = jni_string!(env, &segment_dir, "segment_dir");
            let prefix_jstring = jni_string!(env, &prefix, "prefix");
            let output_jstring = jni_string!(env, &output_mp4, "output_mp4");

            let result = env
                .call_static_method(
                    class,
                    "transcodeDir",
                    "(Ljava/lang/String;Ljava/lang/String;ILjava/lang/String;IIJ)Z",
                    &[
                        JValue::Object(&dir_jstring),
                        JValue::Object(&prefix_jstring),
                        JValue::Int(total_segments as i32),
                        JValue::Object(&output_jstring),
                        JValue::Int(video_bitrate as i32),
                        JValue::Int(audio_bitrate as i32),
                        JValue::Long(expected_duration_ms),
                    ],
                )
                .map_err(|e| anyhow!("JNI call_static_method transcodeDir failed: {}", e))?;

            let success = result
                .z()
                .map_err(|e| anyhow!("JNI get boolean return failed: {}", e))?;

            if success {
                Ok(())
            } else {
                Err(anyhow!("Java MediaCodec segment transcode failed"))
            }
        })
    }

    /// Segment-based mux of a video stream and a separate audio stream (HLS
    /// audio renditions). Each segment is read by its own extractor and the
    /// PTS timeline is re-based continuously across segments.
    #[allow(clippy::too_many_arguments)]
    pub fn mux_segments(
        &self,
        video_dir: &str,
        video_prefix: &str,
        video_total: usize,
        audio_dir: &str,
        audio_prefix: &str,
        audio_total: usize,
        output_mp4: &str,
        expected_duration: Option<f64>,
    ) -> Result<()> {
        let video_dir = video_dir.to_string();
        let video_prefix = video_prefix.to_string();
        let video_total = u32::try_from(video_total)
            .map_err(|_| anyhow!("too many HLS video segments for the Android transcoder"))?;
        let audio_dir = audio_dir.to_string();
        let audio_prefix = audio_prefix.to_string();
        let audio_total = u32::try_from(audio_total)
            .map_err(|_| anyhow!("too many HLS audio segments for the Android transcoder"))?;
        let output_mp4 = output_mp4.to_string();
        let expected_duration_ms = (expected_duration.unwrap_or(0.0) * 1000.0).round() as i64;
        let timeout = Self::jni_operation_timeout(expected_duration);

        self.with_media_transcoder(timeout, move |env, class| {
            let video_dir_jstring = jni_string!(env, &video_dir, "video_dir");
            let video_prefix_jstring = jni_string!(env, &video_prefix, "video_prefix");
            let audio_dir_jstring = jni_string!(env, &audio_dir, "audio_dir");
            let audio_prefix_jstring = jni_string!(env, &audio_prefix, "audio_prefix");
            let output_jstring = jni_string!(env, &output_mp4, "output_mp4");

            let result = env
                .call_static_method(
                    class,
                    "muxDirs",
                    "(Ljava/lang/String;Ljava/lang/String;ILjava/lang/String;Ljava/lang/String;ILjava/lang/String;J)Z",
                    &[
                        JValue::Object(&video_dir_jstring),
                        JValue::Object(&video_prefix_jstring),
                        JValue::Int(video_total as i32),
                        JValue::Object(&audio_dir_jstring),
                        JValue::Object(&audio_prefix_jstring),
                        JValue::Int(audio_total as i32),
                        JValue::Object(&output_jstring),
                        JValue::Long(expected_duration_ms),
                    ],
                )
                .map_err(|e| anyhow!("JNI call_static_method muxDirs failed: {}", e))?;

            let success = result
                .z()
                .map_err(|e| anyhow!("JNI get boolean return failed: {}", e))?;

            if success {
                Ok(())
            } else {
                Err(anyhow!("Java MediaMuxer segment mux failed"))
            }
        })
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

#[allow(clippy::too_many_arguments)]
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
    let reporter = sink_progress_reporter(sink);
    let worker = reporter.clone();
    let outcome = flutter_rust_bridge::spawn_blocking_with(
        move || {
            hls2mp4_core(
                worker,
                url,
                concurrency,
                output,
                retries,
                video_bitrate,
                audio_bitrate,
                keep_temp,
            )
        },
        (),
    )
    .await
    .map_err(|e| anyhow!("hls2mp4 background task failed: {e}"));
    deliver_pipeline_outcome(&reporter, outcome)
}

/// Collapse the FRB task outcome into the stream: forward any pipeline error
/// as a terminal event so the Dart side can display the real reason, then
/// always succeed (the stream itself carries the outcome).
fn deliver_pipeline_outcome(
    reporter: &ProgressReporter,
    outcome: Result<Result<()>>,
) -> Result<()> {
    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            emit_terminal_error(reporter, &error);
            Ok(())
        }
        Err(error) => {
            emit_terminal_error(reporter, &error);
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn hls2mp4_core(
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
        &reporter,
        &url,
        &request_context,
        concurrency,
        &output,
        retries,
        video_bitrate,
        audio_bitrate,
        keep_temp,
    )
}

#[flutter_rust_bridge::frb()]
pub async fn inspect_media_from_url(url: String) -> Result<MediaInspectionResult> {
    flutter_rust_bridge::spawn_blocking_with(
        move || inspect_media_with_context_sync(url, RequestContext::default()),
        (),
    )
    .await
    .map_err(|e| anyhow!("inspect background task failed: {e}"))?
}

#[flutter_rust_bridge::frb()]
pub async fn inspect_media_with_context(
    url: String,
    request_context: RequestContext,
) -> Result<MediaInspectionResult> {
    flutter_rust_bridge::spawn_blocking_with(
        move || inspect_media_with_context_sync(url, request_context),
        (),
    )
    .await
    .map_err(|e| anyhow!("inspect background task failed: {e}"))?
}

pub(crate) fn inspect_media_with_context_sync(
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
    let headers = request_headers(&requested_page_url, &request_context)?;
    let (status, _response_headers, body) = client.get(&url, &headers)?;
    let page_url = requested_page_url;
    let html = String::from_utf8_lossy(&body).into_owned();
    let mut warnings = Vec::new();
    if let Some(reason) = detect_access_challenge(status, &html) {
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
    if !(200..300).contains(&status) {
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
        ) {
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
        ) {
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

    let candidates = score_candidates(collector.finish(), &request_context);

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

fn augment_bilibili_candidates_with_playurl(
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
    let headers = request_headers(page_url, request_context)?;
    let (status, _, body) = client.get(api_url.as_str(), &headers)?;
    if !(200..300).contains(&status) {
        bail!("Bilibili playurl API returned HTTP {}", status);
    }
    let payload: Value =
        nextjson::from_slice(&body).context("Failed to parse Bilibili playurl API response")?;
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
    let body_text = String::from_utf8_lossy(&body).into_owned();
    let synthetic_page = format!("window.__playinfo__={}", body_text);
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
fn augment_youtube_candidates_with_ytdlp(
    page_url: &str,
    request_context: &RequestContext,
    collector: &mut CandidateCollector,
) -> Result<bool> {
    let command_spec = resolve_ytdlp_command()
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
    if !request_context.origin.trim().is_empty()
        && is_safe_ytdlp_header_value(request_context.origin.trim())
    {
        command.args([
            "--add-header",
            &format!("Origin:{}", request_context.origin.trim()),
        ]);
    }
    if !request_context.cookie.trim().is_empty()
        && is_safe_ytdlp_header_value(request_context.cookie.trim())
    {
        command.args([
            "--add-header",
            &format!("Cookie:{}", request_context.cookie.trim()),
        ]);
    }
    for header in &request_context.headers {
        let name = header.name.trim();
        let value = header.value.trim();
        if !name.is_empty()
            && !value.is_empty()
            && is_safe_ytdlp_header_name(name)
            && is_safe_ytdlp_header_value(value)
        {
            command.args(["--add-header", &format!("{}:{}", name, value)]);
        }
    }
    command.arg(page_url);

    let output = command_with_timeout(&mut command, Duration::from_secs(60))
        .ok_or_else(|| anyhow!("yt-dlp inspection timed out"))?
        .context("Failed to start yt-dlp")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    let metadata: Value =
        nextjson::from_slice(&output.stdout).context("Failed to parse yt-dlp metadata")?;
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

/// Rejects header names that could smuggle extra headers into yt-dlp's
/// `--add-header` option (e.g. a `:` inside the name, or control characters).
fn is_safe_ytdlp_header_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(':')
        && name.chars().all(|character| character.is_ascii_graphic())
}

/// Rejects header values that could smuggle extra headers into yt-dlp's
/// `--add-header` value (CR/LF/control bytes). Spaces and non-ASCII are
/// legal per RFC 7230 and preserved.
fn is_safe_ytdlp_header_value(value: &str) -> bool {
    value.bytes().all(is_valid_header_value_byte)
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

#[allow(clippy::too_many_arguments)]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn run_ytdlp_site_pipeline(
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

    let ffmpeg_path = resolve_ffmpeg_path();
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
    if !request_context.origin.trim().is_empty()
        && is_safe_ytdlp_header_value(request_context.origin.trim())
    {
        command.args([
            "--add-headers",
            &format!("Origin:{}", request_context.origin.trim()),
        ]);
    }
    if !request_context.cookie.trim().is_empty()
        && is_safe_ytdlp_header_value(request_context.cookie.trim())
    {
        command.args([
            "--add-headers",
            &format!("Cookie:{}", request_context.cookie.trim()),
        ]);
    }
    for entry in &request_context.headers {
        let name = entry.name.trim();
        let value = entry.value.trim();
        if !name.is_empty()
            && !value.is_empty()
            && is_safe_ytdlp_header_name(name)
            && is_safe_ytdlp_header_value(value)
        {
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

    // Drain stderr on a background thread so the child never blocks on
    // a full pipe while we stream stdout.
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel::<String>();
    std::thread::Builder::new()
        .name("ytdlp-stderr".into())
        .spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            let _ = stderr_tx.send(text);
        })
        .context("Failed to spawn yt-dlp stderr reader")?;

    let lines = BufReader::new(stdout).lines();
    let mut reported_output = None;
    // Throttle yt-dlp progress events (yt-dlp emits multiple lines per
    // second) so the UI only sees ~1% steps instead of a flood of messages.
    let mut last_reported_coarse = i64::MIN;
    // A wedged yt-dlp (stalled network / hung extractor) must not stall the
    // download forever: read stdout on a worker thread and abort when the
    // main loop hears nothing for a long time.
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel::<std::io::Result<String>>();
    std::thread::Builder::new()
        .name("ytdlp-stdout".into())
        .spawn(move || {
            for line in lines {
                if stdout_tx.send(line).is_err() {
                    break;
                }
            }
        })
        .context("Failed to spawn yt-dlp stdout reader")?;
    loop {
        let line = match stdout_rx.recv_timeout(Duration::from_secs(300)) {
            Ok(line) => line.context("Failed to read yt-dlp stdout")?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                bail!("yt-dlp stalled for 5 minutes without progress; aborting");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if let Some((message, progress)) = parse_ytdlp_progress(line.trim()) {
            let coarse = (progress * 100.0) as i64;
            if coarse - last_reported_coarse >= 1 || progress >= 0.999 {
                last_reported_coarse = coarse;
                emit_progress(&reporter, message, progress);
            }
        } else if let Some(path) = line.trim().strip_prefix("FERRISLOAD_OUTPUT:") {
            reported_output = Some(PathBuf::from(path.trim()));
        }
    }

    let status = child.wait().context("Failed to wait for yt-dlp")?;
    let stderr = stderr_rx
        .recv()
        .context("yt-dlp stderr reader terminated unexpectedly")?;
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
        )?;
        cleanup_temp_files(keep_temp, [&downloaded_path]);
    } else if downloaded_path != output_path
        && std::fs::rename(&downloaded_path, &output_path).is_err()
    {
        std::fs::copy(&downloaded_path, &output_path).with_context(|| {
            format!("Failed to move yt-dlp output to {}", output_path.display())
        })?;
        cleanup_temp_files(keep_temp, [&downloaded_path]);
    }

    if !keep_temp {
        let _ = std::fs::remove_dir(&temp_dir);
    }
    ensure_output_file_ready(&output_path)?;
    emit_progress(&reporter, "All tasks completed", 1.0);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
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
    let reporter = sink_progress_reporter(sink);
    let worker = reporter.clone();
    let outcome = flutter_rust_bridge::spawn_blocking_with(
        move || {
            download_media_with_context_core(
                worker,
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
        },
        (),
    )
    .await
    .map_err(|e| anyhow!("download background task failed: {e}"));
    deliver_pipeline_outcome(&reporter, outcome)
}

#[allow(clippy::too_many_arguments)]
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
    let reporter = sink_progress_reporter(sink);
    let worker = reporter.clone();
    let outcome = flutter_rust_bridge::spawn_blocking_with(
        move || {
            download_media_with_context_core(
                worker,
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
        },
        (),
    )
    .await
    .map_err(|e| anyhow!("download background task failed: {e}"));
    deliver_pipeline_outcome(&reporter, outcome)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn download_media_with_context_core(
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

    let plan = DownloadPlan {
        version: DOWNLOAD_PLAN_VERSION,
        page_url: &page_url,
        media_url: &media_url,
        audio_url: audio_url.as_deref(),
        output_path: &output,
        concurrency: u16::try_from(concurrency)
            .map_err(|_| anyhow!("concurrency must be a positive 16-bit value"))?,
        retries: u8::try_from(retries)
            .map_err(|_| anyhow!("retries must be a non-negative 8-bit value"))?,
        video_bitrate_kbps: u32::try_from(video_bitrate)
            .map_err(|_| anyhow!("video bitrate must not be negative"))?,
        audio_bitrate_kbps: u32::try_from(audio_bitrate)
            .map_err(|_| anyhow!("audio bitrate must not be negative"))?,
        keep_temporary_files: keep_temp,
    };
    plan.validate()
        .map_err(|error| anyhow!("invalid download plan: {error}"))?;

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
        let should_use_ytdlp =
            extractor == "youtube" || (extractor == "bilibili" && auto_inspect && check_ffmpeg());
        if should_use_ytdlp {
            let command = if let Some(command) = resolve_ytdlp_command() {
                Some(command)
            } else {
                match provision_ytdlp_command(&reporter) {
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
                );
            }
        }
    }

    if auto_inspect {
        emit_progress(&reporter, "Resolving page media candidate...", 0.01);

        let inspection =
            inspect_media_with_context_sync(page_url.clone(), request_context.clone())?;
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

        return download_media_with_context_core(
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
        );
    }

    if is_hls_like(&media_url) {
        return run_hls_pipeline(
            &reporter,
            &media_url,
            &request_context,
            concurrency,
            &output,
            retries,
            video_bitrate,
            audio_bitrate,
            keep_temp,
        );
    }

    if protocol_from_url(&media_url) == "dash" {
        return run_dash_pipeline(
            &reporter,
            &media_url,
            &request_context,
            &output,
            retries,
            video_bitrate,
            audio_bitrate,
            keep_temp,
        );
    }

    emit_progress(&reporter, "Preparing direct media download...", 0.02);

    let page_url = Url::parse(&page_url).ok();
    let client =
        create_http_client_for_context(page_url.as_ref().map(Url::as_str), &request_context)?;
    let (output_path, temp_dir) = prepare_output_path(&output)?;
    // Build the authenticated headers once and reuse them for every
    // stream so Referer/Origin/Cookie stay consistent across retries.
    let headers = match page_url.as_ref() {
        Some(parsed) => request_headers(parsed, &request_context)?,
        None => {
            let fallback = Url::parse("https://example.com/")
                .context("failed to build fallback URL for headers")?;
            request_headers(&fallback, &request_context)?
        }
    };

    match audio_url {
        Some(audio) => {
            let video_temp = temp_dir.join("stream_video_input.bin");
            let audio_temp = temp_dir.join("stream_audio_input.bin");

            download_with_retries(
                &client,
                &media_url,
                &headers,
                &video_temp,
                retries.max(1) as u8,
                0.04,
                0.44,
                &reporter,
                "Downloading video stream",
            )?;
            download_with_retries(
                &client,
                &audio,
                &headers,
                &audio_temp,
                retries.max(1) as u8,
                0.46,
                0.80,
                &reporter,
                "Downloading audio stream",
            )?;

            merge_media_streams(
                &video_temp,
                &audio_temp,
                None,
                None,
                &output,
                video_bitrate.max(0) as u32,
                audio_bitrate.max(0) as u32,
                reporter.clone(),
                None,
            )?;

            cleanup_temp_files(keep_temp, [&video_temp, &audio_temp]);
        }
        None => {
            let extension = container_from_url(&media_url);
            if extension == "mp4" && video_bitrate <= 0 && audio_bitrate <= 0 {
                download_with_retries(
                    &client,
                    &media_url,
                    &headers,
                    &output_path,
                    retries.max(1) as u8,
                    0.04,
                    0.92,
                    &reporter,
                    "Downloading media",
                )?;
            } else {
                let temp_input = temp_dir.join(format!("direct_input.{}", extension));
                download_with_retries(
                    &client,
                    &media_url,
                    &headers,
                    &temp_input,
                    retries.max(1) as u8,
                    0.04,
                    0.72,
                    &reporter,
                    "Downloading media",
                )?;
                transcode_input_to_output(
                    &temp_input,
                    &output,
                    video_bitrate.max(0) as u32,
                    audio_bitrate.max(0) as u32,
                    reporter.clone(),
                )?;
                cleanup_temp_files(keep_temp, [&temp_input]);
            }
        }
    }

    let _ = std::fs::remove_dir(&temp_dir);
    ensure_output_file_ready(&output_path)?;
    emit_progress(&reporter, "All tasks completed", 1.0);

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_dash_pipeline(
    reporter: &ProgressReporter,
    manifest_url: &str,
    request_context: &RequestContext,
    output: &str,
    retries: i32,
    video_bitrate: i32,
    audio_bitrate: i32,
    keep_temp: bool,
) -> Result<()> {
    emit_progress(reporter, "Resolving DASH manifest...", 0.02);

    let plan = resolve_dash_download_plan(manifest_url, request_context)?;
    let (output_path, temp_dir) = prepare_output_path(output)?;

    let client = create_http_client_for_context(Some(&plan.video_url), request_context)?;
    let retries = retries.max(1) as u8;
    let manifest_base = Url::parse(manifest_url).context("Invalid DASH manifest URL")?;
    let headers = request_headers(&manifest_base, request_context)?;

    let video_extension = container_from_url(&plan.video_url);
    let video_temp = temp_dir.join(format!("dash_video_input.{}", video_extension));
    download_with_retries(
        &client,
        &plan.video_url,
        &headers,
        &video_temp,
        retries,
        0.04,
        0.46,
        reporter,
        "Downloading DASH video",
    )?;

    match plan.audio_url {
        Some(audio_url) => {
            let audio_extension = container_from_url(&audio_url);
            let audio_temp = temp_dir.join(format!("dash_audio_input.{}", audio_extension));

            download_with_retries(
                &client,
                &audio_url,
                &headers,
                &audio_temp,
                retries,
                0.48,
                0.78,
                reporter,
                "Downloading DASH audio",
            )?;

            merge_media_streams(
                &video_temp,
                &audio_temp,
                None,
                None,
                output,
                video_bitrate.max(0) as u32,
                audio_bitrate.max(0) as u32,
                reporter.clone(),
                None,
            )?;

            cleanup_temp_files(keep_temp, [&video_temp, &audio_temp]);
        }
        None => {
            if video_extension == "mp4" && video_bitrate <= 0 && audio_bitrate <= 0 {
                if std::fs::rename(&video_temp, &output_path).is_err() {
                    std::fs::copy(&video_temp, &output_path).with_context(|| {
                        format!("Failed to copy DASH output to {}", output_path.display())
                    })?;
                }
                cleanup_temp_files(keep_temp, [&video_temp]);
            } else {
                transcode_input_to_output(
                    &video_temp,
                    output,
                    video_bitrate.max(0) as u32,
                    audio_bitrate.max(0) as u32,
                    reporter.clone(),
                )?;
                cleanup_temp_files(keep_temp, [&video_temp]);
            }
        }
    }

    let _ = std::fs::remove_dir(&temp_dir);
    ensure_output_file_ready(&output_path)?;
    emit_progress(reporter, "All tasks completed", 1.0);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
// The `video_segments`/`audio_segments` plumbing is consumed only on Android
// (the segment-based transcoder); on desktop these bindings are assigned and
// passed as `None`, so the compiler's `unused_assignments`/`unused_mut` lints
// would otherwise fire for the non-Android target.
#[allow(unused_assignments, unused_mut)]
pub(crate) fn run_hls_pipeline(
    reporter: &ProgressReporter,
    url: &str,
    request_context: &RequestContext,
    concurrency: i32,
    output: &str,
    retries: i32,
    video_bitrate: i32,
    audio_bitrate: i32,
    keep_temp: bool,
) -> Result<()> {
    emit_progress(reporter, "Initializing...", 0.0);

    init_runtime_logging();

    // The Android MediaMuxer/MediaCodec backends fail silently (or leave no
    // file) when the output's parent directory does not exist — e.g. when the
    // external files dir has not been materialised yet. Create it up-front so
    // a missing directory can never cause a spurious "no output file".
    if let Some(parent) = Path::new(output).parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create output directory: {}", parent.display())
            })?;
        }
    }

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

    emit_progress(reporter, "Selecting transcoder backend...", 0.01);

    let backend = select_transcoder_backend()?;
    let requires_reencode = video_bitrate > 0 || audio_bitrate > 0;
    match backend {
        TranscoderKind::Ffmpeg(accel) if requires_reencode => {
            check_pb.finish_with_message(format!("Selected FFmpeg backend ({})", accel.label()));
            emit_progress(
                reporter,
                format!("Selected transcoder backend: {}", accel.label()),
                0.015,
            );
        }
        TranscoderKind::Ffmpeg(_) => {
            check_pb.finish_with_message("Selected FFmpeg stream-copy backend");
            emit_progress(
                reporter,
                "Selected FFmpeg stream copy (no re-encoding)",
                0.015,
            );
        }
        TranscoderKind::AndroidHardware if requires_reencode => {
            check_pb.finish_with_message("Selected Android MediaCodec backend");
            emit_progress(
                reporter,
                "Selected hardware encoder: Android MediaCodec",
                0.015,
            );
        }
        TranscoderKind::AndroidHardware => {
            check_pb.finish_with_message("Selected Android MediaMuxer pipeline");
            emit_progress(
                reporter,
                "Selected Android MediaMuxer (MediaCodec fallback if remux fails)",
                0.015,
            );
        }
        TranscoderKind::IosVideoToolbox if requires_reencode => {
            check_pb.finish_with_message("Selected iOS VideoToolbox hardware encoder");
            emit_progress(
                reporter,
                "Selected hardware encoder: Apple VideoToolbox (A/M-series)",
                0.015,
            );
        }
        TranscoderKind::IosVideoToolbox => {
            check_pb.finish_with_message("Selected iOS AVFoundation remux");
            emit_progress(
                reporter,
                "Selected iOS AVFoundation (hardware H.264 if re-encoding)",
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

    emit_progress(reporter, "Downloading M3U8 playlist...", 0.02);

    let (m3u8_content, effective_playlist_url) = download_playlist_with_url(url, request_context)?;
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

    let mut external_audio_plan: Option<(MediaPlaylist, Url, String)> = None;
    // Per-stream segment metadata handed to the Android segment-based
    // transcoder (see `SegmentInput`). Filled in below from the playlists;
    // ignored on desktop where ffmpeg consumes the merged TS / concat list.
    let mut video_segments: Option<SegmentInput> = None;
    let mut audio_segments: Option<SegmentInput> = None;
    // Sum of the media playlist's EXTINF durations. Used after conversion to
    // detect silent truncation (e.g. only the first few seconds surviving),
    // which previously wasted all the download traffic on a broken output.
    // Both match arms below assign this before use.
    let expected_duration: Option<f64>;

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
                download_playlist_with_url(media_url.as_str(), request_context)?;
            let (_, media_pl) = parse_playlist(&media_content)
                .map_err(|e| anyhow!("Failed to parse m3u8: {:?}", e))?;

            let Playlist::MediaPlaylist(media_playlist) = media_pl else {
                bail!("Master playlist's referenced playlist is not a media playlist");
            };
            let video_total = media_playlist.segments.len();
            expected_duration = Some(
                media_playlist
                    .segments
                    .iter()
                    .map(|segment| segment.duration as f64)
                    .sum(),
            );

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
                        download_playlist_with_url(audio_url.as_str(), request_context)?;
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
                "v",
                &multi_progress,
                video_reporter,
                request_context.clone(),
            )?;
            video_segments = Some(SegmentInput {
                dir: temp_dir.clone(),
                prefix: "v".to_string(),
                total: video_total,
            });
        }
        Playlist::MediaPlaylist(mp) => {
            info!("Media Playlist found, {} segments", mp.segments.len());
            let video_total = mp.segments.len();
            expected_duration = Some(
                mp.segments
                    .iter()
                    .map(|segment| segment.duration as f64)
                    .sum(),
            );
            download_and_merge(
                mp,
                base_url,
                concurrency,
                retries,
                &temp_primary_str,
                &temp_dir,
                "v",
                &multi_progress,
                reporter.clone(),
                request_context.clone(),
            )?;
            video_segments = Some(SegmentInput {
                dir: temp_dir.clone(),
                prefix: "v".to_string(),
                total: video_total,
            });
        }
    }

    if let Some((audio_playlist, audio_base_url, rendition_name)) = external_audio_plan {
        let audio_total = audio_playlist.segments.len();
        emit_progress(
            reporter,
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
            "a",
            &multi_progress,
            staged_progress_reporter(reporter.clone(), "Audio", 0.48, 0.92),
            request_context.clone(),
        )?;
        audio_segments = Some(SegmentInput {
            dir: temp_dir.clone(),
            prefix: "a".to_string(),
            total: audio_total,
        });
        merge_media_streams(
            &temp_primary,
            &temp_audio,
            video_segments,
            audio_segments,
            output,
            video_bitrate,
            audio_bitrate,
            reporter.clone(),
            expected_duration,
        )?;
    } else {
        convert_to_mp4(
            &temp_primary_str,
            video_segments,
            output,
            video_bitrate,
            audio_bitrate,
            &multi_progress,
            backend,
            reporter.clone(),
            expected_duration,
        )?;
    }

    if !keep_temp {
        // remove_dir_all also cleans the per-segment .part files and the
        // ffmpeg concat lists that live inside the temp directory.
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    ensure_output_file_ready(Path::new(output))?;
    emit_progress(reporter, "All tasks completed", 1.0);

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
        // Forward terminal error events verbatim (do not stage their progress).
        let (message, progress) = if update.error.is_some() {
            (update.message, update.progress)
        } else {
            (
                format!("{}: {}", stage, update.message),
                start + update.progress.clamp(0.0, 1.0) * span,
            )
        };
        parent(ProgressUpdate {
            message,
            progress,
            error: update.error,
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

    #[allow(clippy::too_many_arguments)]
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
            id: Uuid::new_v4().to_string(),
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
            parsed.path_segments().and_then(|mut segments| {
                segments
                    .rfind(|segment| !segment.is_empty())
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
        Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&temp_dir).with_context(|| {
        format!(
            "Failed to create temporary directory: {}",
            temp_dir.display()
        )
    })?;
    Ok(temp_dir)
}

fn cleanup_temp_files<'a, I>(keep_temp: bool, paths: I)
where
    I: IntoIterator<Item = &'a PathBuf>,
{
    if keep_temp {
        return;
    }

    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

fn transcode_input_to_output(
    input_path: &Path,
    output: &str,
    video_bitrate: u32,
    audio_bitrate: u32,
    reporter: ProgressReporter,
) -> Result<()> {
    convert_to_mp4(
        input_path.to_string_lossy().as_ref(),
        None,
        output,
        video_bitrate,
        audio_bitrate,
        &MultiProgress::new(),
        select_transcoder_backend()?,
        reporter,
        None,
    )
}

fn has_mp4_signature(prefix: &[u8]) -> bool {
    prefix
        .windows(4)
        .take(24)
        .any(|window| window == b"ftyp" || window == b"styp")
}

fn ensure_output_file_ready(path: &Path) -> Result<()> {
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
        let mut file = std::fs::File::open(path)?;
        let mut prefix = [0u8; 64];
        let bytes_read = file.read(&mut prefix)?;
        if !has_mp4_signature(&prefix[..bytes_read]) {
            bail!(
                "Output does not contain an MP4 file signature: {}",
                path.display()
            );
        }
    }

    let ffprobe = if let Some(ffmpeg) = resolve_ffmpeg_path() {
        let executable_name = if cfg!(target_os = "windows") {
            "ffprobe.exe"
        } else {
            "ffprobe"
        };
        ffmpeg.with_file_name(executable_name)
    } else {
        PathBuf::from("ffprobe")
    };
    let ffprobe_available =
        run_ffmpeg(&ffprobe, &["-version".to_string()], Duration::from_secs(20))
            .map(|output| output.status.success())
            .unwrap_or(false);
    if ffprobe_available {
        let probe = run_ffmpeg(
            &ffprobe,
            &[
                "-v".to_string(),
                "error".to_string(),
                "-select_streams".to_string(),
                "v:0".to_string(),
                "-show_entries".to_string(),
                "stream=codec_type".to_string(),
                "-of".to_string(),
                "default=noprint_wrappers=1:nokey=1".to_string(),
                path.to_string_lossy().to_string(),
            ],
            Duration::from_secs(20),
        )
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

/// Stream a large payload to `path` using bounded Range chunks.
///
/// The `courierust` engine materializes each request body in memory, so
/// large media must be pulled in chunks instead of buffering the whole
/// file. A server that ignores Range (returns `200` with the full body)
/// is handled by writing the single received body.
#[allow(clippy::too_many_arguments)]
fn stream_media_response_to_file(
    client: &SyncHttpClient,
    url: &str,
    headers: &[(String, String)],
    path: &Path,
    progress_start: f64,
    progress_end: f64,
    reporter: &ProgressReporter,
    label: &str,
) -> Result<()> {
    const CHUNK: u64 = 8 * 1024 * 1024;

    let mut output = std::fs::File::create(path)
        .with_context(|| format!("Failed to create media file: {}", path.display()))?;
    let mut downloaded = 0u64;
    let mut last_reported = 0u64;
    emit_progress(reporter, label, progress_start);

    // First request asks for a bounded first chunk (Range). A server that
    // honors Range answers `206` with a partial body; one that ignores it
    // answers `200` with the whole body. Both cases are handled below.
    let (first_status, first_headers, first_body) = client.get_range(url, headers, 0, CHUNK - 1)?;
    if !(200..300).contains(&first_status) {
        bail!("Media request returned HTTP {}", first_status);
    }
    // Reject HTML/JSON responses that indicate a captcha or error page.
    if let Some(content_type) = first_headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.to_ascii_lowercase())
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

    let total = first_headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok());

    let write_and_report = |output: &mut std::fs::File,
                            data: &[u8],
                            downloaded: &mut u64,
                            last_reported: &mut u64|
     -> Result<()> {
        output.write_all(data)?;
        *downloaded = downloaded.saturating_add(data.len() as u64);
        if downloaded.saturating_sub(*last_reported) >= 512 * 1024 {
            let progress = total
                .filter(|total| *total > 0)
                .map(|total| {
                    progress_start
                        + (*downloaded as f64 / total as f64).clamp(0.0, 1.0)
                            * (progress_end - progress_start)
                })
                .unwrap_or(progress_start);
            let detail = total
                .map(|total| format!("{} / {}", human_bytes(*downloaded), human_bytes(total)))
                .unwrap_or_else(|| human_bytes(*downloaded));
            emit_progress(reporter, format!("{} [{}]", label, detail), progress);
            *last_reported = *downloaded;
        }
        Ok(())
    };

    if first_status == 206 {
        // Server honors Range: write the first chunk, then pull the rest.
        if !first_body.is_empty() {
            write_and_report(
                &mut output,
                &first_body,
                &mut downloaded,
                &mut last_reported,
            )?;
        }
        let total = total.unwrap_or(0);
        let mut offset = first_body.len() as u64;
        while total == 0 || offset < total {
            let end = if total > 0 {
                (offset + CHUNK - 1).min(total - 1)
            } else {
                offset + CHUNK - 1
            };
            let (status, _, body) = client.get_range(url, headers, offset, end)?;
            if !(200..300).contains(&status) {
                bail!("Media Range request returned HTTP {}", status);
            }
            if body.is_empty() {
                break;
            }
            write_and_report(&mut output, &body, &mut downloaded, &mut last_reported)?;
            offset = offset.saturating_add(body.len() as u64);
            // Stop when we have reached the declared length, or when the
            // server returned a short chunk with no declared length.
            if total > 0 && offset >= total {
                break;
            }
            if total == 0 && (body.len() as u64) < CHUNK {
                break;
            }
        }
    } else {
        // Server ignored Range and returned the whole body in one shot.
        write_and_report(
            &mut output,
            &first_body,
            &mut downloaded,
            &mut last_reported,
        )?;
    }

    output.flush()?;
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

#[allow(clippy::too_many_arguments)]
fn download_with_retries(
    client: &SyncHttpClient,
    url: &str,
    headers: &[(String, String)],
    path: &Path,
    retries: u8,
    progress_start: f64,
    progress_end: f64,
    reporter: &ProgressReporter,
    label: &str,
) -> Result<()> {
    for attempt in 1..=retries {
        let _ = std::fs::remove_file(path);
        let result = stream_media_response_to_file(
            client,
            url,
            headers,
            path,
            progress_start,
            progress_end,
            reporter,
            label,
        );
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt == retries {
                    return Err(error)
                        .with_context(|| format!("Failed to download media: {}", url));
                }
                warn!(
                    "Media stream attempt {} failed for {}: {}",
                    attempt, url, error
                );
                let delay = retry_backoff_delay(attempt, None);
                std::thread::sleep(delay);
            }
        }
    }

    bail!("Failed to download media after retries: {}", url)
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

fn resolve_dash_download_plan(
    manifest_url: &str,
    request_context: &RequestContext,
) -> Result<DashDownloadPlan> {
    let (manifest_bytes, effective_manifest_url) =
        download_playlist_with_url(manifest_url, request_context)?;
    let manifest = String::from_utf8(manifest_bytes).context("DASH manifest is not valid UTF-8")?;
    resolve_dash_download_plan_from_manifest(effective_manifest_url.as_str(), &manifest)
}

/// Parse a DASH MPD manifest (XML) and select the best video/audio
/// representations, resolving relative BaseURLs against the manifest.
fn resolve_dash_download_plan_from_manifest(
    manifest_url: &str,
    manifest: &str,
) -> Result<DashDownloadPlan> {
    let root = xml::parse_xml(manifest.as_bytes()).context("Failed to parse DASH manifest")?;
    if root.name != "MPD" {
        bail!("DASH manifest root element is not <MPD>");
    }
    let manifest_base = Url::parse(manifest_url).context("Invalid DASH manifest URL")?;

    let mpd_base_urls = base_urls_of(&root);
    let mut best_video: Option<DashRepresentationCandidate> = None;
    let mut best_audio: Option<DashRepresentationCandidate> = None;

    for period in root.children_named("Period") {
        for adaptation in period.children_named("AdaptationSet") {
            let content_type = adaptation
                .attr("contentType")
                .or_else(|| adaptation.attr("mimeType"))
                .unwrap_or_default()
                .to_ascii_lowercase();
            let adaptation_base_urls = base_urls_of(adaptation);

            for representation in adaptation.children_named("Representation") {
                let representation_type = representation
                    .attr("mimeType")
                    .unwrap_or(&content_type)
                    .to_ascii_lowercase();
                let Some(url) = resolve_dash_representation_url(
                    &manifest_base,
                    &mpd_base_urls,
                    &adaptation_base_urls,
                    &base_urls_of(representation),
                ) else {
                    continue;
                };

                let candidate = DashRepresentationCandidate {
                    url,
                    bandwidth: representation
                        .attr("bandwidth")
                        .and_then(|value| value.parse::<i64>().ok())
                        .unwrap_or_default(),
                    width: representation
                        .attr("width")
                        .and_then(|value| value.parse::<i32>().ok())
                        .unwrap_or_default(),
                    height: representation
                        .attr("height")
                        .and_then(|value| value.parse::<i32>().ok())
                        .unwrap_or_default(),
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

/// Collect the text of every direct `<BaseURL>` child of `element`.
fn base_urls_of(element: &Element) -> Vec<String> {
    element
        .children_named("BaseURL")
        .map(|base| base.text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect()
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

    let joined = current.join(leaf).ok()?;
    // Enforce an http/https-only allow-list so a malicious MPD cannot
    // redirect the client to file://, ftp://, data:, etc.
    if !matches!(joined.scheme(), "http" | "https") {
        return None;
    }
    Some(joined.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        bilibili_playurl_api_url, canonical_site_context, checksum_for_release_asset,
        has_mp4_signature, hls_response_bytes, is_valid_header_value_byte, normalize_source_url,
        parse_ytdlp_progress, playlist_base_url, resolve_dash_download_plan_from_manifest,
        resolve_hls_byte_range, select_best_hls_variant, select_hls_audio_rendition,
        should_auto_inspect_download_target, youtube_itag_from_media_url, ByteRange, Playlist,
    };
    use crate::hls::parse_playlist;
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
    fn accepts_space_and_obs_text_in_header_values_but_rejects_crlf() {
        // A real-world User-Agent contains spaces and must validate.
        assert!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"
                .bytes()
                .all(is_valid_header_value_byte)
        );
        assert!("Mozilla/5.0 (中文设备) Chrome/120"
            .bytes()
            .all(is_valid_header_value_byte));
        assert!(!b"\r".iter().copied().all(is_valid_header_value_byte));
        assert!(!b"\n".iter().copied().all(is_valid_header_value_byte));
        assert!(!b"\x00".iter().copied().all(is_valid_header_value_byte));
        assert!(!b"\x7f".iter().copied().all(is_valid_header_value_byte));
        assert!(!"text/html\r\nX-Evil: 1"
            .bytes()
            .all(is_valid_header_value_byte));
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
            hls_response_bytes(200, &[0, 1, 2, 3, 4, 5], Some((2, 4)))
                .expect("full response should be sliced"),
            vec![2, 3, 4]
        );
        assert_eq!(
            hls_response_bytes(206, &[7, 8, 9], Some((100, 102)),)
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

#[allow(clippy::too_many_arguments)]
fn merge_media_streams(
    video_path: &Path,
    audio_path: &Path,
    video_segments: Option<SegmentInput>,
    audio_segments: Option<SegmentInput>,
    output_path: &str,
    video_bitrate: u32,
    audio_bitrate: u32,
    reporter: ProgressReporter,
    expected_duration: Option<f64>,
) -> Result<()> {
    let requires_reencode = video_bitrate > 0 || audio_bitrate > 0;

    #[cfg(target_os = "android")]
    {
        if let Some(transcoder) = ANDROID_HW_TRANSCODER.get() {
            // Prefer the segment-based muxer (per-segment extractors with a
            // continuous PTS timeline — concat-demuxer equivalent) and only
            // fall back to the naively-concatenated TS files on failure.
            let mux_with_fallback = |target: &str| -> Result<()> {
                match (&video_segments, &audio_segments) {
                    (Some(video), Some(audio)) => {
                        match transcoder.mux_segments(
                            video.dir.to_string_lossy().as_ref(),
                            &video.prefix,
                            video.total,
                            audio.dir.to_string_lossy().as_ref(),
                            &audio.prefix,
                            audio.total,
                            target,
                            expected_duration,
                        ) {
                            Ok(()) => return Ok(()),
                            Err(error) => warn!(
                                "Android segment-based mux failed ({}); falling back to merged streams",
                                error
                            ),
                        }
                    }
                    _ => {}
                }
                transcoder.mux(
                    video_path.to_string_lossy().as_ref(),
                    audio_path.to_string_lossy().as_ref(),
                    target,
                    expected_duration,
                )
            };

            if !requires_reencode {
                emit_progress(
                    &reporter,
                    "Merging streams with Android MediaMuxer (no re-encoding)",
                    0.9,
                );
                match mux_with_fallback(output_path) {
                    Ok(_) => return Ok(()),
                    Err(e) => warn!(
                        "Android MediaMuxer merge failed, falling back if possible: {}",
                        e
                    ),
                }
            } else {
                let mux_input = video_path.with_file_name(format!(
                    "merged_transcode_input_{}.mp4",
                    Uuid::new_v4().simple()
                ));
                emit_progress(
                    &reporter,
                    "Preparing separate streams with Android MediaMuxer",
                    0.88,
                );
                let mux_result = mux_with_fallback(mux_input.to_string_lossy().as_ref());

                if let Err(error) = mux_result {
                    warn!(
                        "Android MediaMuxer preparation failed, falling back if possible: {}",
                        error
                    );
                } else {
                    emit_progress(&reporter, "Using Android MediaCodec hardware encoder", 0.93);
                    let transcode_result = transcoder.transcode(
                        mux_input.to_string_lossy().as_ref(),
                        output_path,
                        video_bitrate,
                        audio_bitrate,
                        expected_duration,
                    );
                    let _ = std::fs::remove_file(&mux_input);
                    match transcode_result {
                        Ok(()) => return Ok(()),
                        Err(error) => {
                            warn!(
                                "Android MediaCodec hardware encode failed, falling back if possible: {}",
                                error
                            );
                            let _ = std::fs::remove_file(output_path);
                        }
                    }
                }
                let _ = std::fs::remove_file(&mux_input);
            }
        }
    }

    // iOS has no FFmpeg and no software encoder: merge + (re-)encode with the
    // native AVFoundation / VideoToolbox hardware pipeline. The muxer itself
    // validates the output duration against the expected value.
    #[cfg(target_os = "ios")]
    {
        emit_progress(
            &reporter,
            "Merging streams with Apple VideoToolbox hardware encoder",
            0.92,
        );
        let timeout = ios_videotoolbox_timeout(expected_duration);
        ios_videotoolbox::mux(
            video_path.to_string_lossy().as_ref(),
            audio_path.to_string_lossy().as_ref(),
            output_path,
            expected_duration,
            timeout,
        )?;
        info!("iOS VideoToolbox mux complete: {}", output_path);
        return Ok(());
    }

    // iOS always returns above; the FFmpeg path below serves Android/desktop.
    #[cfg_attr(target_os = "ios", allow(unreachable_code))]
    let ffmpeg_path = resolve_ffmpeg_path()
        .ok_or_else(|| anyhow!("FFmpeg is required to merge separated audio and video streams"))?;

    // Prefer concat demuxer segment lists (written by download_and_merge_once)
    // over the naively-concatenated TS files, for the same discontinuity reasons
    // described in convert_to_mp4.
    let video_concat = concat_list_for_input(video_path);
    let audio_concat = concat_list_for_input(audio_path);

    let mut selected_accel = if requires_reencode {
        detect_acceleration(&ffmpeg_path)?
    } else {
        AccelType::Cpu
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
        video_concat.as_deref(),
        audio_path,
        audio_concat.as_deref(),
        output_path,
        video_bitrate,
        audio_bitrate,
        selected_accel,
        &ffmpeg_path,
    )?;

    if !output.status.success() && requires_reencode && selected_accel != AccelType::Cpu {
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
        selected_accel = AccelType::Cpu;
        let _ = std::fs::remove_file(output_path);
        output = run_ffmpeg_merge(
            video_path,
            video_concat.as_deref(),
            audio_path,
            audio_concat.as_deref(),
            output_path,
            video_bitrate,
            audio_bitrate,
            selected_accel,
            &ffmpeg_path,
        )?;
    }

    if !output.status.success() {
        bail!(
            "FFmpeg merge failed with {}: {}",
            selected_accel.label(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    validate_output_duration(
        Path::new(output_path),
        expected_duration,
        &ffmpeg_path,
        "FFmpeg merge",
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_ffmpeg_merge(
    video_path: &Path,
    video_concat: Option<&Path>,
    audio_path: &Path,
    audio_concat: Option<&Path>,
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
        // Generate missing PTS and ignore DTS errors so independently-encoded
        // HLS segments that were naively concatenated (PTS restarts at zero at
        // every segment boundary) are re-stamped into a continuous timeline.
        // `+genpts` alone only fills absent timestamps; `+igndts` additionally
        // stops ffmpeg from dropping frames when the DTS goes backwards, which
        // was a second root cause of "only the first few seconds survive".
        "-fflags".to_string(),
        "+genpts+igndts".to_string(),
    ];
    if let Some(concat) = video_concat {
        args.extend([
            "-f".to_string(),
            "concat".to_string(),
            "-safe".to_string(),
            "0".to_string(),
            "-i".to_string(),
            concat.to_string_lossy().to_string(),
        ]);
    } else {
        args.extend(["-i".to_string(), video_path.to_string_lossy().to_string()]);
    }
    if let Some(concat) = audio_concat {
        args.extend([
            "-f".to_string(),
            "concat".to_string(),
            "-safe".to_string(),
            "0".to_string(),
            "-i".to_string(),
            concat.to_string_lossy().to_string(),
        ]);
    } else {
        args.extend(["-i".to_string(), audio_path.to_string_lossy().to_string()]);
    }
    args.extend([
        "-map".to_string(),
        "0:v:0".to_string(),
        "-map".to_string(),
        "1:a:0".to_string(),
    ]);

    if !requires_reencode {
        args.extend(["-c".to_string(), "copy".to_string()]);
    } else {
        args.extend([
            "-c:v".to_string(),
            accel.encoder().to_string(),
            "-c:a".to_string(),
            "aac".to_string(),
        ]);
        push_accel_encode_args(&mut args, accel);
        if video_bitrate > 0 {
            args.push("-b:v".to_string());
            args.push(format!("{}k", video_bitrate));
        }
        if audio_bitrate > 0 {
            args.push("-b:a".to_string());
            args.push(format!("{}k", audio_bitrate));
        }
    }

    args.extend([
        "-avoid_negative_ts".to_string(),
        "make_zero".to_string(),
        "-muxpreload".to_string(),
        "0".to_string(),
        "-muxdelay".to_string(),
        "0".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
    ]);
    args.push(output_path.to_string());

    // Bound the merge/encode run so a hung hardware encoder cannot stall
    // the download indefinitely; derive the timeout from the input length.
    let input_duration = probe_media_duration(video_path, ffmpeg_path);
    let hardware = requires_reencode && accel != AccelType::Cpu;
    let timeout = ffmpeg_transcode_timeout(input_duration, hardware);
    run_ffmpeg(ffmpeg_path, &args, timeout)
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

fn score_candidates(
    candidates: Vec<MediaCandidate>,
    request_context: &RequestContext,
) -> Vec<MediaCandidate> {
    let mut candidates = candidates
        .into_iter()
        .map(|candidate| score_candidate(candidate, request_context))
        .collect::<Vec<_>>();

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

fn score_candidate(
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
            inspect_hls_metadata(&candidate.media_url, request_context)
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

fn inspect_hls_metadata(url: &str, request_context: &RequestContext) -> Result<(usize, f64)> {
    let bytes = download_playlist(url, request_context)?;
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
    _source_url: Option<&str>,
    _request_context: &RequestContext,
) -> Result<SyncHttpClient> {
    SyncHttpClient::with_timeouts(Duration::from_secs(10), Duration::from_secs(45))
}

/// Build the per-request header list for a given source URL and context.
fn request_headers(
    source_url: &Url,
    request_context: &RequestContext,
) -> Result<Vec<(String, String)>> {
    let mut headers: Vec<(String, String)> = Vec::new();
    let user_agent = if request_context.user_agent.trim().is_empty() {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"
    } else {
        request_context.user_agent.trim()
    };
    validate_header_value(user_agent, "user-agent")?;
    headers.push(("user-agent".to_string(), user_agent.to_string()));
    headers.push(("accept".to_string(), "*/*".to_string()));
    headers.push((
        "accept-language".to_string(),
        "en-US,en;q=0.9,zh-CN;q=0.8".to_string(),
    ));

    if let Some(domain) = source_url.domain() {
        let (default_referer, default_origin) = canonical_site_context(domain);
        let referer = if request_context.referer.trim().is_empty() {
            default_referer
        } else {
            request_context.referer.trim().to_string()
        };
        let origin = if request_context.origin.trim().is_empty() {
            default_origin
        } else {
            request_context.origin.trim().to_string()
        };
        validate_header_value(&referer, "referer")?;
        validate_header_value(&origin, "origin")?;
        headers.push(("referer".to_string(), referer));
        headers.push(("origin".to_string(), origin));
    }

    if !request_context.cookie.trim().is_empty() {
        let cookie = request_context.cookie.trim().to_string();
        validate_header_value(&cookie, "cookie")?;
        headers.push(("cookie".to_string(), cookie));
    }

    for entry in &request_context.headers {
        let name = entry.name.trim();
        let value = entry.value.trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b':' && byte != b' ')
        {
            bail!("Invalid HTTP header name in request context: {:?}", name);
        }
        // Validate the header value: CR/LF/control bytes would enable
        if !value.bytes().all(is_valid_header_value_byte) {
            bail!(
                "Invalid HTTP header value in request context (contains CR/LF/control bytes): {:?}",
                value
            );
        }
        headers.push((name.to_ascii_lowercase(), value.to_string()));
    }

    Ok(headers)
}

/// RFC 7230 field-value byte rule: `VCHAR` (0x21-0x7E), `SP` (0x20),
/// `HTAB` (0x09) and `obs-text` (0x80-0xFF) are legal in a header value.
/// CR, LF, NUL and the remaining control bytes are rejected because they
/// enable response-splitting / header injection when forwarded upstream.
fn is_valid_header_value_byte(byte: u8) -> bool {
    matches!(byte, b'\t' | b' ' | 0x21..=0x7E | 0x80..=0xFF)
}

/// Reject header values with control characters (like CR/LF) to prevent injection attacks, while allowing spaces, tabs, and non-ASCII text.
fn validate_header_value(value: &str, header_name: &str) -> Result<()> {
    if !value.bytes().all(is_valid_header_value_byte) {
        bail!("Invalid HTTP header value for {header_name:?} (contains CR/LF/control bytes)");
    }
    Ok(())
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

fn download_playlist(url: &str, request_context: &RequestContext) -> Result<Vec<u8>> {
    let (bytes, _) = download_playlist_with_url(url, request_context)?;
    Ok(bytes)
}

fn download_playlist_with_url(
    url: &str,
    request_context: &RequestContext,
) -> Result<(Vec<u8>, Url)> {
    let client = create_http_client_for_context(Some(url), request_context)?;
    let parsed = Url::parse(url).context("Invalid playlist URL")?;
    let headers = request_headers(&parsed, request_context)?;
    let mut last_error: Option<anyhow::Error> = None;
    for attempt in 1..=3u8 {
        let result = client.get(url, &headers);
        match result {
            Ok((status, _, body)) => {
                if !(200..300).contains(&status) {
                    last_error = Some(anyhow!("Failed to download playlist: HTTP {}", status));
                    if attempt < 3 {
                        let delay = retry_backoff_delay(attempt, Some(status));
                        std::thread::sleep(delay);
                    }
                    continue;
                }
                let effective_url = Url::parse(url).context("Invalid playlist URL")?;
                return Ok((body, effective_url));
            }
            Err(error) => {
                last_error = Some(anyhow!("Failed to download playlist: {}", error));
                if attempt < 3 {
                    let delay = retry_backoff_delay(attempt, None);
                    std::thread::sleep(delay);
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("Failed to download playlist: {}", url)))
}

fn check_ffmpeg() -> bool {
    resolve_ffmpeg_path().is_some()
}

fn select_transcoder_backend() -> Result<TranscoderKind> {
    #[cfg(target_os = "android")]
    if ANDROID_HW_TRANSCODER.get().is_some() {
        return Ok(TranscoderKind::AndroidHardware);
    }

    // iOS has no embedded FFmpeg and no software H.264 encoder: the only
    // backend is the native AVFoundation / VideoToolbox hardware pipeline.
    #[cfg(target_os = "ios")]
    if ios_videotoolbox::available() {
        return Ok(TranscoderKind::IosVideoToolbox);
    }

    if let Some(ffmpeg_path) = resolve_ffmpeg_path() {
        let accel = detect_acceleration(&ffmpeg_path)?;
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

#[allow(clippy::too_many_arguments)]
/// Identifies a set of HLS segment files on disk that the Android
/// segment-based transcoder consumes one-by-one (the equivalent of feeding
/// ffmpeg's concat demuxer). On non-Android platforms this is ignored.
#[derive(Clone, Debug)]
struct SegmentInput {
    dir: PathBuf,
    prefix: String,
    total: usize,
}

#[allow(clippy::too_many_arguments)]
fn download_and_merge(
    playlist: MediaPlaylist,
    base_url: Option<Url>,
    concurrency: usize,
    retries: u8,
    output_file: &str,
    temp_dir: &Path,
    segment_prefix: &str,
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
            segment_prefix,
            multi_progress,
            reporter.clone(),
            request_context.clone(),
        ) {
            Ok(()) => return Ok(()),
            Err(error) => {
                warn!(
                    "Multi-thread segment download failed with concurrency {}. Retrying single-threaded: {}",
                    concurrency,
                    error
                );
                cleanup_segment_temp_files(temp_dir, output_file);
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
        segment_prefix,
        multi_progress,
        reporter,
        request_context,
    )
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

fn hls_response_bytes(status: u16, data: &[u8], byte_range: Option<(u64, u64)>) -> Result<Vec<u8>> {
    let Some((start, end)) = byte_range else {
        return Ok(data.to_vec());
    };
    let expected_length = end
        .checked_sub(start)
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| anyhow!("Invalid HLS byte range {}-{}", start, end))?;
    let expected_length = usize::try_from(expected_length)
        .context("HLS byte range is too large for this platform")?;

    if status == 206 {
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

fn download_hls_resource(
    client: &SyncHttpClient,
    request: &HlsResourceRequest,
    headers: &[(String, String)],
    retries: u8,
) -> Result<Vec<u8>> {
    for attempt in 1..=retries {
        let result = match request.byte_range {
            Some((start, end)) => client
                .get_range(&request.url, headers, start, end)
                .map(|(status, _, body)| (status, body)),
            None => client
                .get(&request.url, headers)
                .map(|(status, _, body)| (status, body)),
        };

        match result {
            Ok((status, bytes)) if (200..300).contains(&status) => {
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
            Ok((status, _)) => {
                warn!(
                    "Attempt {} failed: {} HTTP {}",
                    attempt, request.url, status
                );
                if attempt < retries {
                    let delay = retry_backoff_delay(attempt, Some(status));
                    std::thread::sleep(delay);
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
            std::thread::sleep(delay);
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
    aes_128_cbc_decrypt(key, iv, &data).map_err(|e| anyhow!("AES-128-CBC decryption failed: {e}"))
}

#[allow(clippy::too_many_arguments)]
fn download_and_merge_once(
    playlist: MediaPlaylist,
    base_url: Option<Url>,
    concurrency: usize,
    retries: u8,
    output_file: &str,
    temp_dir: &Path,
    segment_prefix: &str,
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

    let segment_headers = match &base_url {
        Some(base) => request_headers(base, &request_context)?,
        None => Vec::new(),
    };
    let segment_headers_shared = Arc::new(segment_headers);

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
        if !matches!(key_url.scheme(), "http" | "https") {
            bail!("Refusing non-HTTP(S) HLS key URL: {}", key_url.scheme());
        }
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
                &segment_headers_shared,
                retries,
            )?;
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
        let resolved = if let Some(base) = &base_url {
            base.join(uri)?
        } else {
            Url::parse(uri)?
        };
        if !matches!(resolved.scheme(), "http" | "https") {
            bail!(
                "Refusing non-HTTP(S) media resource URL: {}",
                resolved.scheme()
            );
        }
        Ok(resolved.to_string())
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

    let client = Arc::new(create_http_client_for_context(
        base_url.as_ref().map(Url::as_str),
        &request_context,
    )?);
    let completed = Arc::new(std::sync::atomic::AtomicU64::new(0));

    let temp_dir = temp_dir.to_path_buf();
    let next_index = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker_count = concurrency.min(total).max(1);
    let errors: Arc<Mutex<Option<anyhow::Error>>> = Arc::new(Mutex::new(None));
    let last_reported_pct = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let client_shared = client.clone();
    let completed_shared = completed.clone();
    let pb_shared = download_pb.clone();
    let reporter_shared = reporter.clone();
    let temp_dir_shared = temp_dir.clone();
    let next_shared = next_index.clone();
    let errors_shared = errors.clone();
    let last_reported_shared = last_reported_pct.clone();
    let segment_headers_shared = segment_headers_shared.clone();
    let segment_requests_shared = segment_requests;
    let init_requests_shared = init_requests;
    let segment_crypto_shared = segment_crypto;

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            let next = next_shared.clone();
            let errors = errors_shared.clone();
            let client = client_shared.clone();
            let completed = completed_shared.clone();
            let pb = pb_shared.clone();
            let reporter = reporter_shared.clone();
            let temp_dir = temp_dir_shared.clone();
            let last_reported = last_reported_shared.clone();
            let headers = segment_headers_shared.clone();
            let segment_requests = &segment_requests_shared;
            let init_requests = &init_requests_shared;
            let segment_crypto = &segment_crypto_shared;
            scope.spawn(move || loop {
                // Stop early if a previous worker failed.
                if errors.lock().unwrap().is_some() {
                    break;
                }
                let idx = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if idx >= total {
                    break;
                }

                let result = (|| -> Result<()> {
                    let segment_request = segment_requests[idx].clone();
                    let init_request = init_requests[idx].clone();
                    let key = segment_crypto.get(idx).cloned().flatten();

                    let mut buffer = Vec::new();
                    if let Some(init_request) = init_request {
                        let init_data =
                            download_hls_resource(&client, &init_request, &headers, retries)?;
                        let init_data = decrypt_hls_resource(init_data, key.as_ref())?;
                        buffer.extend_from_slice(&init_data);
                    }
                    let segment_data =
                        download_hls_resource(&client, &segment_request, &headers, retries)?;
                    let segment_data = decrypt_hls_resource(segment_data, key.as_ref())?;
                    buffer.extend_from_slice(&segment_data);

                    let file_name = format!("{}_{:05}.part", segment_prefix, idx);
                    let tmp_path = temp_dir.join(file_name);
                    std::fs::write(&tmp_path, &buffer).with_context(|| {
                        format!(
                            "Failed to write segment: {} (url: {})",
                            tmp_path.display(),
                            segment_request.url
                        )
                    })?;

                    let count = completed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    pb.set_position(count);

                    let total_u64 = total as u64;
                    let coarse = ((count as f64) / (total as f64) * 900.0) as u64;
                    let is_final = count >= total_u64;
                    let should_emit = is_final
                        || coarse.saturating_sub(
                            last_reported.load(std::sync::atomic::Ordering::Relaxed),
                        ) >= 9;
                    if should_emit {
                        last_reported.store(coarse, std::sync::atomic::Ordering::Relaxed);
                        pb.set_message(format!("Downloading segments [{}/{}]", count, total));
                        emit_progress(
                            &reporter,
                            format!("Downloading segments [{}/{}]", count, total),
                            (count as f64) / (total as f64) * 0.9,
                        );
                    }
                    Ok(())
                })();

                if let Err(error) = result {
                    let mut slot = errors.lock().unwrap();
                    if slot.is_none() {
                        *slot = Some(error);
                    }
                }
            });
        }
    });

    if let Some(error) = errors.lock().unwrap().take() {
        return Err(error);
    }

    download_pb.finish_with_message("All segments downloaded");

    let is_fmp4 = segments.iter().any(|segment| segment.map.is_some());
    let use_concat = !is_fmp4 && cfg!(not(target_os = "android"));
    // Keep the per-segment `.part` files when a later stage needs them:
    // ffmpeg's concat demuxer (desktop) and the Android segment-based
    // transcoder both consume the individual segment files.
    let keep_segments = use_concat || cfg!(target_os = "android");
    let concat_list_path = PathBuf::from(format!("{}.concat.txt", output_file));
    if use_concat {
        let mut list_content = String::new();
        for i in 0..total {
            let segment_path = temp_dir.join(format!("{}_{:05}.part", segment_prefix, i));
            let absolute = std::path::absolute(&segment_path).unwrap_or(segment_path);
            let normalized = absolute.to_string_lossy().replace('\\', "/");
            let escaped = normalized.replace('\'', "'\\''");
            list_content.push_str(&format!("file '{}'\n", escaped));
        }
        std::fs::write(&concat_list_path, list_content).with_context(|| {
            format!(
                "Failed to write concat list for ffmpeg: {}",
                concat_list_path.display()
            )
        })?;
        info!(
            "Wrote ffmpeg concat list with {} segments: {}",
            total,
            concat_list_path.display()
        );
    }

    let merge_pb = multi_progress.add(ProgressBar::new(total as u64));
    merge_pb.set_style(
        ProgressStyle::with_template(
            "{msg} [{elapsed_precise}] {bar:40.green} {pos:>7}/{len:7} ({percent}%)",
        )?
        .progress_chars("##-"),
    );
    merge_pb.set_message("Merging segments");

    let mut output = std::fs::File::create(output_file)
        .with_context(|| format!("Failed to create output TS file: {}", output_file))?;

    for i in 0..total {
        let file_name = format!("{}_{:05}.part", segment_prefix, i);
        let tmp_path = temp_dir.join(&file_name);

        let mut segment = std::fs::File::open(&tmp_path)
            .with_context(|| format!("Failed to read segment: {}", tmp_path.display()))?;

        std::io::copy(&mut segment, &mut output)
            .with_context(|| format!("Failed to write to output TS: {}", output_file))?;

        if !keep_segments {
            let _ = std::fs::remove_file(&tmp_path);
        }
        merge_pb.inc(1);
        merge_pb.set_message(format!("Merging segments [{}/{}]", i + 1, total));
    }

    merge_pb.finish_with_message("Merge complete");
    Ok(())
}

fn cleanup_segment_temp_files(temp_dir: &Path, output_file: &str) {
    // Remove every per-segment `.part` file regardless of its stream prefix.
    if let Ok(entries) = std::fs::read_dir(temp_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".part"))
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    let _ = std::fs::remove_file(output_file);
    let concat_list = PathBuf::from(format!("{}.concat.txt", output_file));
    let _ = std::fs::remove_file(concat_list);
}

fn detect_acceleration(ffmpeg_path: &Path) -> Result<AccelType> {
    let output = run_ffmpeg(
        ffmpeg_path,
        &["-hide_banner".to_string(), "-encoders".to_string()],
        Duration::from_secs(30),
    )?;

    let list = String::from_utf8_lossy(&output.stdout);
    let candidates = [
        AccelType::Nvidia,
        AccelType::Amd,
        AccelType::IntelQuickSync,
        AccelType::LinuxVaapi,
        AccelType::AppleVideoToolbox,
    ];
    for candidate in candidates {
        if list.contains(candidate.encoder()) && probe_ffmpeg_encoder(ffmpeg_path, candidate) {
            return Ok(candidate);
        }
    }
    if list.contains(AccelType::Cpu.encoder()) && probe_ffmpeg_encoder(ffmpeg_path, AccelType::Cpu)
    {
        return Ok(AccelType::Cpu);
    }
    bail!("FFmpeg is installed but no usable H.264 encoder passed the runtime probe")
}

fn probe_ffmpeg_encoder(ffmpeg_path: &Path, accel: AccelType) -> bool {
    let mut args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-f".to_string(),
        "lavfi".to_string(),
        "-i".to_string(),
        "color=c=black:s=64x64:r=1".to_string(),
        "-frames:v".to_string(),
        "1".to_string(),
        "-an".to_string(),
        "-c:v".to_string(),
        accel.encoder().to_string(),
    ];
    match accel {
        AccelType::LinuxVaapi => args.extend([
            "-vaapi_device".to_string(),
            "/dev/dri/renderD128".to_string(),
            "-vf".to_string(),
            "format=nv12,hwupload".to_string(),
        ]),

        AccelType::IntelQuickSync => args.extend([
            "-init_hw_device".to_string(),
            "qsv=hw".to_string(),
            "-filter_hw_device".to_string(),
            "hw".to_string(),
            "-vf".to_string(),
            "format=nv12,hwupload=extra_hw_frames=64".to_string(),
        ]),

        AccelType::Nvidia | AccelType::Amd | AccelType::AppleVideoToolbox | AccelType::Cpu => {}
    }
    args.extend(["-f".to_string(), "null".to_string(), "-".to_string()]);

    match run_ffmpeg(ffmpeg_path, &args, Duration::from_secs(30)) {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

/// Appends the hardware-acceleration specific arguments (device init, frame
/// upload filters, encoder presets) for a real transcode run.
fn push_accel_encode_args(args: &mut Vec<String>, accel: AccelType) {
    match accel {
        AccelType::Nvidia => args.extend([
            "-preset".to_string(),
            "p4".to_string(),
            "-rc".to_string(),
            "vbr".to_string(),
        ]),
        AccelType::Cpu => args.extend(["-preset".to_string(), "medium".to_string()]),
        AccelType::LinuxVaapi => args.extend([
            "-vaapi_device".to_string(),
            "/dev/dri/renderD128".to_string(),
            "-vf".to_string(),
            "format=nv12,hwupload".to_string(),
        ]),
        AccelType::IntelQuickSync => args.extend([
            "-init_hw_device".to_string(),
            "qsv=hw".to_string(),
            "-filter_hw_device".to_string(),
            "hw".to_string(),
            "-vf".to_string(),
            "format=nv12,hwupload=extra_hw_frames=64".to_string(),
        ]),
        // AMF and VideoToolbox accept software frames and upload internally.
        AccelType::Amd | AccelType::AppleVideoToolbox => {}
    }
}

/// Returns the concat-demuxer list file that `download_and_merge_once` writes
/// next to the merged stream (e.g. `temp_primary.ts.concat.txt`), when present.
///
/// Using the concat demuxer with the original segment files is dramatically more
/// robust than transcoding a single naively-concatenated TS: ffmpeg re-stamps
/// each segment and no longer stops at the first non-monotonic timestamp, which
/// was the root cause of outputs containing only the first few seconds.
fn concat_list_for_input(input_path: &Path) -> Option<PathBuf> {
    let list = PathBuf::from(format!("{}.concat.txt", input_path.to_string_lossy()));
    if list.is_file() {
        Some(list)
    } else {
        None
    }
}

fn parse_ffmpeg_duration(stderr: &str) -> Option<f64> {
    let pattern = Regex::new(r"Duration:\s*(\d+):(\d+):(\d+(?:\.\d+)?)").ok()?;
    let captures = pattern.captures(stderr)?;
    let hours: f64 = captures.get(1)?.as_str().parse().ok()?;
    let minutes: f64 = captures.get(2)?.as_str().parse().ok()?;
    let seconds: f64 = captures.get(3)?.as_str().parse().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

fn probe_media_duration(path: &Path, ffmpeg_path: &Path) -> Option<f64> {
    let executable_name = if cfg!(target_os = "windows") {
        "ffprobe.exe"
    } else {
        "ffprobe"
    };
    let ffprobe_path = ffmpeg_path.with_file_name(executable_name);
    let probe_args = [
        "-v".to_string(),
        "error".to_string(),
        "-show_entries".to_string(),
        "format=duration".to_string(),
        "-of".to_string(),
        "default=noprint_wrappers=1:nokey=1".to_string(),
        path.to_string_lossy().to_string(),
    ];
    let probe = run_ffmpeg(&ffprobe_path, &probe_args, Duration::from_secs(20)).ok()?;
    if probe.status.success() {
        if let Ok(text) = String::from_utf8(probe.stdout) {
            if let Ok(seconds) = text.trim().parse::<f64>() {
                if seconds.is_finite() && seconds > 0.0 {
                    return Some(seconds);
                }
            }
        }
    }

    let inspect_args = [
        "-hide_banner".to_string(),
        "-i".to_string(),
        path.to_string_lossy().to_string(),
    ];
    let inspect = run_ffmpeg(ffmpeg_path, &inspect_args, Duration::from_secs(20)).ok()?;
    let stderr = String::from_utf8_lossy(&inspect.stderr);
    parse_ffmpeg_duration(&stderr)
}

/// Verifies that a produced output is not truncated relative to the expected
/// duration derived from the HLS playlist. Without this check a transcoder that
/// silently stops after the first few seconds (while exiting with success)
/// would hand the user a broken file after wasting all the download traffic.
fn validate_output_duration(
    path: &Path,
    expected: Option<f64>,
    ffmpeg_path: &Path,
    stage: &str,
) -> Result<()> {
    let Some(expected) = expected.filter(|value| *value > 0.0) else {
        return Ok(());
    };
    let Some(actual) = probe_media_duration(path, ffmpeg_path) else {
        warn!(
            "Could not probe duration of {} output; skipping duration validation",
            stage
        );
        return Ok(());
    };

    let tolerance = (expected * 0.85).max(expected - 5.0);
    if actual + 1.0 < tolerance {
        bail!(
            "{} output is truncated: expected about {:.1}s but produced {:.1}s",
            stage,
            expected,
            actual
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn convert_to_mp4(
    input_ts: &str,
    video_segments: Option<SegmentInput>,
    output_path: &str,
    video_bitrate: u32,
    audio_bitrate: u32,
    multi_progress: &MultiProgress,
    backend: TranscoderKind,
    reporter: ProgressReporter,
    expected_duration: Option<f64>,
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
                .ok_or_else(|| anyhow!("FFmpeg became unavailable before conversion"))?;
            let concat_list = concat_list_for_input(Path::new(input_ts));
            let used_concat = concat_list.is_some();

            let mut selected_accel = accel;
            let mut output = run_ffmpeg_conversion(
                input_ts,
                concat_list.as_deref(),
                output_path,
                video_bitrate,
                audio_bitrate,
                selected_accel,
                &ffmpeg_path,
            )?;

            if !output.status.success() && selected_accel != AccelType::Cpu && requires_reencode {
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
                let _ = std::fs::remove_file(output_path);
                selected_accel = AccelType::Cpu;
                output = run_ffmpeg_conversion(
                    input_ts,
                    concat_list.as_deref(),
                    output_path,
                    video_bitrate,
                    audio_bitrate,
                    selected_accel,
                    &ffmpeg_path,
                )?;
            }

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                convert_pb.finish_with_message("MP4 transcode failed");
                error!("FFmpeg stderr:\n{}", stderr);
                bail!("MP4 transcode failed with {}", selected_accel.label());
            }

            if let Err(error) = validate_output_duration(
                Path::new(output_path),
                expected_duration,
                &ffmpeg_path,
                "MP4 transcode",
            ) {
                if used_concat {
                    warn!("{}; retrying with the merged stream file", error);
                    let _ = std::fs::remove_file(output_path);
                    output = run_ffmpeg_conversion(
                        input_ts,
                        None,
                        output_path,
                        video_bitrate,
                        audio_bitrate,
                        selected_accel,
                        &ffmpeg_path,
                    )?;
                    if !output.status.success() {
                        bail!(
                            "MP4 transcode failed on retry with {}: {}",
                            selected_accel.label(),
                            String::from_utf8_lossy(&output.stderr).trim()
                        );
                    }
                    validate_output_duration(
                        Path::new(output_path),
                        expected_duration,
                        &ffmpeg_path,
                        "MP4 transcode",
                    )?;
                } else {
                    return Err(error);
                }
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
                video_segments,
                input_ts,
                output_path,
                video_bitrate,
                audio_bitrate,
                expected_duration,
                &convert_pb,
            )?;
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
        TranscoderKind::IosVideoToolbox => {
            emit_progress(
                &reporter,
                "Using Apple VideoToolbox hardware H.264 encoder",
                0.955,
            );
            ios_hardware_transcode(
                input_ts,
                output_path,
                video_bitrate,
                audio_bitrate,
                expected_duration,
            )?;
            convert_pb.finish_with_message("VideoToolbox transcode complete");
            info!("Output file: {}", output_path);

            let out_meta = std::fs::metadata(output_path)
                .context("Transcode output file not found after iOS VideoToolbox")?;
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

/// Transcode a single input (merged TS / direct media) to MP4 using the
/// native iOS AVFoundation / VideoToolbox hardware pipeline.
#[cfg(target_os = "ios")]
fn ios_hardware_transcode(
    input_ts: &str,
    output_mp4: &str,
    video_bitrate: u32,
    audio_bitrate: u32,
    expected_duration: Option<f64>,
) -> Result<()> {
    let timeout = ios_videotoolbox_timeout(expected_duration);
    ios_videotoolbox::transcode(
        input_ts,
        output_mp4,
        video_bitrate,
        audio_bitrate,
        expected_duration,
        timeout,
    )
}

#[cfg(not(target_os = "ios"))]
fn ios_hardware_transcode(
    _input_ts: &str,
    _output_mp4: &str,
    _video_bitrate: u32,
    _audio_bitrate: u32,
    _expected_duration: Option<f64>,
) -> Result<()> {
    bail!("Apple VideoToolbox transcoding is only available on iOS")
}

fn run_ffmpeg_conversion(
    input_path: &str,
    concat_list: Option<&Path>,
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
        "error".to_string(),
        "-fflags".to_string(),
        "+genpts+igndts".to_string(),
    ];
    if let Some(concat_list) = concat_list {
        args.extend([
            "-f".to_string(),
            "concat".to_string(),
            "-safe".to_string(),
            "0".to_string(),
            "-i".to_string(),
            concat_list.to_string_lossy().to_string(),
        ]);
    } else {
        args.extend(["-i".to_string(), input_path.to_string()]);
    }

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
        push_accel_encode_args(&mut args, accel);
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
        "-avoid_negative_ts".to_string(),
        "make_zero".to_string(),
        "-muxpreload".to_string(),
        "0".to_string(),
        "-muxdelay".to_string(),
        "0".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        output_path.to_string(),
    ]);

    let input_duration = probe_media_duration(Path::new(input_path), ffmpeg_path);
    let hardware = video_bitrate > 0 && accel != AccelType::Cpu;
    let timeout = ffmpeg_transcode_timeout(input_duration, hardware);
    run_ffmpeg(ffmpeg_path, &args, timeout)
}

/// Convert an HLS TS / segment set to MP4 on Android.
///
/// `segments` (when present) enables the segment-based pipeline: each
/// `{prefix}_{i:05}.part` file is fed to the codec independently with a
/// continuous PTS timeline — the equivalent of ffmpeg's concat demuxer, and
/// immune to the PTS resets that a naively concatenated TS exhibits. On any
/// segment-path failure we fall back to the legacy single-file path so a
/// download can never regress relative to previous behavior.
fn android_hardware_transcode(
    segments: Option<SegmentInput>,
    input_ts: &str,
    output_mp4: &str,
    video_bitrate: u32,
    audio_bitrate: u32,
    expected_duration: Option<f64>,
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

        if let Some(segments) = segments {
            match transcoder.transcode_segments(
                segments.dir.to_string_lossy().as_ref(),
                &segments.prefix,
                segments.total,
                output_mp4,
                video_bitrate,
                audio_bitrate,
                expected_duration,
            ) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    // The segment pipeline is the preferred path but never a
                    // hard requirement: degrade to the merged-TS pipeline.
                    warn!(
                        "Android segment-based transcode failed ({}); falling back to merged stream",
                        error
                    );
                }
            }
        }

        transcoder.transcode(
            input_ts,
            output_mp4,
            video_bitrate,
            audio_bitrate,
            expected_duration,
        )
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = segments;
        let _ = input_ts;
        let _ = output_mp4;
        let _ = video_bitrate;
        let _ = audio_bitrate;
        let _ = expected_duration;
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
