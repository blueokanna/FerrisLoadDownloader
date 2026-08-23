# 多阶段构建：构建阶段
FROM ubuntu:22.04 AS builder

# 避免交互式提示
ENV DEBIAN_FRONTEND=noninteractive

# 安装系统依赖
RUN apt-get update && apt-get install -y \
    curl \
    build-essential \
    pkg-config \
    libssl-dev \
    clang \
    cmake \
    ninja-build \
    libgtk-3-dev \
    libsecret-1-dev \
    liblzma-dev \
    git \
    unzip \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

# 安装Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# 安装Flutter
RUN git clone https://github.com/flutter/flutter.git -b stable --depth 1 /flutter
ENV PATH="/flutter/bin:/flutter/bin/cache/dart-sdk/bin:${PATH}"

# 验证安装
RUN flutter --version && rustc --version && cargo --version

# 设置工作目录
WORKDIR /app

# 复制项目文件
COPY . .

# 启用Linux桌面支持
RUN flutter config --enable-linux-desktop

# 获取Flutter依赖
RUN flutter pub get

# 生成Rust桥接代码
RUN test -f lib/src/rust/frb_generated.dart

# 构建Rust库
RUN cd rust && cargo build --release --locked

# 构建Linux应用
RUN flutter build linux --release

# 运行时阶段
FROM ubuntu:22.04

# 避免交互式提示
ENV DEBIAN_FRONTEND=noninteractive

# 安装运行时依赖
RUN apt-get update && apt-get install -y \
    ffmpeg \
    libgtk-3-0 \
    libblkid1 \
    liblzma5 \
    libgtk-3-dev \
    libx11-dev \
    libglib2.0-dev \
    && rm -rf /var/lib/apt/lists/*

# 创建应用用户
RUN useradd -m -s /bin/bash appuser

# 创建应用目录
RUN mkdir -p /app && chown -R appuser:appuser /app

# 切换到应用用户
USER appuser

# 设置工作目录
WORKDIR /app

# 从构建阶段复制构建产物
COPY --from=builder /app/build/linux/x64/release/bundle/ ./

# 创建下载目录
RUN mkdir -p downloads

# 设置环境变量
ENV FERRISLOAD_FFMPEG_PATH=/usr/bin/ffmpeg
ENV DOWNLOAD_DIR=/app/downloads

# 暴露端口（可选，用于API模式）
EXPOSE 3000

# 默认启动命令（可以作为GUI应用或命令行工具）
CMD ["./m3u8_downloader"]
