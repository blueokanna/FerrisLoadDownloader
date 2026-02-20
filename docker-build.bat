@echo off
REM M3U8下载器 Docker构建脚本 (Windows)

echo 开始构建M3U8下载器Docker镜像...

REM 检查参数
if "%1"=="" (
    set BUILD_TYPE=desktop
) else (
    set BUILD_TYPE=%1
)

if "%BUILD_TYPE%"=="desktop" (
    echo 构建桌面应用版本...
    docker build -t m3u8-downloader:desktop -f Dockerfile .
) else if "%BUILD_TYPE%"=="api" (
    echo 构建API服务版本...
    docker build -t m3u8-downloader:api -f Dockerfile.api .
) else if "%BUILD_TYPE%"=="all" (
    echo 构建所有版本...
    docker build -t m3u8-downloader:desktop -f Dockerfile .
    docker build -t m3u8-downloader:api -f Dockerfile.api .
) else (
    echo 用法: %0 [desktop^|api^|all]
    echo   desktop - 构建桌面GUI应用 (默认)
    echo   api     - 构建REST API服务
    echo   all     - 构建所有版本
    exit /b 1
)

if %errorlevel% equ 0 (
    echo.
    echo Docker镜像构建成功！
    echo.
    echo 运行方式：
    echo.
    echo 桌面应用版本：
    echo 1. 使用Docker直接运行GUI版本：
    echo    docker run -it --rm ^
    echo      -e DISPLAY=%%DISPLAY%% ^
    echo      -v /tmp/.X11-unix:/tmp/.X11-unix ^
    echo      -v %%cd%%\downloads:/app/downloads ^
    echo      m3u8-downloader:desktop
    echo.
    echo 2. 使用docker-compose运行GUI版本：
    echo    docker-compose up -d
    echo.
    echo API服务版本：
    echo 1. 使用Docker直接运行API版本：
    echo    docker run -p 3000:3000 -v %%cd%%\downloads:/app/downloads m3u8-downloader:api
    echo.
    echo 2. 使用docker-compose运行API版本：
    echo    docker-compose --profile api up -d
    echo.
    echo API将在 http://localhost:3000 提供服务
) else (
    echo.
    echo Docker镜像构建失败！
    exit /b 1
)
