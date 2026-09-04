//! Diagnostic: exercise SyncHttpClient against hls.piotrt.cn to surface the
//! exact transport failure (DNS / TLS / connect / HTTP).
use rust_lib_m3u8_downloader::net::SyncHttpClient;

fn main() {
    let client = match SyncHttpClient::with_timeouts(
        std::time::Duration::from_secs(10),
        std::time::Duration::from_secs(30),
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("client init failed: {e:#}");
            std::process::exit(1);
        }
    };

    let targets = [
        "https://hls.piotrt.cn/",
        // Real-world HLS master playlist (Apple CDN) proving end-to-end
        // playlist fetch through the (possibly falling-back) client.
        "https://devstreaming-cdn.apple.com/videos/streaming/examples/img_bipbop_adv_example_ts/master.m3u8",
    ];

    for url in targets {
        println!("=== GET {url}");
        match client.get(url, &[]) {
            Ok((status, headers, body)) => {
                let preview: String = String::from_utf8_lossy(&body[..body.len().min(200)])
                    .chars()
                    .map(|c| if c == '\n' { '|' } else { c })
                    .collect();
                println!(
                    "  OK status={status} len={} headers={headers:?}\n  body[:200]={preview}",
                    body.len()
                );
            }
            Err(e) => println!("  FAIL {e:#}"),
        }
    }
}
