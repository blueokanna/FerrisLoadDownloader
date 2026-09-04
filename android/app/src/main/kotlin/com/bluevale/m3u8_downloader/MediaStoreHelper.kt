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
 * Saves files to shared storage via MediaStore so they remain visible on
 * Android 10+ (scoped storage).
 */
object MediaStoreHelper {
    private const val TAG = "MediaStoreHelper"

    /**
     * Copies a file from app-private storage into a public Downloads subdir.
     *
     * @param context application context
     * @param srcPath absolute source path (app-private)
     * @param fileName target file name, e.g. "output.mp4"
     * @param mimeType MIME type, e.g. "video/mp4"
     * @param subDir sub-directory under Downloads, e.g. "FerrisLoad"
     * @return the saved public path, or null on failure
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
     * Copies a file to a caller-provided absolute directory (Android 9 and
     * below, or after the user grants a writable dir via SAF).
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
