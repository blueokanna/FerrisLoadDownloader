package com.bluevale.m3u8_downloader

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.PowerManager
import android.provider.Settings
import android.util.Log
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {

    companion object {
        private const val TAG = "MainActivity"
        private const val CHANNEL = "com.blue.ferrisload/media_store"
        private var rustLibLoaded = false

        init {
            try {
                System.loadLibrary("rust_lib_m3u8_downloader")
                rustLibLoaded = true
                Log.i(TAG, "✅ Rust library loaded successfully")
            } catch (e: UnsatisfiedLinkError) {
                Log.e(TAG, "❌ Failed to load rust_lib_m3u8_downloader: ${e.message}", e)
                // Don't crash the app — Flutter side will handle the missing library gracefully
            }
        }
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        Log.i(TAG, "✅ Flutter engine configured")

        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, CHANNEL)
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "saveToDownloads" -> {
                        val srcPath = call.argument<String>("srcPath")
                        val fileName = call.argument<String>("fileName")
                        val mimeType = call.argument<String>("mimeType") ?: "video/mp4"
                        val subDir = call.argument<String>("subDir") ?: "FerrisLoad"

                        if (srcPath == null || fileName == null) {
                            result.error("INVALID_ARGS", "srcPath and fileName are required", null)
                            return@setMethodCallHandler
                        }

                        val savedPath = MediaStoreHelper.saveToDownloads(
                            applicationContext, srcPath, fileName, mimeType, subDir
                        )
                        if (savedPath != null) result.success(savedPath)
                        else result.error("SAVE_FAILED", "Failed to save file to Downloads", null)
                    }

                    "saveToPath" -> {
                        val srcPath = call.argument<String>("srcPath")
                        val destDir = call.argument<String>("destDir")
                        val fileName = call.argument<String>("fileName")

                        if (srcPath == null || destDir == null || fileName == null) {
                            result.error("INVALID_ARGS", "srcPath, destDir and fileName are required", null)
                            return@setMethodCallHandler
                        }

                        val savedPath = MediaStoreHelper.saveToPath(
                            applicationContext, srcPath, destDir, fileName
                        )
                        if (savedPath != null) result.success(savedPath)
                        else result.error("SAVE_FAILED", "Failed to save file", null)
                    }

                    "getAppExternalFilesDir" -> {
                        val dir = applicationContext.getExternalFilesDir(null)
                        result.success(dir?.absolutePath ?: applicationContext.filesDir.absolutePath)
                    }

                    "getDefaultDownloadDir" -> {
                        @Suppress("DEPRECATION")
                        val dl = Environment.getExternalStoragePublicDirectory(
                            Environment.DIRECTORY_DOWNLOADS
                        )
                        result.success(dl.absolutePath)
                    }

                    // ── 前台服务 ──
                    "startForegroundService" -> {
                        DownloadForegroundService.start(applicationContext)
                        result.success(true)
                    }
                    "stopForegroundService" -> {
                        DownloadForegroundService.stop(applicationContext)
                        result.success(true)
                    }
                    "updateServiceProgress" -> {
                        val progress = call.argument<Int>("progress") ?: 0
                        val status = call.argument<String>("status") ?: ""
                        DownloadForegroundService.updateProgress(applicationContext, progress, status)
                        result.success(true)
                    }

                    // ── 电池优化 ──
                    "isIgnoringBatteryOptimizations" -> {
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                            val pm = getSystemService(POWER_SERVICE) as PowerManager
                            result.success(pm.isIgnoringBatteryOptimizations(packageName))
                        } else {
                            result.success(true)
                        }
                    }
                    "requestIgnoreBatteryOptimizations" -> {
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                            val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                                data = Uri.parse("package:$packageName")
                            }
                            startActivity(intent)
                            result.success(true)
                        } else {
                            result.success(true)
                        }
                    }

                    else -> result.notImplemented()
                }
            }
    }
}
