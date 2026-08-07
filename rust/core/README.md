# ferrisload-core

`ferrisload-core` contains FerrisLoad's platform-independent download-plan
types, validation rules, and bounded RustBinary wire format.

The crate is `no_std` by default and does not allocate while encoding or
decoding a `DownloadPlan`. Callers provide the output buffer, and decoded
string fields borrow directly from the input frame. Enable the `std` feature
only when standard error integration is required.

```toml
[dependencies]
ferrisload-core = { version = "0.1.2", default-features = false }
```

```rust
use ferrisload_core::{
    decode_download_plan, encode_download_plan, encoded_download_plan_size,
    DownloadPlan, DOWNLOAD_PLAN_VERSION,
};

let plan = DownloadPlan {
    version: DOWNLOAD_PLAN_VERSION,
    page_url: "https://example.com/watch/1",
    media_url: "https://cdn.example.com/master.m3u8",
    audio_url: None,
    output_path: "video.mp4",
    concurrency: 8,
    retries: 3,
    video_bitrate_kbps: 0,
    audio_bitrate_kbps: 0,
    keep_temporary_files: false,
};

let required = encoded_download_plan_size(&plan)?;
let mut frame = [0_u8; 512];
assert!(required <= frame.len());
let written = encode_download_plan(&mut frame, &plan)?;
let decoded = decode_download_plan(&frame[..written])?;
assert_eq!(decoded, plan);
# Ok::<(), ferrisload_core::WireError>(())
```

Network access, filesystems, Flutter FFI, and hardware codecs remain in the
application's platform adapters and are intentionally outside this crate.
