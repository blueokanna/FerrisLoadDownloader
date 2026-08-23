# M3U8下载器 Docker部署指南

本项目提供了 Docker 化支持，主要面向可实际使用的 REST API 服务，同时保留 Linux GUI 镜像构建入口。推荐在容器中使用 API 版本，因为它不依赖宿主机图形栈，并且现在已经接入真实的 Rust 下载核心。

## 功能特性

- **AES-128加密支持**: 处理加密的HLS流
- **并发下载**: 并行下载视频分片，提高下载速度
- **硬件加速转码**: 支持FFmpeg硬件加速（NVIDIA NVENC、AMD AMF等）
- **跨平台**: 支持多种部署方式（桌面GUI、REST API）

## 文件说明

- `Dockerfile` - Linux桌面应用镜像构建文件（需要 X11 / GUI 环境）
- `Dockerfile.api` - REST API 服务镜像构建文件（推荐）
- `docker-compose.yml` - Docker Compose多服务编排配置
- `.dockerignore` - Docker构建时忽略的文件列表
- `docker-build.sh` / `docker-build.bat` - 跨平台构建脚本

## 快速开始

### 方式一：使用构建脚本

**构建桌面应用版本 (默认):**
```bash
# Linux/macOS
./docker-build.sh

# Windows
docker-build.bat
```

**构建 API 服务版本:**
```bash
# Linux/macOS
./docker-build.sh api

# Windows
docker-build.bat api
```

**构建所有版本:**
```bash
# Linux/macOS
./docker-build.sh all

# Windows
docker-build.bat all
```

### 方式二：使用Docker Compose

**运行桌面GUI应用:**
```bash
# 启动桌面应用
docker-compose up -d

# 查看日志
docker-compose logs -f m3u8-downloader

# 停止服务
docker-compose down
```

**运行REST API服务:**
```bash
# 启动API服务
docker-compose --profile api up -d

# 查看API日志
docker-compose logs -f m3u8-api

# 停止API服务
docker-compose --profile api down
```

## 部署选项

### 1. 桌面GUI应用

适合需要图形界面的用户，提供完整的Flutter桌面应用体验。

```bash
# 直接运行
docker run -it --rm \
  -e DISPLAY=$DISPLAY \
  -v /tmp/.X11-unix:/tmp/.X11-unix \
  -v $(pwd)/downloads:/app/downloads \
  m3u8-downloader:desktop
```

**Windows用户注意:**
- 需要安装X11服务器（如VcXsrv或MobaXterm）
- 设置DISPLAY环境变量指向X服务器

### 2. REST API 服务

适合集成到其他系统或提供web服务。

```bash
# 直接运行
docker run -p 3000:3000 \
  -v $(pwd)/downloads:/app/downloads \
  m3u8-downloader:api
```

**API端点:**
- `POST /download` - 开始下载任务
- `POST /inspect` - 分析页面/直链，返回候选流列表（Web 端使用）
- `GET /status/:task_id` - 查询下载状态
- `GET /tasks` - 列出任务
- `GET /health` - 健康检查

**API 安全（强烈建议）：**

- **SSRF 防护默认开启**：拒绝解析到内网/回环/链路本地地址的目标。需要从本地网络（如 NAS）下载时，加 `-e FERRISLOAD_ALLOW_PRIVATE_NETWORKS=1`。
- **可选 Bearer 鉴权**：加 `-e FERRISLOAD_API_TOKEN=你的令牌` 后，除 `/health` 外的所有端点都要求 `Authorization: Bearer 你的令牌`，避免公开端口被滥用。

```bash
# 带鉴权的推荐启动方式
docker run -p 3000:3000 \
  -e FERRISLOAD_API_TOKEN=change-me \
  -v $(pwd)/downloads:/app/downloads \
  m3u8-downloader:api
```

**下载请求体示例:**
```json
{
  "url": "https://example.com/master.m3u8",
  "output_filename": "demo.mp4",
  "concurrency": 8,
  "retries": 3,
  "video_bitrate": 0,
  "audio_bitrate": 0,
  "keep_temp": false,
  "request_context": {
    "user_agent": "Mozilla/5.0",
    "referer": "https://example.com/",
    "origin": "https://example.com",
    "cookie": "session=...",
    "headers": {
      "Authorization": "Bearer ..."
    }
  }
}
```

说明：
- 只传 `url` 时，服务会先自动分析并挑选最佳候选媒体。
- 如果调用方已经知道精确流地址，可以直接传 `media_url`，并按需传 `audio_url`。

## Docker镜像说明

### 桌面应用镜像
- **基础镜像**: Ubuntu 22.04
- **构建过程**:
  1. 安装Flutter和Rust环境
  2. 生成flutter_rust_bridge代码
  3. 编译Rust后端库
  4. 构建Linux桌面应用
- **运行时依赖**: GTK3, FFmpeg, X11库
- **存储**: 自动挂载downloads目录

### API 服务镜像
- **基础镜像**: Ubuntu 22.04
- **构建过程**:
  1. 安装 Rust 构建环境
  2. 编译真实的 `m3u8_api_server`
  3. 将二进制复制到轻量运行时镜像
- **运行时依赖**: FFmpeg, SSL证书
- **端口**: 3000 (可配置)
- **健康检查**: 内置健康检查端点

## 环境变量

| 变量名 | 默认值 | 说明 |
|--------|--------|------|
| `FFMPEG_PATH` | `/usr/bin/ffmpeg` | FFmpeg可执行文件路径 |
| `DOWNLOAD_DIR` | `/app/downloads` | 下载文件存储目录 |
| `API_PORT` | `3000` | API 服务端口（仅 API 版本） |
| `API_HOST` | `0.0.0.0` | API 绑定地址 |
| `RUST_LOG` | `info` | 日志级别 |

## GitHub Actions 与 GHCR

Release 工作流会构建并推送多架构 API 镜像到 GHCR：

- `linux/amd64`
- `linux/arm64`
- `linux/arm/v7`

推送 tag（例如 `v1.2.0`）时，镜像标签会包含：

- `ghcr.io/<owner>/m3u8-downloader-api:v1.2.0`
- `ghcr.io/<owner>/m3u8-downloader-api:latest`

## 硬件加速支持

Docker容器支持以下硬件加速：

- **NVIDIA**: `h264_nvenc` (需要`--gpus all`参数)
- **AMD**: `h264_amf`
- **Intel**: QuickSync
- **软件**: `libx264` (fallback)

**启用GPU加速:**
```bash
docker run --gpus all -it m3u8-downloader:desktop
```

## 注意事项

1. **桌面应用**: 需要X11显示服务器支持GUI显示
2. **权限**: 确保downloads目录有适当的读写权限
3. **网络**: 应用需要访问互联网下载HLS流
4. **存储**: 下载文件会存储在挂载的downloads目录中
5. **性能**: 并发下载数量会影响系统资源使用

## 故障排除

### 常见问题

**GUI应用无法显示:**
```bash
# 检查X11权限
xhost +local:docker

# 或使用xauth
docker run -e XAUTHORITY=/tmp/.docker.xauth -v /tmp/.docker.xauth:/tmp/.docker.xauth m3u8-downloader:desktop
```

**权限问题:**
```bash
# 修复下载目录权限
sudo chown -R $USER:$USER downloads/
```

**构建失败:**
```bash
# 查看详细构建日志
docker build --progress=plain -t m3u8-downloader:desktop .
```

**API服务无法访问:**
```bash
# 检查端口映射
docker-compose --profile api ps

# 查看API日志
docker-compose --profile api logs m3u8-api
```

## 生产部署建议

1. **使用卷管理**: 配置Docker卷而不是绑定挂载
2. **资源限制**: 设置CPU和内存限制
3. **日志管理**: 配置日志轮转和外部日志收集
4. **监控**: 添加健康检查和监控端点
5. **备份**: 定期备份下载的文件

## 技术栈

- **前端**: Flutter (Dart)
- **后端**: Rust + Tokio异步运行时
- **桥接**: flutter_rust_bridge
- **媒体处理**: FFmpeg
- **网络**: reqwest + rustls
- **加密**: AES-128 (block-modes)
