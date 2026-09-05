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

    private const val MAX_STALL_MS = 30_000L // 30s stall → abort

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

            // 1) Try a lossless stream-copy remux first whenever no re-encode
            //    is requested. MediaMuxer (API 25+) writes a proper ctts table
            //    when B-frame samples are fed in decode order with their REAL
            //    PTS, so even B-frame content is remuxed at disk speed
            //    (seconds for a full movie) instead of being re-encoded. Only
            //    a pre-API-25 device cannot represent B-frames losslessly;
            //    that (and any remux failure caught by the checks below)
            //    falls through to the hardware pipeline.
            val hasBframes = videoNeedsReencode(inputPath)
            val canMuxBframes = Build.VERSION.SDK_INT >= Build.VERSION_CODES.N_MR1
            if (vBitrate <= 0 && aBitrate <= 0 && (!hasBframes || canMuxBframes)) {
                val remuxOk = tryRemux(inputPath, outputPath, hasBframes && canMuxBframes)
                if (remuxOk &&
                                verifyOutput(outputPath) &&
                                verifyDuration(outputPath, expectedDurationMs, "remux")
                ) {
                    Log.i(TAG, "✅ Remux succeeded: ${File(outputPath).length()} bytes")
                    return true
                }
                // Remux failed/empty/truncated; clean up and fall through.
                File(outputPath).delete()
                Log.w(
                        TAG,
                        "Remux failed, empty or truncated output, falling back to hardware transcode"
                )
            }

            // 2) Hardware transcode.
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
            // MediaMuxer (API 25+) can stream-copy B-frame H.264 into MP4 when
            // samples arrive in decode order with their REAL PTS (it writes a
            // ctts table), so keep the B-frames and make the merge lossless at
            // disk speed. The audio track is always flattened because the MP4
            // writer requires strictly monotonic audio PTS.
            val videoHasBframes = videoNeedsReencode(videoPath)
            val canMuxBframes = Build.VERSION.SDK_INT >= Build.VERSION_CODES.N_MR1
            val videoSamples =
                    writeTrackSamples(
                            videoPath,
                            videoTrack.index,
                            muxVideoIndex,
                            muxer,
                            video = true,
                            preserveBframes = videoHasBframes && canMuxBframes
                    )
            val audioSamples =
                    writeTrackSamples(
                            audioPath,
                            audioTrack.index,
                            muxAudioIndex,
                            muxer,
                            video = false,
                            preserveBframes = false
                    )
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

        // 1) Stream-copy remux of every segment. MediaMuxer (API 25+) writes a
        //    proper ctts table when the video samples are fed in decode order
        //    with their REAL (B-frame swinging) PTS, so even B-frame content
        //    can be remuxed losslessly and at disk speed. Only when that is
        //    impossible (old API, or the muxer rejects the stream) do we fall
        //    back to hardware re-encoding.
        val hasBframes = segmentVideoNeedsReencode(segmentDir, prefix, total)
        val canMuxBframes = Build.VERSION.SDK_INT >= Build.VERSION_CODES.N_MR1
        if (vBitrate <= 0 && aBitrate <= 0 && (!hasBframes || canMuxBframes)) {
            val remuxOk =
                    remuxSegments(
                            segmentDir,
                            prefix,
                            total,
                            outputPath,
                            allowBframes = hasBframes && canMuxBframes
                    )
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

        val hasBframes = segmentVideoNeedsReencode(videoDir, videoPrefix, videoTotal)
        if (hasBframes && Build.VERSION.SDK_INT < Build.VERSION_CODES.N_MR1) {
            Log.w(
                    TAG,
                    "muxDirs: video stream uses B-frames and the platform is pre-API-25; " +
                            "stream copy is not possible, returning failure"
            )
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
                            outputPath,
                            allowBframes = hasBframes
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
            // A constant frame rate only needs a short sampling window;
            // scanning a whole long input just to measure fps wastes I/O.
            while (frames < 900) {
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
     * Maps decode-order source PTS into a continuous feed timeline.
     *
     * A B-frame stream stores samples in decode order, so consecutive
     * sampleTime values legitimately step backward inside each GOP. The
     * decoder reorders to presentation order by itself, and it needs every
     * frame's REAL PTS to do that; forcing a synthetic monotonic timeline
     * makes the reordered Surface timestamps go backward and GraphicBufferSource
     * drops the frames (the "going backward in time" flood). Only a large
     * backward jump (> 500 ms) means a fresh segment whose PTS restarted;
     * that segment is lifted so the overall timeline stays continuous.
     */
    private class DecoderPtsTimeline {
        private var offsetUs = 0L
        private var maxFedUs = Long.MIN_VALUE

        fun next(rawUs: Long): Long {
            val raw = if (rawUs < 0) 0L else rawUs
            if (maxFedUs != Long.MIN_VALUE &&
                    raw + offsetUs < maxFedUs - SEGMENT_RESET_US
            ) {
                offsetUs = maxFedUs - raw
            }
            val fed = raw + offsetUs
            if (fed > maxFedUs) maxFedUs = fed
            return fed
        }

        companion object {
            private const val SEGMENT_RESET_US = 500_000L
        }
    }

    /**
     * True when the video stream in `path` uses B-frame reordering (a
     * backward PTS step between consecutive decode-order samples).
     *
     * A B-frame stream cannot be remuxed by simply flattening its PTS to a
     * monotonic timeline — that is exactly what makes MediaMuxer drop samples
     * or inflate the duration. Two correct options exist: on API 25+ the
     * stream is still copied losslessly when its REAL decode-order PTS is
     * preserved (MediaMuxer then writes a ctts table — see
     * [writeTrackSamples] with `preserveBframes = true`); on older platforms
     * no lossless representation is possible and the stream must be
     * re-encoded B-frame-free instead. Callers combine this probe with the
     * platform check to choose which path applies.
     */
    private fun videoNeedsReencode(path: String): Boolean {
        val extractor = MediaExtractor()
        try {
            extractor.setDataSource(path)
            val videoIdx = findTrackIdx(extractor, true)
            if (videoIdx < 0) return false
            extractor.selectTrack(videoIdx)
            var previousPts = Long.MIN_VALUE
            var seen = 0
            // Walk decode order only (no sample data is copied). 20k samples
            // (~11 min at 30 fps) is far beyond where B-frames can first appear.
            while (seen < 20_000) {
                val pts = extractor.sampleTime
                if (seen > 0 && pts < previousPts) {
                    Log.i(TAG, "videoNeedsReencode: B-frame reorder at sample $seen")
                    return true
                }
                previousPts = pts
                seen++
                if (!extractor.advance()) break
            }
            return false
        } catch (e: Exception) {
            Log.w(TAG, "videoNeedsReencode probe failed: ${e.message}")
            return false
        } finally {
            extractor.release()
        }
    }

    /**
     * Runs [videoNeedsReencode] over every segment. B-frames may start well
     * after the first few segments; a mid-stream remux failure would waste an
     * entire copy pass before falling back, so probe the whole set up front.
     * The probe only walks sample timestamps (no data copy), which is cheap.
     */
    private fun segmentVideoNeedsReencode(dir: String, prefix: String, total: Int): Boolean {
        for (i in 0 until total) {
            val path = segmentPath(dir, prefix, i)
            if (!File(path).exists()) continue
            if (videoNeedsReencode(path)) return true
        }
        return false
    }

    /**
     * Stream-copy remux of one stream across all segments into `muxer`'s `dstIdx` track. Every
     * segment is opened with its own extractor and the PTS timeline is re-based continuously across
     * segment boundaries, so a per-segment PTS reset can never make the output non-monotonic. A
     * corrupt/unreadable segment is skipped rather than aborting the whole download (the duration
     * check still catches severe truncation).
     *
     * When `preserveBframes` is true (video tracks that contain B-frames, API 25+), each sample is
     * fed with its REAL decode-order PTS (only whole segments whose PTS restarted are lifted).
     * MediaMuxer then writes a proper ctts table and the B-frames stay lossless — this is the
     * disk-speed path that avoids re-encoding entirely. Audio tracks are always flattened (their
     * PTS must be strictly monotonic for the MP4 writer).
     */
    private fun writeSegmentTrack(
            segmentDir: String,
            prefix: String,
            total: Int,
            video: Boolean,
            dstIdx: Int,
            muxer: MediaMuxer,
            preserveBframes: Boolean = false
    ): Int {
        val buf = ByteBuffer.allocateDirect(8 * 1024 * 1024)
        val info = MediaCodec.BufferInfo()
        var lastPts = Long.MIN_VALUE
        var ptsOffset = 0L
        var firstPts = -1L
        var samples = 0
        val bframeTimeline = DecoderPtsTimeline()
        // Map a source PTS to the PTS handed to the muxer. B-frame video keeps
        // its real (swinging) decode-order PTS so MediaMuxer can emit ctts;
        // everything else is clamped to a strictly monotonic timeline.
        val mapPts: (Long) -> Long =
                if (video && preserveBframes) {
                    { raw -> bframeTimeline.next(raw) }
                } else {
                    { raw ->
                        var adjusted = raw + ptsOffset
                        if (adjusted <= lastPts) {
                            ptsOffset += (lastPts - adjusted) + 1L
                            adjusted = raw + ptsOffset
                        }
                        lastPts = adjusted
                        adjusted
                    }
                }
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
                    val mappedPts = mapPts(ext.sampleTime)
                    info.presentationTimeUs = mappedPts
                    info.flags = ext.sampleFlags
                    // Zero-base each track like the encoder output so audio
                    // and video stay aligned regardless of source PTS origin.
                    if (firstPts < 0) firstPts = mappedPts
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
            outputPath: String,
            allowBframes: Boolean = false
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
                        writeSegmentTrack(
                                segmentDir,
                                prefix,
                                total,
                                true,
                                vOutIdx,
                                muxer,
                                preserveBframes = allowBframes
                        )
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
            outputPath: String,
            allowBframes: Boolean = false
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
                        writeSegmentTrack(
                                videoDir,
                                videoPrefix,
                                videoTotal,
                                true,
                                vOutIdx,
                                muxer,
                                preserveBframes = allowBframes
                        )
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
                        defaultVideoBitrate(w, h, fps)
                    }
            val encFmt = createAvcEncodeFormat(w, h, fps, targetBitrate)
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
            var segmentIndex = 0
            val ptsTimeline = DecoderPtsTimeline()

            val decInfo = MediaCodec.BufferInfo()
            val encInfo = MediaCodec.BufferInfo()

            // Main pump: each iteration feeds every free decoder input buffer
            // and drains every ready decoder/encoder output buffer, so the
            // hardware codecs stay saturated and the loop only blocks when
            // nothing is ready (no CPU spin). Feed PTS is the source PTS
            // unchanged: B-frame streams store samples in decode order whose
            // PTS steps backward inside each GOP, but the decoder reorders to
            // presentation order and needs the REAL PTS to do that. A
            // synthetic monotonic timeline made the reordered Surface
            // timestamps go backward, which GraphicBufferSource then dropped.
            while (!encoderDone) {
                var progressed = false

                // ── 1. Feed decoder across segments until input queue full ──
                if (!inputDone) {
                    while (true) {
                        val feedIdx = decoder.dequeueInputBuffer(0)
                        if (feedIdx < 0) break
                        progressed = true
                        var fed = false
                        while (!fed) {
                            if (currentExtractor == null) {
                                if (segmentIndex >= total) {
                                    decoder.queueInputBuffer(
                                            feedIdx,
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
                            val buf = decoder.getInputBuffer(feedIdx)!!
                            val sz = ext.readSampleData(buf, 0)
                            if (sz < 0) {
                                ext.release()
                                currentExtractor = null
                                continue // exhausted: continue with next segment
                            }
                            decoder.queueInputBuffer(
                                    feedIdx,
                                    0,
                                    sz,
                                    ptsTimeline.next(ext.sampleTime),
                                    0
                            )
                            ext.advance()
                            fed = true
                        }
                        if (inputDone) break
                    }
                    if (progressed) lastProgressMs = System.currentTimeMillis()
                }

                // ── 2. Drain every ready decoder output → Surface ──
                if (!decoderDone) {
                    while (true) {
                        val drainIdx = decoder.dequeueOutputBuffer(decInfo, 0)
                        if (drainIdx == MediaCodec.INFO_TRY_AGAIN_LATER) break
                        if (drainIdx == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) continue
                        if (drainIdx < 0) break
                        progressed = true
                        lastProgressMs = System.currentTimeMillis()
                        val eos = (decInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0
                        decoder.releaseOutputBuffer(drainIdx, !eos)
                        if (eos) {
                            decoderDone = true
                            encoder.signalEndOfInputStream()
                            break
                        }
                    }
                }

                // ── 3. Drain every ready encoder output → muxer ──
                while (true) {
                    val outIdx = encoder.dequeueOutputBuffer(encInfo, 0)
                    if (outIdx == MediaCodec.INFO_TRY_AGAIN_LATER) break
                    if (outIdx == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                        if (!muxerStarted) {
                            videoMuxIdx = muxer.addTrack(encoder.outputFormat)
                            muxer.start()
                            muxerStarted = true
                        }
                        continue
                    }
                    if (outIdx < 0) break
                    progressed = true
                    lastProgressMs = System.currentTimeMillis()
                    val data = encoder.getOutputBuffer(outIdx)!!
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
                    encoder.releaseOutputBuffer(outIdx, false)
                    if (encInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) {
                        encoderDone = true
                        break
                    }
                }

                // Nothing was ready this pass (codecs still busy): yield the
                // CPU briefly instead of spinning, then poll again.
                if (!progressed && !encoderDone) {
                    try {
                        Thread.sleep(1L)
                    } catch (ignored: InterruptedException) {
                        Thread.currentThread().interrupt()
                        return false
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
     * promised. Three failure shapes are caught:
     *
     *  1. Catastrophic truncation — Android's MediaExtractor can silently stop
     *     early when reading a naively-concatenated TS (timestamp
     *     discontinuities), which used to produce a "successful" output
     *     containing only the first few seconds after all download traffic was
     *     already spent.
     *  2. Timeline inflation — an encoder that re-times Surface input can
     *     stretch the output (the "30 min becomes 60 min" failure), which is
     *     just as wrong as a truncated one.
     *  3. Zero-duration output — a muxer that ended up with an empty (or all
     *     samples dropped) video track still produces an MP4 whose container
     *     reports 0:00. Such a file must never be delivered as a success.
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
        val minTolerance = (expectedMs * 80L / 100L).coerceAtLeast(expectedMs - 120_000L)
        val maxTolerance = expectedMs + maxOf(30_000L, expectedMs / 5L)

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

        val referenceMs = if (actualMs > 0) actualMs else probeDurationMs(path)
        if (referenceMs <= 0) {
            // The container carries no readable timeline. Accepting it would
            // deliver a 0:00 file as success, so treat it as a failed output.
            Log.e(
                    TAG,
                    "$what duration check failed: no usable duration in output (expected≈${expectedMs}ms)"
            )
            return false
        }
        return durationWithinBounds(referenceMs, minTolerance, maxTolerance, expectedMs, what)
    }

    /**
     * Secondary duration source when MediaMetadataRetriever reports nothing.
     * Prefers the per-track KEY_DURATION metadata that MediaExtractor exposes
     * for MP4 (cheap, no sample I/O); only if that is absent walks the last
     * samples to recover a timeline. Returns ms, or -1 for an unreadable file.
     */
    private fun probeDurationMs(path: String): Long {
        val executor = java.util.concurrent.Executors.newSingleThreadExecutor()
        return try {
            val task =
                    executor.submit(
                            java.util.concurrent.Callable<Long> {
                                val extractor = MediaExtractor()
                                try {
                                    extractor.setDataSource(path)
                                    var longestUs = -1L
                                    var sawTrack = false
                                    for (i in 0 until extractor.trackCount) {
                                        sawTrack = true
                                        val format = extractor.getTrackFormat(i)
                                        if (format.containsKey(MediaFormat.KEY_DURATION)) {
                                            val durationUs =
                                                    runCatching {
                                                                format.getLong(
                                                                        MediaFormat.KEY_DURATION
                                                                )
                                                            }
                                                            .getOrDefault(-1L)
                                            if (durationUs > longestUs) longestUs = durationUs
                                        }
                                    }
                                    if (longestUs <= 0 && sawTrack) {
                                        // No track durations in the container;
                                        // recover the timeline by walking samples
                                        // (decode order, without copying data).
                                        for (i in 0 until extractor.trackCount) {
                                            extractor.selectTrack(i)
                                            var lastPts = Long.MIN_VALUE
                                            while (true) {
                                                val pts = extractor.sampleTime
                                                if (pts > lastPts) lastPts = pts
                                                if (!extractor.advance()) break
                                            }
                                            extractor.unselectTrack(i)
                                            if (lastPts > longestUs) longestUs = lastPts
                                        }
                                    }
                                    if (longestUs <= 0) -1L else (longestUs / 1000L).coerceAtLeast(1L)
                                } catch (error: Exception) {
                                    Log.e(TAG, "probeDurationMs failed: ${error.message}", error)
                                    -1L
                                } finally {
                                    extractor.release()
                                }
                            }
                    )
            task.get(20, java.util.concurrent.TimeUnit.SECONDS)
        } catch (error: Exception) {
            Log.e(TAG, "probeDurationMs timed out or failed: ${error.message}", error)
            -1L
        } finally {
            executor.shutdownNow()
        }
    }

    private fun durationWithinBounds(
            actualMs: Long,
            minTolerance: Long,
            maxTolerance: Long,
            expectedMs: Long,
            what: String
    ): Boolean {
        if (actualMs + 1000L < minTolerance) {
            Log.e(TAG, "$what duration check failed: expected≈${expectedMs}ms got ${actualMs}ms")
            return false
        }
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
            muxer: MediaMuxer,
            video: Boolean,
            preserveBframes: Boolean
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
            // Map a source PTS to the PTS handed to the muxer. B-frame video
            // keeps its real (swinging) decode-order PTS so MediaMuxer (API
            // 25+) writes a proper ctts table and the stream is copied
            // losslessly; every backward step of more than 500 ms is a fresh
            // segment/timeline whose PTS restarted, and is lifted so the whole
            // output stays continuous. Everything else (audio, and video that
            // MediaMuxer must flatten) is clamped to a strictly monotonic
            // timeline, which the MP4 writer requires.
            val bframeTimeline = DecoderPtsTimeline()
            var lastPts = Long.MIN_VALUE
            var ptsOffset = 0L
            val mapPts: (Long) -> Long =
                    if (video && preserveBframes) {
                        { raw -> bframeTimeline.next(raw) }
                    } else {
                        { raw ->
                            var adjusted = raw + ptsOffset
                            if (adjusted <= lastPts) {
                                ptsOffset += (lastPts - adjusted) + 1L
                                adjusted = raw + ptsOffset
                            }
                            lastPts = adjusted
                            adjusted
                        }
                    }
            while (true) {
                val size = extractor.readSampleData(buffer, 0)
                if (size < 0) break
                info.offset = 0
                info.size = size
                var adjustedPts = mapPts(extractor.sampleTime)
                info.flags = extractor.sampleFlags
                if (firstPts < 0) firstPts = adjustedPts
                if (firstPts > 0) adjustedPts -= firstPts
                if (adjustedPts < 0) adjustedPts = 0
                info.presentationTimeUs = adjustedPts
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

    private fun tryRemux(
            inputPath: String,
            outputPath: String,
            allowBframes: Boolean = false
    ): Boolean {
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
            remuxTracks(inputPath, outputPath, videoIdx, audioIdx, allowBframes)
            return true
        } catch (e: Exception) {
            Log.w(TAG, "Remux exception: ${e.message}", e)
            return false
        } finally {
            runCatching { extractor?.release() }
        }
    }

    @Throws(IOException::class)
    private fun remuxTracks(
            inputPath: String,
            outputPath: String,
            videoIdx: Int,
            audioIdx: Int,
            allowBframes: Boolean = false
    ) {
        val muxer = MediaMuxer(outputPath, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)
        try {
            val setup = MediaExtractor().also { it.setDataSource(inputPath) }
            val vOutIdx = muxer.addTrack(setup.getTrackFormat(videoIdx))
            val aOutIdx = if (audioIdx >= 0) muxer.addTrack(setup.getTrackFormat(audioIdx)) else -1
            setup.release()

            muxer.start()

            // Write each track with its own extractor to avoid PTS interleaving.
            // Video keeps its B-frames when requested (real decode-order PTS →
            // MediaMuxer writes ctts); audio is always flattened.
            val videoSamples =
                    writeTrackSamples(
                            inputPath,
                            videoIdx,
                            vOutIdx,
                            muxer,
                            video = true,
                            preserveBframes = allowBframes
                    )
            var audioSamples = 0
            if (audioIdx >= 0 && aOutIdx >= 0) {
                audioSamples =
                        writeTrackSamples(
                                inputPath,
                                audioIdx,
                                aOutIdx,
                                muxer,
                                video = false,
                                preserveBframes = false
                        )
            }
            muxer.stop()
            Log.i(TAG, "Remux tracks: video=$videoSamples audio=$audioSamples")
        } finally {
            runCatching { muxer.release() }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  HARDWARE TRANSCODE - Surface mode: Decoder -> Surface -> Encoder
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

            // ── Encoder ──
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
                        defaultVideoBitrate(w, h, fps)
                    }
            val encFmt = createAvcEncodeFormat(w, h, fps, targetBitrate)
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
            val ptsTimeline = DecoderPtsTimeline()

            val decInfo = MediaCodec.BufferInfo()
            val encInfo = MediaCodec.BufferInfo()

            while (!encoderDone) {
                var progressed = false

                // ── 1. Feed decoder until input queue is full ──
                if (!inputDone) {
                    while (true) {
                        val feedIdx = decoder.dequeueInputBuffer(0)
                        if (feedIdx < 0) break
                        progressed = true
                        lastProgressMs = System.currentTimeMillis()
                        val buf = decoder.getInputBuffer(feedIdx)!!
                        val sz = extractor.readSampleData(buf, 0)
                        if (sz < 0) {
                            decoder.queueInputBuffer(
                                    feedIdx,
                                    0,
                                    0,
                                    0,
                                    MediaCodec.BUFFER_FLAG_END_OF_STREAM
                            )
                            inputDone = true
                            break
                        }
                        decoder.queueInputBuffer(
                                feedIdx,
                                0,
                                sz,
                                ptsTimeline.next(extractor.sampleTime),
                                0
                        )
                        extractor.advance()
                    }
                }

                // ── 2. Drain every ready decoder output → Surface ──
                if (!decoderDone) {
                    while (true) {
                        val drainIdx = decoder.dequeueOutputBuffer(decInfo, 0)
                        if (drainIdx == MediaCodec.INFO_TRY_AGAIN_LATER) break
                        if (drainIdx == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) continue
                        if (drainIdx < 0) break
                        progressed = true
                        lastProgressMs = System.currentTimeMillis()
                        val eos = (decInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0
                        decoder.releaseOutputBuffer(drainIdx, !eos)
                        if (eos) {
                            decoderDone = true
                            encoder.signalEndOfInputStream()
                            break
                        }
                    }
                }

                // ── 3. Drain every ready encoder output → muxer ──
                while (true) {
                    val outIdx = encoder.dequeueOutputBuffer(encInfo, 0)
                    if (outIdx == MediaCodec.INFO_TRY_AGAIN_LATER) break
                    if (outIdx == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                        if (!muxerStarted) {
                            videoMuxIdx = muxer.addTrack(encoder.outputFormat)
                            muxer.start()
                            muxerStarted = true
                        }
                        continue
                    }
                    if (outIdx < 0) break
                    progressed = true
                    lastProgressMs = System.currentTimeMillis()
                    val data = encoder.getOutputBuffer(outIdx)!!
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
                    encoder.releaseOutputBuffer(outIdx, false)
                    if (encInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) {
                        encoderDone = true
                        break
                    }
                }

                // Nothing was ready this pass (codecs still busy): yield the
                // CPU briefly instead of spinning, then poll again.
                if (!progressed && !encoderDone) {
                    try {
                        Thread.sleep(1L)
                    } catch (ignored: InterruptedException) {
                        Thread.currentThread().interrupt()
                        return false
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
     * Resolution-aware default bitrate (~0.10 bits per pixel per frame).
     * Keeps re-encoded H.264 crisp while staying in the encoder's range.
     */
    private fun defaultVideoBitrate(width: Int, height: Int, fps: Int): Int {
        val estimated = width.toLong() * height * fps.coerceAtLeast(1) * 10L / 100L
        return estimated.coerceIn(800_000L, 16_000_000L).toInt()
    }

    /**
     * AVC encoder format for Surface input. VBR keeps quality high at the
     * same average bitrate; no B-frames keeps the output timeline monotonic
     * and easy to decode. KEY_PRIORITY and KEY_OPERATING_RATE are both
     * deliberately unset: this is a batch/offline transcode fed as fast as the
     * disk allows, so hinting a realtime input rate (or realtime priority)
     * makes some vendor encoders pace themselves against the wall clock and
     * the transcode crawls. Frame timing is fully owned by the PTS we feed.
     *
     * KEY_I_FRAME_INTERVAL is 2 s instead of 1 s: an I-frame costs roughly an
     * order of magnitude more to encode than a P-frame, so halving the forced
     * keyframe rate meaningfully speeds up the encode (and shrinks the file)
     * while keeping seeking snappy. HLS sources usually carry 2-6 s GOPs, so
     * 2 s also avoids re-inserting extra keyframes the source never had.
     */
    private fun createAvcEncodeFormat(
            width: Int,
            height: Int,
            fps: Int,
            bitrate: Int
    ): MediaFormat =
            MediaFormat.createVideoFormat("video/avc", width, height).apply {
                setInteger(
                        MediaFormat.KEY_COLOR_FORMAT,
                        MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface
                )
                setInteger(MediaFormat.KEY_BIT_RATE, bitrate)
                setInteger(MediaFormat.KEY_FRAME_RATE, fps)
                setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 2)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.LOLLIPOP) {
                    setInteger(
                            MediaFormat.KEY_BITRATE_MODE,
                            MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_VBR
                    )
                }
                setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
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
                // MediaMuxer REQUIRES per-track monotonic PTS.
                if (adjustedPts <= lastPts) {
                    ptsOffset += (lastPts - adjustedPts) + 1L
                    adjustedPts = rawPts + ptsOffset
                }
                lastPts = adjustedPts
                info.presentationTimeUs = adjustedPts
                info.flags = ext.sampleFlags

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
