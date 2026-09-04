package com.bluevale.m3u8_downloader

import android.annotation.SuppressLint
import android.media.*
import android.os.Build
import android.util.Log
import android.view.Surface
import java.io.File
import java.io.IOException
import java.nio.ByteBuffer
import org.json.JSONArray
import org.json.JSONObject

@SuppressLint("LogNotTimber")
object MediaTranscoder {
    private const val TAG = "MediaTranscoder"

    @JvmStatic
    fun capabilityReport(): String {
        val codecs = MediaCodecList(MediaCodecList.ALL_CODECS).codecInfos
        val hardwareEncoders =
                codecs.asSequence()
                        .filter {
                            it.isEncoder &&
                                    !isSoftwareCodec(it) &&
                                    supportsSurfaceInput(it, "video/avc")
                        }
                        .flatMap { codec ->
                            codec.supportedTypes
                                    .asSequence()
                                    .filter { it.startsWith("video/", ignoreCase = true) }
                                    .map { mime -> "${codec.name} | ${mime.lowercase()}" }
                        }
                        .distinct()
                        .sorted()
                        .toList()
        val hardwareDecoders =
                codecs.asSequence()
                        .filter { !it.isEncoder && !isSoftwareCodec(it) }
                        .flatMap { codec ->
                            codec.supportedTypes
                                    .asSequence()
                                    .filter { it.startsWith("video/", ignoreCase = true) }
                                    .map { mime -> "${codec.name} | ${mime.lowercase()}" }
                        }
                        .distinct()
                        .sorted()
                        .toList()

        return JSONObject()
                .put("hardwareVideoEncoders", JSONArray(hardwareEncoders))
                .put("hardwareVideoDecoders", JSONArray(hardwareDecoders))
                .toString()
    }

    private const val TIMEOUT_US = 10_000L // 10ms per poll
    private const val MAX_STALL_MS = 30_000L // 30s stall → abort

    // PTS re-basing threshold. MediaExtractor yields samples in DECODE order,
    // so H.264/HEVC streams with B-frames legitimately step BACKWARD by a few
    // frame periods inside every GOP. Re-basing on every backward step (the
    // old `<= lastPts` test) rewrote the whole timeline of B-frame content:
    // stretched duration, duplicated/lost timestamps, wrong effective fps.
    // Only treat a backward jump bigger than this as a real discontinuity (an
    // HLS/TS segment resetting its PTS back to ~0), which is always
    // seconds-scale, never a sub-second B-frame reorder.
    private const val PTS_REBASE_THRESHOLD_US = 500_000L // 500 ms

    private data class EncoderSession(val codec: MediaCodec, val surface: Surface)

    @JvmStatic
    fun transcode(
            inputPath: String?,
            outputPath: String?,
            vBitrate: Int,
            aBitrate: Int,
            expectedDurationMs: Long
    ): Boolean {
        if (inputPath == null || outputPath == null) {
            Log.e(TAG, "transcode: input or output is null")
            return false
        }
        if (aBitrate > 0) {
            Log.e(TAG, "Audio bitrate conversion is not supported by the Android backend")
            return false
        }
        try {
            val inFile = File(inputPath)
            if (!inFile.exists()) {
                Log.e(TAG, "Input file does not exist: $inputPath")
                return false
            }
            if (inFile.length() == 0L) {
                Log.e(TAG, "Input file is empty: $inputPath")
                return false
            }

            // 1) 尝试 remux（仅在不需要重新编码时）
            if (vBitrate <= 0 && aBitrate <= 0) {
                val remuxOk = tryRemux(inputPath, outputPath)
                if (remuxOk &&
                                verifyOutput(outputPath) &&
                                verifyDuration(outputPath, expectedDurationMs, "remux")
                ) {
                    Log.i(TAG, "✅ Remux succeeded: ${File(outputPath).length()} bytes")
                    return true
                }
                // remux 失败、输出为空或时长被截断，清理后 fall through
                File(outputPath).delete()
                Log.w(
                        TAG,
                        "Remux failed, empty or truncated output, falling back to hardware transcode"
                )
            }

            // 2) 硬件转码
            val hwOk = hardwareTranscode(inputPath, outputPath, vBitrate, aBitrate)
            if (hwOk &&
                            verifyOutput(outputPath) &&
                            verifyDuration(outputPath, expectedDurationMs, "transcode")
            ) {
                Log.i(TAG, "✅ Hardware transcode succeeded: ${File(outputPath).length()} bytes")
                return true
            }
            File(outputPath).delete()
            Log.e(TAG, "Hardware transcode failed, produced empty output, or output was truncated")
            return false
        } catch (e: Exception) {
            Log.e(TAG, "transcode exception: ${e.message}", e)
            runCatching { File(outputPath).delete() }
            return false
        }
    }

    @JvmStatic
    fun mux(
            videoPath: String?,
            audioPath: String?,
            outputPath: String?,
            expectedDurationMs: Long
    ): Boolean {
        if (videoPath == null || audioPath == null || outputPath == null) {
            Log.e(TAG, "mux: input or output is null")
            return false
        }
        val videoFile = File(videoPath)
        val audioFile = File(audioPath)
        if (!videoFile.exists() || videoFile.length() == 0L) {
            Log.e(TAG, "mux: invalid video file $videoPath")
            return false
        }
        if (!audioFile.exists() || audioFile.length() == 0L) {
            Log.e(TAG, "mux: invalid audio file $audioPath")
            return false
        }

        var muxer: MediaMuxer? = null
        try {
            val videoTrack = findTrack(videoPath, true)
            val audioTrack = findTrack(audioPath, false)
            if (videoTrack.index < 0 || audioTrack.index < 0) {
                Log.e(TAG, "mux: missing video or audio track")
                return false
            }
            if (!isMp4MuxableVideo(videoTrack.mime) || !isMp4MuxableAudio(audioTrack.mime)) {
                Log.e(
                        TAG,
                        "mux: unsupported mime video=${videoTrack.mime} audio=${audioTrack.mime}"
                )
                return false
            }

            muxer = MediaMuxer(outputPath, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)
            val muxVideoIndex = muxer.addTrack(videoTrack.format)
            val muxAudioIndex = muxer.addTrack(audioTrack.format)
            muxer.start()
            val videoSamples = writeTrackSamples(videoPath, videoTrack.index, muxVideoIndex, muxer)
            val audioSamples = writeTrackSamples(audioPath, audioTrack.index, muxAudioIndex, muxer)
            muxer.stop()
            val ok =
                    videoSamples > 0 &&
                            audioSamples > 0 &&
                            verifyOutput(outputPath) &&
                            verifyDuration(outputPath, expectedDurationMs, "mux")
            if (!ok) File(outputPath).delete()
            Log.i(TAG, "mux done video=$videoSamples audio=$audioSamples ok=$ok")
            return ok
        } catch (e: Exception) {
            Log.e(TAG, "mux exception: ${e.message}", e)
            runCatching { File(outputPath).delete() }
            return false
        } finally {
            runCatching { muxer?.release() }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  SEGMENT-BASED PIPELINES — the concat-demuxer equivalent for Android.
    //
    //  A naively byte-concatenated HLS TS resets its PTS at every segment
    //  boundary, which makes MediaExtractor / MediaCodec drop frames after
    //  the first segment ("only the first few seconds survive"). Instead of
    //  fighting that, we open EVERY segment independently and re-base the
    //  timeline continuously across segments, exactly like feeding ffmpeg's
    //  concat demuxer. The Rust side only calls these when the per-segment
    //  files are available and always falls back to the legacy merged-TS
    //  path on failure, so nothing can regress.
    // ═══════════════════════════════════════════════════════════════════════

    @JvmStatic
    fun transcodeDir(
            segmentDir: String?,
            prefix: String?,
            total: Int,
            outputPath: String?,
            vBitrate: Int,
            aBitrate: Int,
            expectedDurationMs: Long
    ): Boolean {
        if (segmentDir == null || prefix == null || outputPath == null) {
            Log.e(TAG, "transcodeDir: null argument")
            return false
        }
        if (total <= 0) {
            Log.e(TAG, "transcodeDir: total segment count must be positive")
            return false
        }
        if (!File(segmentDir).isDirectory) {
            Log.e(TAG, "transcodeDir: segment directory missing: $segmentDir")
            return false
        }
        if (aBitrate > 0) {
            Log.e(TAG, "Audio bitrate conversion is not supported by the Android backend")
            return false
        }

        // 1) Stream-copy remux of every segment (preferred when no re-encode).
        if (vBitrate <= 0 && aBitrate <= 0) {
            val remuxOk = remuxSegments(segmentDir, prefix, total, outputPath)
            if (remuxOk &&
                            verifyOutput(outputPath) &&
                            verifyDuration(outputPath, expectedDurationMs, "remux")
            ) {
                Log.i(TAG, "✅ Segment remux succeeded: ${File(outputPath).length()} bytes")
                return true
            }
            File(outputPath).delete()
            Log.w(
                    TAG,
                    "Segment remux failed, empty or truncated output; falling back to hardware transcode"
            )
        }

        // 2) Hardware (or software fallback) transcode feeding every segment
        //    into ONE continuous decode→encode→mux pipeline.
        val hwOk =
                hardwareTranscodeSegments(segmentDir, prefix, total, outputPath, vBitrate, aBitrate)
        if (hwOk &&
                        verifyOutput(outputPath) &&
                        verifyDuration(outputPath, expectedDurationMs, "transcode")
        ) {
            Log.i(TAG, "✅ Segment hardware transcode succeeded: ${File(outputPath).length()} bytes")
            return true
        }
        File(outputPath).delete()
        Log.e(TAG, "Segment transcode failed, produced empty output, or output was truncated")
        return false
    }

    @JvmStatic
    fun muxDirs(
            videoDir: String?,
            videoPrefix: String?,
            videoTotal: Int,
            audioDir: String?,
            audioPrefix: String?,
            audioTotal: Int,
            outputPath: String?,
            expectedDurationMs: Long
    ): Boolean {
        if (videoDir == null ||
                        videoPrefix == null ||
                        audioDir == null ||
                        audioPrefix == null ||
                        outputPath == null
        ) {
            Log.e(TAG, "muxDirs: null argument")
            return false
        }
        if (videoTotal <= 0 || audioTotal <= 0) {
            Log.e(TAG, "muxDirs: segment counts must be positive")
            return false
        }
        if (!File(videoDir).isDirectory || !File(audioDir).isDirectory) {
            Log.e(TAG, "muxDirs: segment directory missing")
            return false
        }

        return try {
            val ok =
                    remuxDirs(
                            videoDir,
                            videoPrefix,
                            videoTotal,
                            audioDir,
                            audioPrefix,
                            audioTotal,
                            outputPath
                    ) &&
                            verifyOutput(outputPath) &&
                            verifyDuration(outputPath, expectedDurationMs, "mux")
            if (!ok) File(outputPath).delete()
            Log.i(TAG, "muxDirs done ok=$ok")
            ok
        } catch (e: Exception) {
            Log.e(TAG, "muxDirs exception: ${e.message}", e)
            runCatching { File(outputPath).delete() }
            false
        }
    }

    private fun segmentPath(dir: String, prefix: String, index: Int): String =
            File(dir, String.format("%s_%05d.part", prefix, index)).absolutePath

    /** Returns the first video/audio track index in `extractor`, or -1. */
    private fun findTrackIdx(extractor: MediaExtractor, video: Boolean): Int {
        for (i in 0 until extractor.trackCount) {
            val mime = extractor.getTrackFormat(i).getString(MediaFormat.KEY_MIME) ?: ""
            if (video && mime.startsWith("video/")) return i
            if (!video && mime.startsWith("audio/")) return i
        }
        return -1
    }

    /**
     * Measures the real video frame rate of `path` from actual sample PTS instead of trusting the
     * container's (often wrong or missing) FRAME_RATE metadata. A wrong encoder `KEY_FRAME_RATE`
     * makes some hardware encoders re-time or duplicate frames, which surfaces as playback stutter.
     */
    private fun measureSegmentFps(path: String): Int {
        val ext = MediaExtractor()
        return try {
            ext.setDataSource(path)
            val vIdx = findTrackIdx(ext, true)
            if (vIdx < 0) return 30
            ext.selectTrack(vIdx)
            var firstPts = Long.MAX_VALUE
            var lastPts = Long.MIN_VALUE
            var frames = 0
            val scratch = ByteBuffer.allocateDirect(4 * 1024 * 1024)
            while (true) {
                val sz = ext.readSampleData(scratch, 0)
                if (sz < 0) break
                val pts = ext.sampleTime
                if (pts >= 0) {
                    if (pts < firstPts) firstPts = pts
                    if (pts > lastPts) lastPts = pts
                    frames++
                }
                ext.advance()
            }
            // Require a representative sample before trusting the estimate.
            if (frames >= 5 && lastPts > firstPts) {
                val durationUs = lastPts - firstPts
                val measured = (frames - 1) * 1_000_000L / durationUs
                measured.coerceIn(1, 120).toInt()
            } else {
                30
            }
        } catch (e: Exception) {
            Log.w(TAG, "measureSegmentFps failed: ${e.message}")
            30
        } finally {
            ext.release()
        }
    }

    /**
     * Stream-copy remux of one stream across all segments into `muxer`'s `dstIdx` track. Every
     * segment is opened with its own extractor and the PTS timeline is re-based continuously across
     * segment boundaries, so a per-segment PTS reset can never make the output non-monotonic. A
     * corrupt/unreadable segment is skipped rather than aborting the whole download (the duration
     * check still catches severe truncation).
     */
    private fun writeSegmentTrack(
            segmentDir: String,
            prefix: String,
            total: Int,
            video: Boolean,
            dstIdx: Int,
            muxer: MediaMuxer
    ): Int {
        val buf = ByteBuffer.allocateDirect(8 * 1024 * 1024)
        val info = MediaCodec.BufferInfo()
        var lastPts = Long.MIN_VALUE
        var ptsOffset = 0L
        var firstPts = -1L
        var samples = 0
        for (i in 0 until total) {
            val ext = MediaExtractor()
            try {
                ext.setDataSource(segmentPath(segmentDir, prefix, i))
                val srcIdx = findTrackIdx(ext, video)
                if (srcIdx < 0) {
                    Log.w(
                            TAG,
                            "writeSegmentTrack: segment $i has no ${if (video) "video" else "audio"} track; skipping"
                    )
                    continue
                }
                ext.selectTrack(srcIdx)
                while (true) {
                    val sz = ext.readSampleData(buf, 0)
                    if (sz < 0) break
                    info.offset = 0
                    info.size = sz
                    val rawPts = ext.sampleTime
                    var adjustedPts = rawPts + ptsOffset
                    if (adjustedPts < lastPts - PTS_REBASE_THRESHOLD_US) {
                        ptsOffset += (lastPts - adjustedPts) + 1L
                        adjustedPts = rawPts + ptsOffset
                    }
                    lastPts = adjustedPts
                    info.presentationTimeUs = adjustedPts
                    info.flags = ext.sampleFlags
                    // Zero-base each track like the encoder output so audio
                    // and video stay aligned regardless of source PTS origin.
                    if (firstPts < 0) firstPts = info.presentationTimeUs
                    if (firstPts > 0) info.presentationTimeUs -= firstPts
                    if (info.presentationTimeUs < 0) info.presentationTimeUs = 0
                    muxer.writeSampleData(dstIdx, buf, info)
                    samples++
                    ext.advance()
                }
            } catch (e: Exception) {
                Log.w(TAG, "writeSegmentTrack: segment $i failed: ${e.message}")
            } finally {
                ext.release()
            }
        }
        return samples
    }

    /** Stream-copy remux of a single video stream (with in-band audio). */
    private fun remuxSegments(
            segmentDir: String,
            prefix: String,
            total: Int,
            outputPath: String
    ): Boolean {
        val firstPath = segmentPath(segmentDir, prefix, 0)
        var probe: MediaExtractor? = null
        try {
            val first = MediaExtractor()
            probe = first
            first.setDataSource(firstPath)
            val videoIdx = findTrackIdx(first, true)
            if (videoIdx < 0) {
                Log.e(TAG, "remuxSegments: no video track in first segment")
                return false
            }
            val vMime = first.getTrackFormat(videoIdx).getString(MediaFormat.KEY_MIME) ?: ""
            if (vMime != "video/avc" && vMime != "video/hevc") {
                Log.w(TAG, "remuxSegments: unsupported video codec $vMime for stream copy")
                return false
            }
            var audioIdx = findTrackIdx(first, false)
            if (audioIdx >= 0) {
                val aMime = first.getTrackFormat(audioIdx).getString(MediaFormat.KEY_MIME) ?: ""
                if (aMime != "audio/mp4a-latm" && aMime != "audio/aac") {
                    Log.w(TAG, "remuxSegments: dropping non-AAC audio track $aMime")
                    audioIdx = -1
                }
            }
            first.release()
            probe = null

            val muxer = MediaMuxer(outputPath, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)
            try {
                val setup = MediaExtractor().also { it.setDataSource(firstPath) }
                val vOutIdx = muxer.addTrack(setup.getTrackFormat(videoIdx))
                val aOutIdx =
                        if (audioIdx >= 0) muxer.addTrack(setup.getTrackFormat(audioIdx)) else -1
                setup.release()

                muxer.start()
                val videoSamples =
                        writeSegmentTrack(segmentDir, prefix, total, true, vOutIdx, muxer)
                var audioSamples = 0
                if (audioIdx >= 0 && aOutIdx >= 0) {
                    audioSamples =
                            writeSegmentTrack(segmentDir, prefix, total, false, aOutIdx, muxer)
                }
                muxer.stop()
                Log.i(TAG, "remuxSegments video=$videoSamples audio=$audioSamples")
                return videoSamples > 0
            } finally {
                runCatching { muxer.release() }
            }
        } catch (e: Exception) {
            Log.w(TAG, "remuxSegments exception: ${e.message}", e)
            return false
        } finally {
            runCatching { probe?.release() }
        }
    }

    /** Stream-copy mux of a separate video stream and audio stream. */
    private fun remuxDirs(
            videoDir: String,
            videoPrefix: String,
            videoTotal: Int,
            audioDir: String,
            audioPrefix: String,
            audioTotal: Int,
            outputPath: String
    ): Boolean {
        val vFirst = segmentPath(videoDir, videoPrefix, 0)
        val aFirst = segmentPath(audioDir, audioPrefix, 0)
        var videoIdx = -1
        var audioIdx = -1
        try {
            val vProbe = MediaExtractor()
            try {
                vProbe.setDataSource(vFirst)
                videoIdx = findTrackIdx(vProbe, true)
                if (videoIdx < 0) return false
                val vMime = vProbe.getTrackFormat(videoIdx).getString(MediaFormat.KEY_MIME) ?: ""
                if (vMime != "video/avc" && vMime != "video/hevc") return false
            } finally {
                vProbe.release()
            }
            val aProbe = MediaExtractor()
            try {
                aProbe.setDataSource(aFirst)
                audioIdx = findTrackIdx(aProbe, false)
                if (audioIdx < 0) return false
                val aMime = aProbe.getTrackFormat(audioIdx).getString(MediaFormat.KEY_MIME) ?: ""
                if (aMime != "audio/mp4a-latm" && aMime != "audio/aac") return false
            } finally {
                aProbe.release()
            }

            val muxer = MediaMuxer(outputPath, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)
            try {
                val vSetup = MediaExtractor().also { it.setDataSource(vFirst) }
                val vOutIdx = muxer.addTrack(vSetup.getTrackFormat(videoIdx))
                vSetup.release()
                val aSetup = MediaExtractor().also { it.setDataSource(aFirst) }
                val aOutIdx = muxer.addTrack(aSetup.getTrackFormat(audioIdx))
                aSetup.release()

                muxer.start()
                val videoSamples =
                        writeSegmentTrack(videoDir, videoPrefix, videoTotal, true, vOutIdx, muxer)
                val audioSamples =
                        writeSegmentTrack(audioDir, audioPrefix, audioTotal, false, aOutIdx, muxer)
                muxer.stop()
                Log.i(TAG, "remuxDirs video=$videoSamples audio=$audioSamples")
                return videoSamples > 0 && audioSamples > 0
            } finally {
                runCatching { muxer.release() }
            }
        } catch (e: Exception) {
            Log.w(TAG, "remuxDirs exception: ${e.message}", e)
            return false
        }
    }

    /**
     * Hardware (or software fallback) transcode that feeds EVERY segment into one continuous
     * decoder → surface → encoder → muxer pipeline. The PTS timeline is re-based continuously
     * across segments, so the decoder never sees the per-segment PTS resets that used to make it
     * drop everything after the first segment.
     */
    private fun hardwareTranscodeSegments(
            segmentDir: String,
            prefix: String,
            total: Int,
            outputPath: String,
            vBitrate: Int,
            aBitrate: Int
    ): Boolean {
        val firstPath = segmentPath(segmentDir, prefix, 0)
        val probe = MediaExtractor()
        var muxer: MediaMuxer? = null
        var decoder: MediaCodec? = null
        var encoder: MediaCodec? = null
        var encoderSurface: Surface? = null
        var currentExtractor: MediaExtractor? = null

        try {
            probe.setDataSource(firstPath)
            val videoIdx = findTrackIdx(probe, true)
            if (videoIdx < 0) {
                Log.e(TAG, "hardwareTranscodeSegments: no video track")
                return false
            }
            val vFmt = probe.getTrackFormat(videoIdx)
            val vMime = vFmt.getString(MediaFormat.KEY_MIME) ?: "video/avc"
            val rotation = vFmt.safeInt(MediaFormat.KEY_ROTATION, 0)
            var w = vFmt.safeInt(MediaFormat.KEY_WIDTH, 1920)
            var h = vFmt.safeInt(MediaFormat.KEY_HEIGHT, 1080)
            if (rotation == 90 || rotation == 270) {
                val tmp = w
                w = h
                h = tmp
            }
            val fps = measureSegmentFps(firstPath).coerceAtLeast(1)
            Log.i(TAG, "Input video: $vMime ${w}x${h}@${fps}fps rotation=$rotation segments=$total")

            val targetBitrate =
                    if (vBitrate > 0) {
                        try {
                            Math.multiplyExact(vBitrate, 1000)
                        } catch (error: ArithmeticException) {
                            Log.e(
                                    TAG,
                                    "Video bitrate is too large for MediaCodec: $vBitrate kbit/s"
                            )
                            return false
                        }
                    } else {
                        val estimated = w.toLong() * h * fps * 7L / 100L
                        estimated.coerceIn(800_000L, 16_000_000L).toInt()
                    }
            val encFmt =
                    MediaFormat.createVideoFormat("video/avc", w, h).apply {
                        setInteger(
                                MediaFormat.KEY_COLOR_FORMAT,
                                MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface
                        )
                        setInteger(MediaFormat.KEY_BIT_RATE, targetBitrate)
                        setInteger(MediaFormat.KEY_FRAME_RATE, fps)
                        setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
                        // `KEY_OPERATING_RATE` is documented in *frames per
                        // second* (not µs): asking for `fps * 1000` (e.g.
                        // 30_000 fps for 30 fps content) confuses the codec's
                        // rate scheduler and can make it re-time the output
                        // timeline.  Pass the real rate only.
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                            setInteger(MediaFormat.KEY_OPERATING_RATE, fps)
                        }
                        // Deliberately do NOT set KEY_PRIORITY=0 (realtime).
                        // This is an offline batch transcode fed as fast as the
                        // hardware can consume it; realtime priority makes some
                        // vendor encoders pace output against the wall clock
                        // instead of the input timestamps, which stretches the
                        // timeline (the source of “30 min becomes 60 min” and
                        // “30 fps plays at ~15 fps” reports).  The default
                        // non-realtime priority lets the encoder honor input
                        // presentation timestamps.
                        // No B-frames: lower encode latency and simpler decode on
                        // low-end players; most AVC hardware encoders default to 0.
                        setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
                    }
            val encoderSession = createAvcEncoder(encFmt, w, h, fps, targetBitrate)
            if (encoderSession == null) {
                Log.e(TAG, "No AVC encoder (hardware or software) is available on this device")
                return false
            }
            encoder = encoderSession.codec
            encoderSurface = encoderSession.surface

            val decoderInfo = selectCodec(vMime, encoder = false, hardwareOnly = false)
            decoder =
                    if (decoderInfo != null) {
                        Log.i(TAG, "Using decoder ${decoderInfo.name}")
                        MediaCodec.createByCodecName(decoderInfo.name)
                    } else {
                        Log.w(TAG, "No preferred decoder found for $vMime, using system default")
                        MediaCodec.createDecoderByType(vMime)
                    }
            decoder.configure(vFmt, encoderSurface, null, 0)
            decoder.start()

            muxer = MediaMuxer(outputPath, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)

            // Audio passthrough preparation from the first segment.
            var audioMuxIdx = -1
            val aProbe = MediaExtractor()
            try {
                aProbe.setDataSource(firstPath)
                val audioIdx = findTrackIdx(aProbe, false)
                if (audioIdx >= 0) {
                    val aFmt = aProbe.getTrackFormat(audioIdx)
                    val aMime = aFmt.getString(MediaFormat.KEY_MIME) ?: ""
                    if (aMime == "audio/mp4a-latm" || aMime == "audio/aac") {
                        audioMuxIdx = muxer.addTrack(aFmt)
                    } else {
                        Log.w(TAG, "hardwareTranscodeSegments: dropping non-AAC audio $aMime")
                    }
                }
            } finally {
                aProbe.release()
            }

            var videoMuxIdx = -1
            var muxerStarted = false
            var inputDone = false
            var decoderDone = false
            var encoderDone = false
            var frames = 0
            var firstVideoPts = -1L
            var lastProgressMs = System.currentTimeMillis()
            var lastVideoDecodePts = Long.MIN_VALUE
            var videoPtsOffset = 0L
            var segmentIndex = 0

            val decInfo = MediaCodec.BufferInfo()
            val encInfo = MediaCodec.BufferInfo()

            while (!encoderDone) {
                // ── 1. Feed decoder across segments (continuous PTS) ──
                if (!inputDone) {
                    val idx = decoder.dequeueInputBuffer(TIMEOUT_US)
                    if (idx >= 0) {
                        var fed = false
                        while (!fed) {
                            if (currentExtractor == null) {
                                if (segmentIndex >= total) {
                                    decoder.queueInputBuffer(
                                            idx,
                                            0,
                                            0,
                                            0,
                                            MediaCodec.BUFFER_FLAG_END_OF_STREAM
                                    )
                                    inputDone = true
                                    fed = true
                                    break
                                }
                                val path = segmentPath(segmentDir, prefix, segmentIndex)
                                segmentIndex++
                                val next = MediaExtractor()
                                try {
                                    next.setDataSource(path)
                                    val vIdx = findTrackIdx(next, true)
                                    if (vIdx < 0) {
                                        next.release()
                                        continue // segment has no video: skip
                                    }
                                    next.selectTrack(vIdx)
                                    currentExtractor = next
                                } catch (e: Exception) {
                                    runCatching { next.release() }
                                    continue // corrupt segment: skip
                                }
                            }
                            val ext = currentExtractor!!
                            val buf = decoder.getInputBuffer(idx)!!
                            val sz = ext.readSampleData(buf, 0)
                            if (sz < 0) {
                                ext.release()
                                currentExtractor = null
                                continue // exhausted: continue with next segment
                            }
                            val rawPts = ext.sampleTime
                            var adjustedPts = rawPts + videoPtsOffset
                            if (adjustedPts < lastVideoDecodePts - PTS_REBASE_THRESHOLD_US) {
                                videoPtsOffset += (lastVideoDecodePts - adjustedPts) + 1L
                                adjustedPts = rawPts + videoPtsOffset
                            }
                            lastVideoDecodePts = adjustedPts
                            decoder.queueInputBuffer(idx, 0, sz, adjustedPts, 0)
                            ext.advance()
                            fed = true
                        }
                        lastProgressMs = System.currentTimeMillis()
                    }
                }

                // ── 2. Drain decoder → Surface ──
                if (!decoderDone) {
                    val idx = decoder.dequeueOutputBuffer(decInfo, TIMEOUT_US)
                    if (idx >= 0) {
                        lastProgressMs = System.currentTimeMillis()
                        val eos = (decInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0
                        decoder.releaseOutputBuffer(idx, !eos)
                        if (eos) {
                            decoderDone = true
                            encoder.signalEndOfInputStream()
                        }
                    }
                }

                // ── 3. Drain encoder ──
                val idx = encoder.dequeueOutputBuffer(encInfo, TIMEOUT_US)
                when {
                    idx >= 0 -> {
                        lastProgressMs = System.currentTimeMillis()
                        val data = encoder.getOutputBuffer(idx)!!
                        if (encInfo.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                            encInfo.size = 0
                        }
                        if (encInfo.size > 0) {
                            if (!muxerStarted) {
                                videoMuxIdx = muxer.addTrack(encoder.outputFormat)
                                muxer.start()
                                muxerStarted = true
                            }
                            if (firstVideoPts < 0) {
                                firstVideoPts = encInfo.presentationTimeUs
                            }
                            if (firstVideoPts > 0) {
                                encInfo.presentationTimeUs -= firstVideoPts
                            }
                            if (encInfo.presentationTimeUs < 0) {
                                encInfo.presentationTimeUs = 0
                            }
                            data.position(encInfo.offset)
                            data.limit(encInfo.offset + encInfo.size)
                            muxer.writeSampleData(videoMuxIdx, data, encInfo)
                            frames++
                        }
                        encoder.releaseOutputBuffer(idx, false)
                        if (encInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) {
                            encoderDone = true
                        }
                    }
                    idx == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                        if (!muxerStarted) {
                            videoMuxIdx = muxer.addTrack(encoder.outputFormat)
                            muxer.start()
                            muxerStarted = true
                        }
                    }
                }

                if (System.currentTimeMillis() - lastProgressMs > MAX_STALL_MS) {
                    Log.e(TAG, "Segment transcode stalled for ${MAX_STALL_MS}ms, aborting")
                    return false
                }
            }

            // ── 4. Audio passthrough across all segments ──
            if (audioMuxIdx >= 0 && muxerStarted) {
                writeSegmentTrack(segmentDir, prefix, total, false, audioMuxIdx, muxer)
            }

            muxer.stop()
            Log.i(TAG, "Segment hardware transcode done: $frames video frames")
            return frames > 0
        } catch (e: Exception) {
            Log.e(TAG, "hardwareTranscodeSegments failed: ${e.message}", e)
            return false
        } finally {
            runCatching { currentExtractor?.release() }
            runCatching {
                decoder?.stop()
                decoder?.release()
            }
            runCatching {
                encoder?.stop()
                encoder?.release()
            }
            runCatching { encoderSurface?.release() }
            runCatching { muxer?.release() }
            probe.release()
        }
    }

    private fun verifyOutput(path: String): Boolean {
        val f = File(path)
        if (!f.exists() || f.length() <= 1024) return false
        val future = java.util.concurrent.Executors.newSingleThreadExecutor()
        return try {
            val task =
                    future.submit(
                            java.util.concurrent.Callable<Boolean> {
                                val extractor = MediaExtractor()
                                try {
                                    extractor.setDataSource(path)
                                    (0 until extractor.trackCount).any { index ->
                                        extractor
                                                .getTrackFormat(index)
                                                .getString(MediaFormat.KEY_MIME)
                                                ?.startsWith("video/") == true
                                    }
                                } catch (error: Exception) {
                                    Log.e(
                                            TAG,
                                            "verifyOutput could not read media tracks: ${error.message}",
                                            error
                                    )
                                    false
                                } finally {
                                    extractor.release()
                                }
                            }
                    )
            task.get(20, java.util.concurrent.TimeUnit.SECONDS)
        } catch (error: Exception) {
            Log.e(TAG, "verifyOutput timed out or failed: ${error.message}", error)
            false
        } finally {
            future.shutdownNow()
        }
    }

    /**
     * Rejects outputs whose duration is wrong versus what the HLS playlist
     * promised. Two failure shapes are caught:
     *
     *  1. Catastrophic truncation — Android's MediaExtractor can silently stop
     *     early when reading a naively-concatenated TS (timestamp
     *     discontinuities), which used to produce a "successful" output
     *     containing only the first few seconds after all download traffic was
     *     already spent.
     *  2. Timeline inflation — an encoder that re-times Surface input can
     *     stretch the output (the "30 min becomes 60 min" failure), which is
     *     just as wrong as a truncated one.
     *
     * The expected value is the SUM of the playlist EXTINF tags, and EXTINF is
     * rounded UP per segment, so the real media is routinely a few seconds to
     * a couple of minutes SHORTER than the sum for long streams (hundreds of
     * segments). The lower bound therefore only rejects losing a significant
     * fraction of the content (>= 20%), never a benign shortfall — otherwise a
     * correct long download would be rejected and needlessly re-encoded. When
     * the expected duration is unknown (0) this check is skipped.
     */
    private fun verifyDuration(path: String, expectedMs: Long, what: String): Boolean {
        if (expectedMs <= 0) return true
        val future = java.util.concurrent.Executors.newSingleThreadExecutor()
        val actualMs =
                try {
                    val task =
                            future.submit(
                                    java.util.concurrent.Callable<Long> {
                                        val retriever = MediaMetadataRetriever()
                                        try {
                                            retriever.setDataSource(path)
                                            retriever
                                                    .extractMetadata(
                                                            MediaMetadataRetriever
                                                                    .METADATA_KEY_DURATION
                                                    )
                                                    ?.toLongOrNull()
                                                    ?: 0L
                                        } finally {
                                            retriever.release()
                                        }
                                    }
                            )
                    task.get(20, java.util.concurrent.TimeUnit.SECONDS)
                } catch (error: Exception) {
                    Log.e(TAG, "$what duration probe timed out or failed: ${error.message}", error)
                    0L
                } finally {
                    future.shutdownNow()
                }
        if (actualMs <= 0) return true // cannot determine; do not block on it
        // 80% floor, or at worst 2 minutes short of the EXTINF sum for long
        // content (tolerates per-segment EXTINF round-up + trailing partial
        // segment); still catches the "only first few seconds" truncation.
        val minTolerance = (expectedMs * 80L / 100L).coerceAtLeast(expectedMs - 120_000L)
        if (actualMs + 1000L < minTolerance) {
            Log.e(TAG, "$what duration check failed: expected≈${expectedMs}ms got ${actualMs}ms")
            return false
        }
        // Upper bound: an output that is significantly LONGER than the playlist
        // says (e.g. a doubled timeline from an encoder that re-times Surface
        // input) is just as wrong as a truncated one.  Allow normal container
        // slack (trailing frames / segment rounding): up to 120% + 30 s.
        val maxTolerance = expectedMs + maxOf(30_000L, expectedMs / 5L)
        if (actualMs > maxTolerance) {
            Log.e(
                    TAG,
                    "$what duration check failed: expected≈${expectedMs}ms got ${actualMs}ms (inflated)"
            )
            return false
        }
        return true
    }

    private data class TrackInfo(val index: Int, val format: MediaFormat, val mime: String)

    private fun findTrack(path: String, video: Boolean): TrackInfo {
        val extractor = MediaExtractor()
        try {
            extractor.setDataSource(path)
            for (i in 0 until extractor.trackCount) {
                val format = extractor.getTrackFormat(i)
                val mime = format.getString(MediaFormat.KEY_MIME) ?: ""
                if ((video && mime.startsWith("video/")) || (!video && mime.startsWith("audio/"))) {
                    return TrackInfo(i, format, mime)
                }
            }
            return TrackInfo(-1, MediaFormat(), "")
        } finally {
            extractor.release()
        }
    }

    private fun isMp4MuxableVideo(mime: String): Boolean =
            mime == "video/avc" ||
                    mime == "video/hevc" ||
                    mime == "video/mp4v-es" ||
                    mime == "video/av01"

    private fun isMp4MuxableAudio(mime: String): Boolean =
            mime == "audio/mp4a-latm" || mime == "audio/aac"

    private fun writeTrackSamples(
            path: String,
            sourceIndex: Int,
            destinationIndex: Int,
            muxer: MediaMuxer
    ): Int {
        val extractor = MediaExtractor()
        try {
            extractor.setDataSource(path)
            extractor.selectTrack(sourceIndex)
            // 8 MiB so high-bitrate 4K H.264/HEVC samples (which can exceed
            // 1 MiB per NAL unit) never overflow the read buffer.
            val buffer = ByteBuffer.allocateDirect(8 * 1024 * 1024)
            val info = MediaCodec.BufferInfo()
            var firstPts = -1L
            var samples = 0
            // HLS video/audio streams are naive concatenations of independent
            // segments whose PTS restarts at zero at each boundary; re-base the
            // timeline so the merged MP4 is monotonic.
            var lastPts = Long.MIN_VALUE
            var ptsOffset = 0L
            while (true) {
                val size = extractor.readSampleData(buffer, 0)
                if (size < 0) break
                info.offset = 0
                info.size = size
                val rawPts = extractor.sampleTime
                var adjustedPts = rawPts + ptsOffset
                if (adjustedPts < lastPts - PTS_REBASE_THRESHOLD_US) {
                    ptsOffset += (lastPts - adjustedPts) + 1L
                    adjustedPts = rawPts + ptsOffset
                }
                lastPts = adjustedPts
                info.presentationTimeUs = adjustedPts
                info.flags = extractor.sampleFlags
                if (firstPts < 0) firstPts = info.presentationTimeUs
                if (firstPts > 0) info.presentationTimeUs -= firstPts
                if (info.presentationTimeUs < 0) info.presentationTimeUs = 0
                muxer.writeSampleData(destinationIndex, buffer, info)
                samples++
                extractor.advance()
            }
            return samples
        } finally {
            extractor.release()
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  REMUX — Pure repackaging, no recoding
    // ═══════════════════════════════════════════════════════════════════════

    private fun tryRemux(inputPath: String, outputPath: String): Boolean {
        var extractor: MediaExtractor? = null
        try {
            extractor = MediaExtractor().also { it.setDataSource(inputPath) }

            var videoIdx = -1
            var audioIdx = -1
            for (i in 0 until extractor.trackCount) {
                val mime = extractor.getTrackFormat(i).getString(MediaFormat.KEY_MIME) ?: ""
                if (mime.startsWith("video/") && videoIdx < 0) videoIdx = i
                else if (mime.startsWith("audio/") && audioIdx < 0) audioIdx = i
            }
            if (videoIdx < 0) return false

            val vMime = extractor.getTrackFormat(videoIdx).getString(MediaFormat.KEY_MIME) ?: ""
            if (vMime != "video/avc" && vMime != "video/hevc") return false

            if (audioIdx >= 0) {
                val aMime = extractor.getTrackFormat(audioIdx).getString(MediaFormat.KEY_MIME) ?: ""
                if (aMime != "audio/mp4a-latm" && aMime != "audio/aac") {
                    Log.w(TAG, "Remux: dropping non-AAC audio track $aMime")
                    audioIdx = -1
                }
            }

            extractor.release()
            extractor = null
            remuxTracks(inputPath, outputPath, videoIdx, audioIdx)
            return true
        } catch (e: Exception) {
            Log.w(TAG, "Remux exception: ${e.message}", e)
            return false
        } finally {
            runCatching { extractor?.release() }
        }
    }

    @Throws(IOException::class)
    private fun remuxTracks(inputPath: String, outputPath: String, videoIdx: Int, audioIdx: Int) {
        val muxer = MediaMuxer(outputPath, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)
        try {
            val setup = MediaExtractor().also { it.setDataSource(inputPath) }
            val vOutIdx = muxer.addTrack(setup.getTrackFormat(videoIdx))
            val aOutIdx = if (audioIdx >= 0) muxer.addTrack(setup.getTrackFormat(audioIdx)) else -1
            setup.release()

            muxer.start()

            val buf = ByteBuffer.allocateDirect(8 * 1024 * 1024)
            val info = MediaCodec.BufferInfo()

            // 逐轨道写入，每个轨道独立 extractor，避免 PTS 交叉
            val pairs = mutableListOf(videoIdx to vOutIdx)
            if (audioIdx >= 0 && aOutIdx >= 0) pairs.add(audioIdx to aOutIdx)

            for ((srcIdx, dstIdx) in pairs) {
                val ext =
                        MediaExtractor().also {
                            it.setDataSource(inputPath)
                            it.selectTrack(srcIdx)
                        }
                var firstPts = -1L
                var n = 0
                var lastPts = Long.MIN_VALUE
                var ptsOffset = 0L
                while (true) {
                    val sz = ext.readSampleData(buf, 0)
                    if (sz < 0) break
                    info.offset = 0
                    info.size = sz
                    val rawPts = ext.sampleTime
                    var adjustedPts = rawPts + ptsOffset
                    if (adjustedPts < lastPts - PTS_REBASE_THRESHOLD_US) {
                        ptsOffset += (lastPts - adjustedPts) + 1L
                        adjustedPts = rawPts + ptsOffset
                    }
                    lastPts = adjustedPts
                    info.presentationTimeUs = adjustedPts
                    info.flags = ext.sampleFlags
                    if (firstPts < 0) firstPts = info.presentationTimeUs
                    if (firstPts > 0) info.presentationTimeUs -= firstPts
                    if (info.presentationTimeUs < 0) info.presentationTimeUs = 0
                    muxer.writeSampleData(dstIdx, buf, info)
                    n++
                    ext.advance()
                }
                ext.release()
                Log.i(TAG, "Remux track $srcIdx→$dstIdx: $n samples")
            }
            muxer.stop()
        } finally {
            runCatching { muxer.release() }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  HARDWARE TRANSCODE — Surface 模式：Decoder → Surface → Encoder
    // ═══════════════════════════════════════════════════════════════════════

    private fun hardwareTranscode(
            inputPath: String,
            outputPath: String,
            vBitrate: Int,
            aBitrate: Int
    ): Boolean {
        val extractor = MediaExtractor()
        var muxer: MediaMuxer? = null
        var decoder: MediaCodec? = null
        var encoder: MediaCodec? = null
        var encoderSurface: Surface? = null

        try {
            extractor.setDataSource(inputPath)

            var videoIdx = -1
            var audioIdx = -1
            for (i in 0 until extractor.trackCount) {
                val mime = extractor.getTrackFormat(i).getString(MediaFormat.KEY_MIME) ?: ""
                if (mime.startsWith("video/") && videoIdx < 0) videoIdx = i
                else if (mime.startsWith("audio/") && audioIdx < 0) audioIdx = i
            }
            if (videoIdx < 0) {
                Log.e(TAG, "No video track")
                return false
            }

            val vFmt = extractor.getTrackFormat(videoIdx)
            val vMime = vFmt.getString(MediaFormat.KEY_MIME) ?: "video/avc"
            val rotation = vFmt.safeInt(MediaFormat.KEY_ROTATION, 0)
            var w = vFmt.safeInt(MediaFormat.KEY_WIDTH, 1920)
            var h = vFmt.safeInt(MediaFormat.KEY_HEIGHT, 1080)
            if (rotation == 90 || rotation == 270) {
                val tmp = w
                w = h
                h = tmp
            }
            val fps = measureSegmentFps(inputPath).coerceAtLeast(1)
            Log.i(TAG, "Input video: $vMime ${w}x${h}@${fps}fps rotation=$rotation")

            // ── 编码器 ──
            val targetBitrate =
                    if (vBitrate > 0) {
                        try {
                            Math.multiplyExact(vBitrate, 1000)
                        } catch (error: ArithmeticException) {
                            Log.e(
                                    TAG,
                                    "Video bitrate is too large for MediaCodec: $vBitrate kbit/s"
                            )
                            return false
                        }
                    } else {
                        val estimated = w.toLong() * h * fps.coerceAtLeast(1) * 7L / 100L
                        estimated.coerceIn(800_000L, 16_000_000L).toInt()
                    }
            val encFmt =
                    MediaFormat.createVideoFormat("video/avc", w, h).apply {
                        setInteger(
                                MediaFormat.KEY_COLOR_FORMAT,
                                MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface
                        )
                        setInteger(MediaFormat.KEY_BIT_RATE, targetBitrate)
                        setInteger(MediaFormat.KEY_FRAME_RATE, fps)
                        setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
                        // See the comment in hardwareTranscodeSegments: the
                        // operating rate is frames/second and realtime priority
                        // must not be set for an offline batch transcode.
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                            setInteger(MediaFormat.KEY_OPERATING_RATE, fps)
                        }
                        // No B-frames: lower encode latency and simpler decode.
                        setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
                    }
            val encoderSession = createAvcEncoder(encFmt, w, h, fps, targetBitrate)
            if (encoderSession == null) {
                Log.e(TAG, "No AVC encoder (hardware or software) is available on this device")
                return false
            }
            encoder = encoderSession.codec
            encoderSurface = encoderSession.surface

            // ── Decoder ──
            val decoderInfo = selectCodec(vMime, encoder = false, hardwareOnly = false)
            decoder =
                    if (decoderInfo != null) {
                        Log.i(TAG, "Using decoder ${decoderInfo.name}")
                        MediaCodec.createByCodecName(decoderInfo.name)
                    } else {
                        Log.w(
                                TAG,
                                "No preferred hardware decoder found for $vMime, using system default"
                        )
                        MediaCodec.createDecoderByType(vMime)
                    }
            decoder.configure(vFmt, encoderSurface, null, 0)
            decoder.start()

            // ── Muxer ──
            muxer = MediaMuxer(outputPath, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)

            // ── Audio passthrough preparation ──
            var audioMuxIdx = -1
            if (audioIdx >= 0) {
                val aFmt = extractor.getTrackFormat(audioIdx)
                val aMime = aFmt.getString(MediaFormat.KEY_MIME) ?: ""
                if (aMime == "audio/mp4a-latm" || aMime == "audio/aac") {
                    audioMuxIdx = muxer.addTrack(aFmt)
                } else {
                    Log.w(TAG, "hardwareTranscode: dropping non-AAC audio track $aMime")
                    audioIdx = -1
                }
            }

            extractor.selectTrack(videoIdx)

            var videoMuxIdx = -1
            var muxerStarted = false
            var inputDone = false
            var decoderDone = false
            var encoderDone = false
            var frames = 0
            var firstVideoPts = -1L
            var lastProgressMs = System.currentTimeMillis()
            var lastVideoDecodePts = Long.MIN_VALUE
            var videoPtsOffset = 0L

            val decInfo = MediaCodec.BufferInfo()
            val encInfo = MediaCodec.BufferInfo()

            while (!encoderDone) {
                // ── 1. Feed decoder ──
                if (!inputDone) {
                    val idx = decoder.dequeueInputBuffer(TIMEOUT_US)
                    if (idx >= 0) {
                        lastProgressMs = System.currentTimeMillis()
                        val buf = decoder.getInputBuffer(idx)!!
                        val sz = extractor.readSampleData(buf, 0)
                        if (sz < 0) {
                            decoder.queueInputBuffer(
                                    idx,
                                    0,
                                    0,
                                    0,
                                    MediaCodec.BUFFER_FLAG_END_OF_STREAM
                            )
                            inputDone = true
                        } else {
                            val rawPts = extractor.sampleTime
                            var adjustedPts = rawPts + videoPtsOffset
                            if (adjustedPts < lastVideoDecodePts - PTS_REBASE_THRESHOLD_US) {
                                videoPtsOffset += (lastVideoDecodePts - adjustedPts) + 1L
                                adjustedPts = rawPts + videoPtsOffset
                            }
                            lastVideoDecodePts = adjustedPts
                            decoder.queueInputBuffer(idx, 0, sz, adjustedPts, 0)
                            extractor.advance()
                        }
                    }
                }

                // ── 2. Drain decoder → Surface ──
                if (!decoderDone) {
                    val idx = decoder.dequeueOutputBuffer(decInfo, TIMEOUT_US)
                    if (idx >= 0) {
                        lastProgressMs = System.currentTimeMillis()
                        val eos = (decInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0
                        decoder.releaseOutputBuffer(idx, !eos)
                        if (eos) {
                            decoderDone = true
                            encoder.signalEndOfInputStream()
                        }
                    }
                }

                // ── 3. Drain encoder ──
                val idx = encoder.dequeueOutputBuffer(encInfo, TIMEOUT_US)
                when {
                    idx >= 0 -> {
                        lastProgressMs = System.currentTimeMillis()
                        val data = encoder.getOutputBuffer(idx)!!

                        if (encInfo.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                            encInfo.size = 0
                        }
                        if (encInfo.size > 0) {
                            if (!muxerStarted) {
                                videoMuxIdx = muxer.addTrack(encoder.outputFormat)
                                muxer.start()
                                muxerStarted = true
                            }
                            if (firstVideoPts < 0) {
                                firstVideoPts = encInfo.presentationTimeUs
                            }
                            if (firstVideoPts > 0) {
                                encInfo.presentationTimeUs -= firstVideoPts
                            }
                            if (encInfo.presentationTimeUs < 0) {
                                encInfo.presentationTimeUs = 0
                            }
                            data.position(encInfo.offset)
                            data.limit(encInfo.offset + encInfo.size)
                            muxer.writeSampleData(videoMuxIdx, data, encInfo)
                            frames++
                        }
                        encoder.releaseOutputBuffer(idx, false)

                        if (encInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) {
                            encoderDone = true
                        }
                    }
                    idx == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                        if (!muxerStarted) {
                            videoMuxIdx = muxer.addTrack(encoder.outputFormat)
                            muxer.start()
                            muxerStarted = true
                        }
                    }
                }

                if (System.currentTimeMillis() - lastProgressMs > MAX_STALL_MS) {
                    Log.e(TAG, "Transcode stalled for ${MAX_STALL_MS}ms, aborting")
                    return false
                }
            }

            if (audioIdx >= 0 && audioMuxIdx >= 0 && muxerStarted) {
                writeAudioPassthrough(inputPath, audioIdx, audioMuxIdx, muxer)
            }

            muxer.stop()
            Log.i(TAG, "Hardware transcode done: $frames video frames")
            return frames > 0
        } catch (e: Exception) {
            Log.e(TAG, "hardwareTranscode failed: ${e.message}", e)
            return false
        } finally {
            runCatching {
                decoder?.stop()
                decoder?.release()
            }
            runCatching {
                encoder?.stop()
                encoder?.release()
            }
            runCatching { encoderSurface?.release() }
            runCatching { muxer?.release() }
            extractor.release()
        }
    }

    /**
     * Creates an AVC encoder that prefers dedicated hardware and falls back to a software (CPU)
     * encoder when no hardware encoder supports the requested resolution/bitrate. This guarantees
     * the transcode path works on every device instead of failing when GPU encoders are missing or
     * refuse the format.
     */
    private fun createAvcEncoder(
            format: MediaFormat,
            width: Int,
            height: Int,
            frameRate: Int,
            bitrate: Int
    ): EncoderSession? {
        createEncoderWith(
                        codecCandidates("video/avc", encoder = true, hardwareOnly = true),
                        format,
                        width,
                        height,
                        frameRate,
                        bitrate
                )
                ?.let {
                    return it
                }
        Log.w(TAG, "No suitable hardware AVC encoder; falling back to software encoder")
        return createEncoderWith(
                codecCandidates("video/avc", encoder = true, hardwareOnly = false),
                format,
                width,
                height,
                frameRate,
                bitrate
        )
    }

    private fun createEncoderWith(
            candidates: List<MediaCodecInfo>,
            format: MediaFormat,
            width: Int,
            height: Int,
            frameRate: Int,
            bitrate: Int
    ): EncoderSession? {
        for (codecInfo in candidates) {
            val capabilities =
                    try {
                        codecInfo.getCapabilitiesForType("video/avc")
                    } catch (error: Exception) {
                        Log.w(TAG, "Cannot query ${codecInfo.name}: ${error.message}")
                        continue
                    }
            if (!capabilities.colorFormats.contains(
                            MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface
                    )
            ) {
                continue
            }

            val videoCapabilities = capabilities.videoCapabilities ?: continue
            val supported =
                    runCatching {
                                videoCapabilities.isSizeSupported(width, height) &&
                                        videoCapabilities.areSizeAndRateSupported(
                                                width,
                                                height,
                                                frameRate.coerceAtLeast(1).toDouble()
                                        ) &&
                                        videoCapabilities.bitrateRange.contains(bitrate)
                            }
                            .getOrDefault(false)
            if (!supported) {
                Log.i(
                        TAG,
                        "Skipping ${codecInfo.name}: unsupported ${width}x$height@$frameRate or $bitrate bps"
                )
                continue
            }

            var codec: MediaCodec? = null
            try {
                codec = MediaCodec.createByCodecName(codecInfo.name)
                codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                val surface = codec.createInputSurface()
                codec.start()
                Log.i(TAG, "Using encoder ${codecInfo.name}")
                return EncoderSession(codec, surface)
            } catch (error: Exception) {
                Log.w(TAG, "Encoder ${codecInfo.name} rejected the format: ${error.message}")
                runCatching { codec?.release() }
            }
        }
        return null
    }

    private fun writeAudioPassthrough(
            inputPath: String,
            trackIdx: Int,
            muxIdx: Int,
            muxer: MediaMuxer
    ) {
        val ext = MediaExtractor()
        try {
            ext.setDataSource(inputPath)
            ext.selectTrack(trackIdx)
            val buf = ByteBuffer.allocateDirect(256 * 1024)
            val info = MediaCodec.BufferInfo()
            var firstPts = -1L
            var n = 0
            // Audio in a naively-concatenated TS also restarts its PTS at each
            // HLS segment boundary; re-base the timeline so audio stays
            // monotonic and aligned with the (already re-based) video track.
            var lastPts = Long.MIN_VALUE
            var ptsOffset = 0L
            while (true) {
                val sz = ext.readSampleData(buf, 0)
                if (sz < 0) break
                info.offset = 0
                info.size = sz
                val rawPts = ext.sampleTime
                var adjustedPts = rawPts + ptsOffset
                if (adjustedPts < lastPts - PTS_REBASE_THRESHOLD_US) {
                    ptsOffset += (lastPts - adjustedPts) + 1L
                    adjustedPts = rawPts + ptsOffset
                }
                lastPts = adjustedPts
                info.presentationTimeUs = adjustedPts
                info.flags = ext.sampleFlags
                // Zero-base the audio timeline exactly like the video encoder
                // output, so A/V stays in sync regardless of the source PTS
                // origin (TS streams often start at arbitrary offsets).
                if (firstPts < 0) firstPts = info.presentationTimeUs
                if (firstPts > 0) info.presentationTimeUs -= firstPts
                if (info.presentationTimeUs < 0) info.presentationTimeUs = 0
                muxer.writeSampleData(muxIdx, buf, info)
                n++
                ext.advance()
            }
            Log.i(TAG, "Audio passthrough: $n samples")
        } finally {
            ext.release()
        }
    }

    private fun selectCodec(
            mime: String,
            encoder: Boolean,
            hardwareOnly: Boolean
    ): MediaCodecInfo? {
        val selected = codecCandidates(mime, encoder, hardwareOnly).firstOrNull()
        if (selected != null) {
            Log.i(
                    TAG,
                    "Selected ${if (encoder) "encoder" else "decoder"} ${selected.name} for $mime"
            )
        }
        return selected
    }

    private fun codecCandidates(
            mime: String,
            encoder: Boolean,
            hardwareOnly: Boolean
    ): List<MediaCodecInfo> =
            MediaCodecList(MediaCodecList.REGULAR_CODECS)
                    .codecInfos
                    .filter { codec ->
                        codec.isEncoder == encoder &&
                                codec.supportedTypes.any { it.equals(mime, ignoreCase = true) } &&
                                (!hardwareOnly || !isSoftwareCodec(codec))
                    }
                    .sortedByDescending { scoreCodec(it) }

    private fun supportsSurfaceInput(codec: MediaCodecInfo, mime: String): Boolean =
            runCatching {
                        codec.supportedTypes.any { it.equals(mime, ignoreCase = true) } &&
                                codec.getCapabilitiesForType(mime)
                                        .colorFormats
                                        .contains(
                                                MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface
                                        )
                    }
                    .getOrDefault(false)

    private fun scoreCodec(codec: MediaCodecInfo): Int {
        val name = codec.name.lowercase()
        var score = 0
        if (!isSoftwareCodec(codec)) score += 100
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q && codec.isHardwareAccelerated)
                score += 60
        if (name.contains("qcom") || name.contains("qti")) score += 40
        if (name.contains("mtk") || name.contains("mediatek")) score += 38
        if (name.contains("hisi") || name.contains("kirin") || name.contains("huawei")) score += 36
        if (name.contains("exynos") || name.contains("sec")) score += 34
        if (name.startsWith("c2.") || name.startsWith("omx.")) score += 10
        if (name.contains("google") || name.contains("android") || name.contains("sw")) score -= 80
        return score
    }

    private fun isSoftwareCodec(codec: MediaCodecInfo): Boolean {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) return codec.isSoftwareOnly
        val name = codec.name.lowercase()
        return name.contains("google") ||
                name.contains("android") ||
                name.contains("ffmpeg") ||
                name.contains("sw") ||
                name.startsWith("c2.android") ||
                name.startsWith("omx.google")
    }

    private fun MediaFormat.safeInt(key: String, default: Int): Int =
            runCatching { if (containsKey(key)) getInteger(key) else default }.getOrDefault(default)
}
