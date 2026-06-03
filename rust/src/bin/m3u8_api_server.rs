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

#[tokio::main]
async fn main() {
    init_runtime_logging();
    rust_lib_m3u8_downloader::api_server::run_server().await;
}
