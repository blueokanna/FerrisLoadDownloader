package com.bluevale.m3u8_downloader

import android.content.ContentValues
import android.content.Context
import android.media.MediaScannerConnection
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.util.Log
import java.io.File
import java.io.FileInputStream

/**
 * 使用 MediaStore API 将文件保存到公共存储，
 * 确保在 Android 10+ (Scoped Storage) 下文件对用户可见。
 */
object MediaStoreHelper {
    private const val TAG = "MediaStoreHelper"

    /**
     * 将应用私有目录中的文件复制到公共 Downloads 子目录。
     *
     * @param context    应用上下文
     * @param srcPath    源文件绝对路径（应用私有目录中）
     * @param fileName   目标文件名（如 "output.mp4"）
     * @param mimeType   MIME 类型（如 "video/mp4"）
     * @param subDir     Downloads 下的子目录名（如 "FerrisLoad"）
     * @return 保存成功后的公共路径字符串，失败返回 null
     */
    @JvmStatic
    fun saveToDownloads(
        context: Context,
        srcPath: String,
        fileName: String,
        mimeType: String,
        subDir: String
    ): String? {
        val srcFile = File(srcPath)
        if (!srcFile.exists()) {
            Log.e(TAG, "Source file does not exist: $srcPath")
            return null
        }

        return try {
            val safeName = sanitizeFileName(fileName)
            val safeSubDir = sanitizeSubDir(subDir)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                saveViaMediaStore(context, srcFile, safeName, mimeType, safeSubDir)
            } else {
                saveViaDirectCopy(context, srcFile, safeName, safeSubDir)
            }
        } catch (e: Exception) {
            Log.e(TAG, "Failed to save file to Downloads: ${e.message}", e)
            null
        }
    }

    /**
     * Strips path separators, reserved characters and control bytes from a
     * caller-provided file name so it can never escape the destination
     * directory (path traversal) or break MediaStore's display name.
     */
    private fun sanitizeFileName(fileName: String): String {
        val cleaned = fileName
            .replace(Regex("[\\\\/:*?\"<>|\\x00-\\x1f]"), "_")
            .trim('.', ' ')
        return cleaned.ifEmpty { "video.mp4" }
    }

    /**
     * Sanitizes a sub-directory name the same way, so `../..` or
     * `foo/../../etc` cannot redirect the destination outside Downloads.
     */
    private fun sanitizeSubDir(subDir: String): String {
        val cleaned = subDir
            .replace(Regex("[\\\\/:*?\"<>|\\x00-\\x1f]"), "_")
            .trim('.', ' ')
        return cleaned.ifEmpty { "FerrisLoad" }
    }

    /**
     * 将文件直接复制到用户指定的绝对路径目录。
     * 用于 Android 9 及以下，或用户通过 SAF 选择了可写目录后
     * 在 Dart 侧拿到了真实路径的场景。
     */
    @JvmStatic
    fun saveToPath(
        context: Context,
        srcPath: String,
        destDir: String,
        fileName: String
    ): String? {
        val srcFile = File(srcPath)
        if (!srcFile.exists()) {
            Log.e(TAG, "Source file does not exist: $srcPath")
            return null
        }
        return try {
            val dir = File(destDir)
            if (!dir.exists()) dir.mkdirs()
            // Defense in depth: never let a caller-provided file name escape
            // the chosen directory (path traversal).
            val destFile = File(dir, sanitizeFileName(fileName))
            srcFile.copyTo(destFile, overwrite = true)
            MediaScannerConnection.scanFile(
                context,
                arrayOf(destFile.absolutePath),
                arrayOf("video/mp4"),
                null
            )
            Log.i(TAG, "File saved to path: ${destFile.absolutePath}")
            destFile.absolutePath
        } catch (e: Exception) {
            Log.e(TAG, "saveToPath failed: ${e.message}", e)
            null
        }
    }

    private fun saveViaMediaStore(
        context: Context,
        srcFile: File,
        fileName: String,
        mimeType: String,
        subDir: String
    ): String? {
        val resolver = context.contentResolver
        val relativePath = if (subDir.isNotEmpty()) {
            "${Environment.DIRECTORY_DOWNLOADS}/$subDir"
        } else {
            Environment.DIRECTORY_DOWNLOADS
        }

        val contentValues = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, fileName)
            put(MediaStore.MediaColumns.MIME_TYPE, mimeType)
            put(MediaStore.MediaColumns.RELATIVE_PATH, relativePath)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                put(MediaStore.MediaColumns.IS_PENDING, 1)
            }
        }

        val collection: Uri = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        } else {
            MediaStore.Files.getContentUri("external")
        }

        val itemUri = resolver.insert(collection, contentValues)
        if (itemUri == null) {
            Log.e(TAG, "MediaStore insert returned null")
            return null
        }

        resolver.openOutputStream(itemUri)?.use { outputStream ->
            FileInputStream(srcFile).use { inputStream ->
                inputStream.copyTo(outputStream, bufferSize = 65536)
                outputStream.flush()
            }
        } ?: run {
            Log.e(TAG, "Failed to open output stream for URI: $itemUri")
            resolver.delete(itemUri, null, null)
            return null
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val updateValues = ContentValues().apply {
                put(MediaStore.MediaColumns.IS_PENDING, 0)
            }
            resolver.update(itemUri, updateValues, null, null)
        }

        val savedPath = getPathFromUri(context, itemUri)
        Log.i(TAG, "File saved via MediaStore: $savedPath (URI: $itemUri)")

        @Suppress("DEPRECATION")
        return savedPath
            ?: "${Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS)}/$subDir/$fileName"
    }

    private fun saveViaDirectCopy(
        context: Context,
        srcFile: File,
        fileName: String,
        subDir: String
    ): String? {
        @Suppress("DEPRECATION")
        val base = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS)
        val downloadsDir = if (subDir.isNotEmpty()) File(base, subDir) else base
        if (!downloadsDir.exists()) downloadsDir.mkdirs()

        val destFile = File(downloadsDir, fileName)
        srcFile.copyTo(destFile, overwrite = true)

        MediaScannerConnection.scanFile(
            context,
            arrayOf(destFile.absolutePath),
            arrayOf("video/mp4"),
            null
        )
        Log.i(TAG, "File saved via direct copy: ${destFile.absolutePath}")
        return destFile.absolutePath
    }

    private fun getPathFromUri(context: Context, uri: Uri): String? {
        return try {
            val projection = arrayOf(MediaStore.MediaColumns.DATA)
            context.contentResolver.query(uri, projection, null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) {
                    val idx = cursor.getColumnIndexOrThrow(MediaStore.MediaColumns.DATA)
                    cursor.getString(idx)
                } else null
            }
        } catch (e: Exception) {
            Log.w(TAG, "Could not resolve path from URI: ${e.message}")
            null
        }
    }
}
