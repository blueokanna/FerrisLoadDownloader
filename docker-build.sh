#!/bin/bash

# M3U8下载器 Docker构建脚本

echo "开始构建M3U8下载器Docker镜像..."

# 检查参数
BUILD_TYPE=${1:-desktop}

case $BUILD_TYPE in
    "desktop")
        echo "构建桌面应用版本..."
        docker build -t m3u8-downloader:desktop -f Dockerfile .
        ;;
    "api")
        echo "构建API服务版本..."
        docker build -t m3u8-downloader:api -f Dockerfile.api .
        ;;
    "all")
        echo "构建所有版本..."
        docker build -t m3u8-downloader:desktop -f Dockerfile .
        docker build -t m3u8-downloader:api -f Dockerfile.api .
        ;;
    *)
        echo "用法: $0 [desktop|api|all]"
        echo "  desktop - 构建桌面GUI应用 (默认)"
        echo "  api     - 构建REST API服务"
        echo "  all     - 构建所有版本"
        exit 1
        ;;
esac

if [ $? -eq 0 ]; then
    echo "Docker镜像构建成功！"
    echo ""
    echo "运行方式："
    echo ""
    echo "桌面应用版本："
    echo "1. 使用Docker直接运行GUI版本："
    echo "   docker run -it --rm \\"
    echo "     -e DISPLAY=\$DISPLAY \\"
    echo "     -v /tmp/.X11-unix:/tmp/.X11-unix \\"
    echo "     -v \$(pwd)/downloads:/app/downloads \\"
    echo "     m3u8-downloader:desktop"
    echo ""
    echo "2. 使用docker-compose运行GUI版本："
    echo "   docker-compose up -d"
    echo ""
    echo "API服务版本："
    echo "1. 使用Docker直接运行API版本："
    echo "   docker run -p 3000:3000 -v \$(pwd)/downloads:/app/downloads m3u8-downloader:api"
    echo ""
    echo "2. 使用docker-compose运行API版本："
    echo "   docker-compose --profile api up -d"
    echo ""
    echo "API将在 http://localhost:3000 提供服务"
else
    echo "Docker镜像构建失败！"
    exit 1
fi
