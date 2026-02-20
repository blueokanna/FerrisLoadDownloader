package com.bluevale.m3u8_downloader

import android.annotation.SuppressLint
import android.media.*
import android.util.Log
import java.io.File
import java.io.IOException
import java.nio.ByteBuffer

@SuppressLint("LogNotTimber")
object MediaTranscoder {
    private const val TAG = "MediaTranscoder"

    private const val TIMEOUT_US = 10_000L        // 10ms per poll
    private const val MAX_STALL_MS = 30_000L       // 30s stall → abort

    @JvmStatic
    fun transcode(
        inputPath: String?,
        outputPath: String?,
        vBitrate: Int,
        aBitrate: Int
    ): Boolean {
        if (inputPath == null || outputPath == null) {
            Log.e(TAG, "transcode: input or output is null")
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
                if (remuxOk && verifyOutput(outputPath)) {
                    Log.i(TAG, "✅ Remux succeeded: ${File(outputPath).length()} bytes")
                    return true
                }
                // remux 失败或产出空文件，清理后 fall through
                File(outputPath).delete()
                Log.w(TAG, "Remux failed or empty output, falling back to hardware transcode")
            }

            // 2) 硬件转码
            val hwOk = hardwareTranscode(inputPath, outputPath, vBitrate, aBitrate)
            if (hwOk && verifyOutput(outputPath)) {
                Log.i(TAG, "✅ Hardware transcode succeeded: ${File(outputPath).length()} bytes")
                return true
            }
            File(outputPath).delete()
            Log.e(TAG, "Hardware transcode failed or produced empty output")
            return false

        } catch (e: Exception) {
            Log.e(TAG, "transcode exception: ${e.message}", e)
            runCatching { File(outputPath).delete() }
            return false
        }
    }

    private fun verifyOutput(path: String): Boolean {
        val f = File(path)
        return f.exists() && f.length() > 1024 // 至少 1KB 才算有效
    }

    // ═══════════════════════════════════════════════════════════════════════
    //  REMUX — 纯转封装，不重新编码
    // ═══════════════════════════════════════════════════════════════════════

    private fun tryRemux(inputPath: String, outputPath: String): Boolean {
        var extractor: MediaExtractor? = null
        try {
            extractor = MediaExtractor().also { it.setDataSource(inputPath) }

            var videoIdx = -1; var audioIdx = -1
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
                if (aMime != "audio/mp4a-latm" && aMime != "audio/aac") audioIdx = -1
            }

            extractor.release(); extractor = null
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
    private fun remuxTracks(
        inputPath: String, outputPath: String,
        videoIdx: Int, audioIdx: Int
    ) {
        val muxer = MediaMuxer(outputPath, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)
        try {
            // 添加轨道
            val setup = MediaExtractor().also { it.setDataSource(inputPath) }
            val vOutIdx = muxer.addTrack(setup.getTrackFormat(videoIdx))
            val aOutIdx = if (audioIdx >= 0) muxer.addTrack(setup.getTrackFormat(audioIdx)) else -1
            setup.release()

            muxer.start()

            val buf = ByteBuffer.allocateDirect(1024 * 1024)
            val info = MediaCodec.BufferInfo()

            // 逐轨道写入，每个轨道独立 extractor，避免 PTS 交叉
            val pairs = mutableListOf(videoIdx to vOutIdx)
            if (audioIdx >= 0 && aOutIdx >= 0) pairs.add(audioIdx to aOutIdx)

            for ((srcIdx, dstIdx) in pairs) {
                val ext = MediaExtractor().also {
                    it.setDataSource(inputPath)
                    it.selectTrack(srcIdx)
                }
                var firstPts = -1L; var n = 0
                while (true) {
                    val sz = ext.readSampleData(buf, 0)
                    if (sz < 0) break
                    info.offset = 0; info.size = sz
                    info.presentationTimeUs = ext.sampleTime
                    info.flags = ext.sampleFlags
                    if (firstPts < 0) firstPts = info.presentationTimeUs
                    if (firstPts > 0) info.presentationTimeUs -= firstPts
                    if (info.presentationTimeUs < 0) info.presentationTimeUs = 0
                    muxer.writeSampleData(dstIdx, buf, info)
                    n++; ext.advance()
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
        inputPath: String, outputPath: String,
        vBitrate: Int, aBitrate: Int
    ): Boolean {
        val extractor = MediaExtractor()
        var muxer: MediaMuxer? = null
        var decoder: MediaCodec? = null
        var encoder: MediaCodec? = null

        try {
            extractor.setDataSource(inputPath)

            var videoIdx = -1; var audioIdx = -1
            for (i in 0 until extractor.trackCount) {
                val mime = extractor.getTrackFormat(i).getString(MediaFormat.KEY_MIME) ?: ""
                if (mime.startsWith("video/") && videoIdx < 0) videoIdx = i
                else if (mime.startsWith("audio/") && audioIdx < 0) audioIdx = i
            }
            if (videoIdx < 0) {
                Log.e(TAG, "No video track"); return false
            }

            val vFmt = extractor.getTrackFormat(videoIdx)
            val vMime = vFmt.getString(MediaFormat.KEY_MIME) ?: "video/avc"
            val w = vFmt.safeInt(MediaFormat.KEY_WIDTH, 1920)
            val h = vFmt.safeInt(MediaFormat.KEY_HEIGHT, 1080)
            val fps = vFmt.safeInt(MediaFormat.KEY_FRAME_RATE, 30)
            Log.i(TAG, "Input video: $vMime ${w}x${h}@${fps}fps")

            // ── 编码器 ──
            val targetBitrate = if (vBitrate > 0) vBitrate * 1000 else 2_500_000
            val encFmt = MediaFormat.createVideoFormat("video/avc", w, h).apply {
                setInteger(MediaFormat.KEY_COLOR_FORMAT,
                    MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
                setInteger(MediaFormat.KEY_BIT_RATE, targetBitrate)
                setInteger(MediaFormat.KEY_FRAME_RATE, fps)
                setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
            }
            encoder = MediaCodec.createEncoderByType("video/avc")
            encoder.configure(encFmt, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            val surface = encoder.createInputSurface()
            encoder.start()

            // ── 解码器 ──
            decoder = MediaCodec.createDecoderByType(vMime)
            decoder.configure(vFmt, surface, null, 0)
            decoder.start()

            // ── Muxer ──
            muxer = MediaMuxer(outputPath, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)

            // 音频 passthrough 准备
            var audioMuxIdx = -1
            if (audioIdx >= 0) {
                val aFmt = extractor.getTrackFormat(audioIdx)
                val aMime = aFmt.getString(MediaFormat.KEY_MIME) ?: ""
                if (aMime == "audio/mp4a-latm" || aMime == "audio/aac") {
                    audioMuxIdx = muxer.addTrack(aFmt)
                } else {
                    Log.w(TAG, "Skipping non-AAC audio: $aMime")
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
            var lastProgressMs = System.currentTimeMillis()

            val decInfo = MediaCodec.BufferInfo()
            val encInfo = MediaCodec.BufferInfo()

            while (!encoderDone) {
                // ── 1. Feed decoder ──
                if (!inputDone) {
                    val idx = decoder.dequeueInputBuffer(TIMEOUT_US)
                    if (idx >= 0) {
                        val buf = decoder.getInputBuffer(idx)!!
                        val sz = extractor.readSampleData(buf, 0)
                        if (sz < 0) {
                            decoder.queueInputBuffer(idx, 0, 0, 0,
                                MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                            inputDone = true
                        } else {
                            decoder.queueInputBuffer(idx, 0, sz, extractor.sampleTime, 0)
                            extractor.advance()
                        }
                    }
                }

                // ── 2. Drain decoder → Surface ──
                if (!decoderDone) {
                    val idx = decoder.dequeueOutputBuffer(decInfo, TIMEOUT_US)
                    if (idx >= 0) {
                        val eos = (decInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0
                        val render = !eos && decInfo.size > 0
                        decoder.releaseOutputBuffer(idx, render)
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

                // 防止无限卡死
                if (System.currentTimeMillis() - lastProgressMs > MAX_STALL_MS) {
                    Log.e(TAG, "Transcode stalled for ${MAX_STALL_MS}ms, aborting")
                    return false
                }
            }

            // ── 4. 音频 passthrough ──
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
            runCatching { decoder?.stop(); decoder?.release() }
            runCatching { encoder?.stop(); encoder?.release() }
            runCatching { muxer?.release() }
            extractor.release()
        }
    }

    private fun writeAudioPassthrough(
        inputPath: String, trackIdx: Int, muxIdx: Int, muxer: MediaMuxer
    ) {
        val ext = MediaExtractor()
        try {
            ext.setDataSource(inputPath)
            ext.selectTrack(trackIdx)
            val buf = ByteBuffer.allocateDirect(256 * 1024)
            val info = MediaCodec.BufferInfo()
            var firstPts = -1L; var n = 0
            while (true) {
                val sz = ext.readSampleData(buf, 0)
                if (sz < 0) break
                info.offset = 0; info.size = sz
                info.presentationTimeUs = ext.sampleTime
                info.flags = ext.sampleFlags
                if (firstPts < 0) firstPts = info.presentationTimeUs
                if (firstPts > 0) info.presentationTimeUs -= firstPts
                if (info.presentationTimeUs < 0) info.presentationTimeUs = 0
                muxer.writeSampleData(muxIdx, buf, info)
                n++; ext.advance()
            }
            Log.i(TAG, "Audio passthrough: $n samples")
        } finally {
            ext.release()
        }
    }

    private fun MediaFormat.safeInt(key: String, default: Int): Int =
        runCatching { if (containsKey(key)) getInteger(key) else default }.getOrDefault(default)
}
