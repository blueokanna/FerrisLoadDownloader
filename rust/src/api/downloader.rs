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
use reqwest::{header, Client};
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
    let _ = sink.add(ProgressUpdate {
        message: "Initializing...".to_string(),
        progress: 0.0,
    });

    #[cfg(target_os = "android")]
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    #[cfg(not(target_os = "android"))]
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .try_init()
        .ok();

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

    let m3u8_content = download_playlist(&url).await?;
    let (_, playlist) =
        parse_playlist(&m3u8_content).map_err(|e| anyhow!("Failed to parse M3U8: {:?}", e))?;
    download_pb.finish_with_message("Parsed M3U8 playlist");

    let base_url = if url.starts_with("http") {
        let mut parsed_url = Url::parse(&url)?;
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

            let media_content = download_playlist(media_url.as_str()).await?;
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
            )
            .await?;
        }
    }

    convert_to_mp4(
        &temp_ts_str,
        &output,
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

async fn download_playlist(url: &str) -> Result<Vec<u8>> {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
        ),
    );
    headers.insert(header::ACCEPT, header::HeaderValue::from_static("*/*"));
    headers.insert(
        header::ACCEPT_LANGUAGE,
        header::HeaderValue::from_static("en-US,en;q=0.9"),
    );

    if let Ok(parsed_url) = Url::parse(url) {
        if let Some(domain) = parsed_url.domain() {
            let referer = format!("https://{}/", domain);
            headers.insert(header::REFERER, header::HeaderValue::from_str(&referer)?);
        }
    }

    let client = Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .build()?;

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
) -> Result<()> {
    // 纭繚涓存椂鐩綍瀛樺湪涓斿彲鍐�
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

    // 澶勭悊 AES-128 鍔犲瘑
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
                let client = create_http_client()?;
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
    let client = Arc::new(create_http_client()?);
    let completed = Arc::new(Mutex::new(0u64));

    // 鉁� 鍏抽敭淇锛氫紶閫� temp_dir 鍒板紓姝ヤ换鍔�
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
            let temp_dir = temp_dir.clone(); // 鉁� 鍏嬮殕鍒颁换鍔�

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

                            // 鉁� 鍏抽敭淇锛氬垎鐗囧啓鍏� temp_dir 涓�
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
                        }

                        Err(e) => {
                            pb.set_message(format!("Retrying... ({}/{})", attempt, retries));
                            warn!("Attempt {} request error: {} - {}", attempt, seg_url, e);
                        }
                    }

                    if attempt < retries {
                        tokio::time::sleep(Duration::from_millis(2000)).await;
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

            // Verify output file
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

            // Verify output file
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
            error!("鉂� CRITICAL: Android MediaCodec transcoder not registered!");
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
                info!("鉁� {}", msg);
            }
            Err(e) => {
                warn!("鈿狅笍 Transcoder check result: {}", e);
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
            eprintln!("鉂� JNI_OnLoad: Failed to create JavaVM: {:?}", e);
            return jni::sys::JNI_VERSION_1_6 as i32;
        }
    };

    match register_android_mediacodec_transcoder(jvm.clone()) {
        Ok(_) => {
            info!("鉁� Android MediaCodec transcoder registered successfully in JNI_OnLoad");
        }
        Err(e) => {
            error!("鉂� Failed to register transcoder in JNI_OnLoad: {}", e);
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