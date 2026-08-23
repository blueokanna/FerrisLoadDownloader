# FerrisLoad

一个用 Flutter 写的跨平台视频下载器，核心下载与转码逻辑在 Rust 里（通过 `flutter_rust_bridge` 桥接）。

主打 HLS / M3U8 下载：拉取播放列表、并发抓段、AES-128 解密、合并、转封装成 MP4 存到你指定的位置。也能直接下 MP4 / WebM / MKV / DASH 直链，并支持从普通网页、YouTube、Bilibili 页面里自动找出可用的媒体流。

> 英文版： [README.en.md](README.en.md)

## 它能做什么

- **M3U8 / HLS**：支持主/媒体播放列表、自动选择最高清晰度的变体、AES-128-CBC 解密、带退避重试的并发分片下载。并发失败会自动降级为单线程重试，而不是直接报错。
- **直链下载**：粘贴一个 `.m3u8` 或媒体直链，直接下载，无需先"分析"。粘贴 YouTube / Bilibili 页面地址也会自动解析出可下载的流。
- **页面分析**：可选地先分析网页，列出候选流并按清晰度/码率排序，再选一个下载。
- **转封装 / 转码**：桌面端用 FFmpeg（自动探测 NVENC / AMF / Intel QSV / VAAPI / VideoToolbox）；Android 端用系统 `MediaCodec` 硬件编解码，不依赖 FFmpeg。
- **多任务**：下载进行中你可以继续改地址、改文件名再开一个新任务，互不干扰。
- **授权会话**：有些站点需要登录才能下。你可以手动填入 Cookie / User-Agent / Referer / Origin / 自定义请求头，也可以打开内置浏览器登录后一键导入。程序不会绕过 Cloudflare、验证码、DRM、防盗链签名或限流——这些限制本身就不该被绕。

## 工程结构

```
lib/            Flutter 界面（Material 3，动态取色，多语言）
rust/           Rust 核心
  src/api/      下载管线（HLS/DASH/直链）、站点适配、FFmpeg、Android JNI
  src/api_server/ 可选的 HTTP API 服务（Docker）
  src/crypto/   自研 AES-128-CBC 与 SHA-256（依赖标准库）
  src/hls.rs    自研 HLS 播放列表解析器（带资源上限）
  src/net.rs    同步 HTTP 客户端（courierust + rustls 后备）
android/        Android 宿主：MediaCodec 转码器、MediaStore 导出、前台服务
```

## 快速开始

```bash
flutter pub get
flutter run
```

Rust 库由 `rust_builder`（Cargokit）在 Flutter 构建时自动编译，一般不需要手动干预。

桌面端转码需要 FFmpeg 在 `PATH` 里；Android 不需要。

### 重新生成 FRB 绑定（仅在改动 Rust API 时需要）

```bash
flutter_rust_bridge_codegen generate
```

要求 codegen 版本与 `rust/Cargo.toml` 里的 `flutter_rust_bridge` 一致（目前为 2.13.0）。

## 使用

1. 把链接粘贴到「资源地址」。
   - `.m3u8` 或媒体直链：直接点下载。
   - 网页地址：可以先「分析」，选好候选流再下载。
2. 填「输出文件」名，可选指定保存目录。
3. 「下载选项」里可调并发数、重试次数、视频/音频码率（0 = 保持源码率）。
4. 点「下载」。想先看候选就点「分析」。
5. 完成后会显示保存路径并记入历史。

需要登录的站点：打开设置里的授权浏览器，自己完成登录后导入会话，后续分析与下载都会复用这个授权上下文。

## Android 转码说明

Android 端没有 FFmpeg，转码完全靠系统 `MediaCodec`：

- **解码/编码都优先硬件**（按厂商名评分：高通、联发科、海思、三星等），没有可用硬件编码器时才回退软件编码器。
- **HLS 逐段处理**：不再把 TS 字节硬拼成一个文件（那样每段的时间戳会重置，导致 MediaCodec 只解出前几秒）。现在把每一段独立喂给解码器，跨段连续重建时间戳，等价于 FFmpeg 的 concat demuxer。
- **纯转封装优先**：码率为 0 时先尝试无损 remux（视频/音频直接搬进 MP4）；失败或时长被截断时才做硬件重编码。
- **输出自检**：转完会用 `MediaExtractor` / `MediaMetadataRetriever` 验证轨道存在、时长达标，不产出"只有前几秒"的废文件。
- 编码器按分辨率自适应默认码率、实测帧率设 `KEY_FRAME_RATE` / `KEY_OPERATING_RATE`、实时优先级、无 B 帧，尽量减少播放时的卡顿。

## iOS 转码说明（Apple A / M 系列硬件）

iOS 没有 FFmpeg、也没有软件 H.264 编码器，转码走原生 `AVFoundation` / `VideoToolbox`（`ios/Runner/VideoToolboxBridge.m`）：

- **AVAssetWriter + H.264 自动使用硬件编码器**——A 系列（iPhone/iPad）和 M 系列（Mac）芯片都会命中 VideoToolbox，这是 iOS 上"正确调用硬件"的唯一途径。
- **旋转保留**：输出带上 `preferredTransform`，竖屏素材不会被横过来。
- **音频处理**：AAC 直接 passthrough（不重编码）；非 AAC 自动转 AAC，保证 MP4 兼容。
- **时长自检**：转码/合并后对照 HLS 期望时长校验，截断立即报错而不是产出废文件。
- **超时兜底**：Rust 侧用 `ios_videotoolbox_timeout` 做墙钟上限，原生侧内部也有有界等待，硬件挂起不会卡死下载。

桌面端（Windows/macOS/Linux x86 与 Apple Silicon）仍由 FFmpeg 驱动：自动探测 NVENC / AMF / QSV / VAAPI / VideoToolbox，探测失败回退 CPU libx264。

## Web / WASM

浏览器里跑不了 Rust 引擎，所以 Web 版把下载/分析交给 **FerrisLoad API 服务**（即 Docker 镜像里的同一个服务器）：

```bash
# 本地起 API 服务（多架构：amd64 / arm64 / armv7）
docker compose --profile api up -d
```

- 构建：`flutter build web --release`（JS）或 `flutter build web --release --wasm`（WASM）。
- 网页端在「设置」里填入 API 地址（默认 `http://localhost:3000`）；非本机地址强制要求 `https`，避免会话凭据明文传输。
- 分析（`POST /inspect`）、下载（`POST /download` + 轮询 `GET /status/:id`）与原生端共用同一套引擎，界面一致。

## Docker API

容器里跑的是真正的下载器（不是模拟进度）。接口：

- `GET /health`
- `POST /inspect`（返回页面/直链的候选流列表，Web 端分析用）
- `POST /download`（可直接给 `url`，或给 `media_url` / `audio_url` 指定精确流）
- `GET /status/:task_id`
- `GET /tasks`

```bash
docker build -f Dockerfile.api -t ferrisload-api .
docker run --rm -p 3000:3000 -e DOWNLOAD_DIR=/app/downloads \
  -v $(pwd)/downloads:/app/downloads ferrisload-api
```

**API 安全（默认开启）**：

- **SSRF 防护**：API 是网络暴露面，默认拒绝会解析到内网/回环/链路本地地址的目标（云元数据端点、Docker 内网、宿主机 loopback 都挡在门外）。确有需要（如从 NAS 下载）时设 `FERRISLOAD_ALLOW_PRIVATE_NETWORKS=1`。
- **可选 Bearer 鉴权**：设 `FERRISLOAD_API_TOKEN=xxx` 后，除 `GET /health` 外的所有端点都要求 `Authorization: Bearer xxx`，防止公开端口被当成开放下载代理。Web 端在设置里填入同样的令牌即可。
- 非 root 运行、HEALTHCHECK、TLS 校验、路径穿越消毒、请求体上限等保持不变。

CI（GitHub Actions）会构建并推送 `linux/amd64`、`linux/arm64`、`linux/arm/v7` 三架构镜像到 GHCR。

## 安全与隐私

- 只允许 `http/https` 目标，网络层与解析层都有 scheme 白名单（防 SSRF / 本地文件读取）。
- 自定义请求头做了 CR/LF 注入校验；Cookie 等凭据只在内存里，不落盘。
- HLS 密钥、DASH 片段 URL 同样强制 `http/https`。
- TLS 校验默认全开；内置客户端对某些 P-384 证书链兼容性差时自动切换到 rustls 后备。
- 输出文件名做了路径穿越消毒。
- Web 端把会话凭据转给 API 服务时，非本机地址强制 HTTPS（`ApiDownloadEngine` 传输安全校验）。
- CI 每天/每次提交跑安全扫描：`cargo audit`（Rust 漏洞）、`flutter pub outdated`（Dart 依赖）、Trivy（容器漏洞 + 密钥 + 配置错误）。
- 请只下载你拥有或已获授权的资源。

## 验证

```bash
flutter analyze
flutter test
cargo test --manifest-path rust/Cargo.toml
cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets --locked -- -D warnings
```

CI 还会额外构建 Web（JS + WASM）与 iOS（arm64，未签名），并对 Rust/Dart/容器依赖做安全扫描。

## License

见 [LICENSE](LICENSE)。
