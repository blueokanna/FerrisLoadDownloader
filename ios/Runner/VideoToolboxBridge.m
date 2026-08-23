//
//  VideoToolboxBridge.m
//  Runner
//
//  Native iOS transcoder used by the Rust download engine.
//
//  Design notes
//  ------------
//  * AVAssetWriter + AVVideoCodecTypeH264 transparently uses the hardware
//    H.264 encoder (VideoToolbox) on all Apple devices (A-series and M-series
//    chips). There is no "software H.264" fallback on iOS, so this is the
//    correct way to hit the hardware codec.
//  * Everything runs synchronously on the caller's (background) thread. The
//    Rust side already bounds the whole operation with a wall-clock timeout.
//  * Audio is passed through when it is AAC (the only audio format MP4 muxes
//    natively without re-encoding); anything else is re-encoded to AAC.
//  * The produced file is validated against `expected_ms` so a truncated
//    output (a failure mode of the older byte-concatenation approach) is
//    detected and surfaced as an error instead of a silent bad file.
//

#import "VideoToolboxBridge.h"

@import Foundation;
@import AVFoundation;
@import CoreMedia;
@import CoreVideo;
@import VideoToolbox;

static BOOL ferris_write_error(char *errbuf, size_t errbuf_len, NSString *message) {
    if (errbuf != NULL && errbuf_len > 0) {
        const char *utf8 = [message UTF8String];
        snprintf(errbuf, errbuf_len, "%s", utf8 != NULL ? utf8 : "unknown error");
    }
    return NO;
}

static NSString *ferris_error_desc(NSError *error, NSString *fallback) {
    if (error != nil && error.localizedDescription.length > 0) {
        return error.localizedDescription;
    }
    return fallback;
}

/// Blocks until the asset tracks are loaded (bounded by `timeout`).
static BOOL ferris_wait_for_tracks(AVURLAsset *asset, NSTimeInterval timeout) {
    dispatch_semaphore_t semaphore = dispatch_semaphore_create(0);
    __block BOOL finished = NO;
    [asset loadValuesAsynchronouslyForKeys:@[ @"tracks" ]
                         completionHandler:^{
                           finished = YES;
                           dispatch_semaphore_signal(semaphore);
                         }];
    dispatch_time_t deadline = dispatch_time(DISPATCH_TIME_NOW, (int64_t)(timeout * NSEC_PER_SEC));
    dispatch_semaphore_wait(semaphore, deadline);
    return finished;
}

/// Whether the audio track is AAC (safe to pass through into MP4).
static BOOL ferris_is_aac_track(AVAssetTrack *audioTrack) {
    NSArray<id> *formats = audioTrack.formatDescriptions;
    if (formats.count == 0) {
        return NO;
    }
    CMFormatDescriptionRef description = (__bridge CMFormatDescriptionRef)formats[0];
    FourCharCode subtype = CMFormatDescriptionGetMediaSubType(description);
    // `kAudioFormatMPEG4AAC` == 'mp4a'; use the literal to avoid depending on
    // the CoreAudioTypes module in every SDK.
    return subtype == 'mp4a';
}

/// Build the H.264 hardware encode settings for a track.
static NSDictionary *ferris_h264_settings(CGSize size, int videoBitrateKbps) {
    NSInteger width = ((NSInteger)size.width / 2) * 2;
    NSInteger height = ((NSInteger)size.height / 2) * 2;
    if (width <= 0) width = 1280;
    if (height <= 0) height = 720;

    NSMutableDictionary *settings = [NSMutableDictionary dictionary];
    settings[AVVideoCodecKey] = AVVideoCodecTypeH264;
    settings[AVVideoWidthKey] = @(width);
    settings[AVVideoHeightKey] = @(height);
    if (videoBitrateKbps > 0) {
        settings[AVVideoAverageBitRateKey] = @((NSUInteger)videoBitrateKbps * 1000);
    }
    settings[AVVideoProfileLevelKey] = AVVideoProfileLevelH264HighAutoLevel;
    settings[AVVideoAllowFrameReorderingKey] = @NO;
    settings[AVVideoCompressionPropertiesKey] = @{
        AVVideoRealTimeKey : @YES,
        AVVideoAverageNonDroppableFrameRateKey : @30,
    };
    return settings;
}

/// Drain one reader output into one writer input until the track ends.
static BOOL ferris_drain(AVAssetReaderTrackOutput *output,
                         AVAssetWriterInput *input,
                         AVAssetReader *reader,
                         AVAssetWriter *writer,
                         char *errbuf,
                         size_t errbuf_len) {
    while (input.readyForMoreMediaData) {
        if (reader.status == AVAssetReaderStatusFailed) {
            return ferris_write_error(errbuf, errbuf_len,
                                      ferris_error_desc(reader.error, @"Reader failed during drain"));
        }
        if (writer.status == AVAssetWriterStatusFailed) {
            return ferris_write_error(errbuf, errbuf_len,
                                      ferris_error_desc(writer.error, @"Writer failed during drain"));
        }
        CMSampleBufferRef sample = [output copyNextSampleBuffer];
        if (sample == NULL) {
            break;
        }
        if (![input appendSampleBuffer:sample]) {
            CFRelease(sample);
            return ferris_write_error(errbuf, errbuf_len,
                                      ferris_error_desc(writer.error, @"Sample append failed"));
        }
        CFRelease(sample);
    }
    [input markAsFinished];
    return YES;
}

/// Finalize the writer and validate the output duration.
static BOOL ferris_finish_writer(AVAssetWriter *writer,
                                 long long expected_ms,
                                 char *errbuf,
                                 size_t errbuf_len) {
    dispatch_semaphore_t semaphore = dispatch_semaphore_create(0);
    [writer finishWritingWithCompletionHandler:^{
      dispatch_semaphore_signal(semaphore);
    }];
    dispatch_time_t deadline = dispatch_time(DISPATCH_TIME_NOW, (int64_t)(300 * NSEC_PER_SEC));
    dispatch_semaphore_wait(semaphore, deadline);

    if (writer.status != AVAssetWriterStatusCompleted) {
        return ferris_write_error(errbuf, errbuf_len,
                                  ferris_error_desc(writer.error, @"Writer did not complete"));
    }
    if (expected_ms > 0) {
        double seconds = CMTimeGetSeconds(writer.asset.duration);
        if (isfinite(seconds) && seconds > 0.0) {
            double expected = expected_ms / 1000.0;
            double tolerance = MAX(expected * 0.85, expected - 5.0);
            if (seconds + 1.0 < tolerance) {
                return ferris_write_error(
                    errbuf, errbuf_len,
                    [NSString stringWithFormat:@"Output truncated: expected ~%.1fs but produced %.1fs",
                                               expected, seconds]);
            }
        }
    }
    return YES;
}

static BOOL ferris_transcode_impl(NSString *inputPath,
                                  NSString *outputPath,
                                  int videoBitrate,
                                  int audioBitrate,
                                  long long expectedMs,
                                  char *errbuf,
                                  size_t errbufLen) {
    [[NSFileManager defaultManager] removeItemAtPath:outputPath error:NULL];

    NSError *error = nil;
    AVURLAsset *asset = [AVURLAsset URLAssetWithURL:[NSURL fileURLWithPath:inputPath] options:nil];
    if (!ferris_wait_for_tracks(asset, 30.0)) {
        return ferris_write_error(errbuf, errbufLen, @"Timed out loading asset tracks");
    }

    AVAssetTrack *videoTrack = nil;
    AVAssetTrack *audioTrack = nil;
    for (AVAssetTrack *track in asset.tracks) {
        if ([track.mediaType isEqualToString:AVMediaTypeVideo] && videoTrack == nil) {
            videoTrack = track;
        } else if ([track.mediaType isEqualToString:AVMediaTypeAudio] && audioTrack == nil) {
            audioTrack = track;
        }
    }
    if (videoTrack == nil) {
        return ferris_write_error(errbuf, errbufLen, @"No video track found in input");
    }

    AVAssetReader *reader = [AVAssetReader assetReaderWithAsset:asset error:&error];
    if (reader == nil) {
        return ferris_write_error(errbuf, errbufLen,
                                  [NSString stringWithFormat:@"Cannot read input: %@",
                                                             ferris_error_desc(error, @"unknown")]);
    }

    NSDictionary *videoOutputSettings = @{
        (id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
    };
    AVAssetReaderTrackOutput *videoOutput =
        [AVAssetReaderTrackOutput assetReaderTrackOutputWithTrack:videoTrack
                                                   outputSettings:videoOutputSettings];
    videoOutput.alwaysCopiesSampleData = NO;
    if (![reader canAddOutput:videoOutput]) {
        return ferris_write_error(errbuf, errbufLen, @"Cannot add video reader output");
    }
    [reader addOutput:videoOutput];

    AVAssetReaderTrackOutput *audioOutput = nil;
    if (audioTrack != nil) {
        // `nil` output settings => pass the compressed samples through unchanged.
        audioOutput = [AVAssetReaderTrackOutput assetReaderTrackOutputWithTrack:audioTrack
                                                                 outputSettings:nil];
        audioOutput.alwaysCopiesSampleData = NO;
        if ([reader canAddOutput:audioOutput]) {
            [reader addOutput:audioOutput];
        } else {
            audioOutput = nil;
        }
    }

    if (![reader startReading]) {
        return ferris_write_error(errbuf, errbufLen,
                                  [NSString stringWithFormat:@"Reader failed to start: %@",
                                                             ferris_error_desc(reader.error, @"unknown")]);
    }

    AVAssetWriter *writer =
        [AVAssetWriter assetWriterWithURL:[NSURL fileURLWithPath:outputPath]
                                 fileType:AVFileTypeMPEG4
                                    error:&error];
    if (writer == nil) {
        return ferris_write_error(errbuf, errbufLen,
                                  [NSString stringWithFormat:@"Cannot create writer: %@",
                                                             ferris_error_desc(error, @"unknown")]);
    }

    AVAssetWriterInput *videoInput =
        [AVAssetWriterInput assetWriterInputWithMediaType:AVMediaTypeVideo
                                           outputSettings:ferris_h264_settings(videoTrack.naturalSize, videoBitrate)];
    videoInput.expectsMediaDataInRealTime = NO;
    videoInput.transform = videoTrack.preferredTransform;  // preserve rotation
    if (![writer canAddInput:videoInput]) {
        return ferris_write_error(errbuf, errbufLen, @"Cannot add video writer input");
    }
    [writer addInput:videoInput];

    AVAssetWriterInput *audioInput = nil;
    if (audioOutput != nil && audioTrack != nil) {
        BOOL passthrough = audioBitrate <= 0 && ferris_is_aac_track(audioTrack);
        if (passthrough) {
            audioInput = [AVAssetWriterInput assetWriterInputWithMediaType:AVMediaTypeAudio
                                                            outputSettings:nil];
        } else {
            NSDictionary *audioSettings = @{
                AVFormatIDKey : @('mp4a'),
                AVEncoderBitRateKey : @((NSUInteger)MAX(audioBitrate, 0) * 1000),
                AVNumberOfChannelsKey : @2,
                AVSampleRateKey : @44100,
            };
            audioInput = [AVAssetWriterInput assetWriterInputWithMediaType:AVMediaTypeAudio
                                                            outputSettings:audioSettings];
        }
        audioInput.expectsMediaDataInRealTime = NO;
        if ([writer canAddInput:audioInput]) {
            [writer addInput:audioInput];
        } else {
            audioInput = nil;
        }
    }

    if (![writer startWriting]) {
        return ferris_write_error(errbuf, errbufLen,
                                  [NSString stringWithFormat:@"Writer failed to start: %@",
                                                             ferris_error_desc(writer.error, @"unknown")]);
    }
    [writer startSessionAtSourceTime:kCMTimeZero];

    // Feed video first, then audio. AVAssetWriter re-interleaves by PTS when
    // finalizing the MP4, so a sequential drain is safe.
    if (!ferris_drain(videoOutput, videoInput, reader, writer, errbuf, errbufLen)) {
        return NO;
    }
    if (audioOutput != nil && audioInput != nil) {
        if (!ferris_drain(audioOutput, audioInput, reader, writer, errbuf, errbufLen)) {
            return NO;
        }
    }

    if (reader.status == AVAssetReaderStatusReading) {
        [reader cancelReading];
    }
    return ferris_finish_writer(writer, expectedMs, errbuf, errbufLen);
}

static BOOL ferris_mux_impl(NSString *videoPath,
                            NSString *audioPath,
                            NSString *outputPath,
                            long long expectedMs,
                            char *errbuf,
                            size_t errbufLen) {
    [[NSFileManager defaultManager] removeItemAtPath:outputPath error:NULL];

    NSError *error = nil;
    AVURLAsset *videoAsset = [AVURLAsset URLAssetWithURL:[NSURL fileURLWithPath:videoPath] options:nil];
    AVURLAsset *audioAsset = [AVURLAsset URLAssetWithURL:[NSURL fileURLWithPath:audioPath] options:nil];
    if (!ferris_wait_for_tracks(videoAsset, 30.0) || !ferris_wait_for_tracks(audioAsset, 30.0)) {
        return ferris_write_error(errbuf, errbufLen, @"Timed out loading tracks for mux");
    }

    AVAssetTrack *videoTrack = [videoAsset tracksWithMediaType:AVMediaTypeVideo].firstObject;
    AVAssetTrack *audioTrack = [audioAsset tracksWithMediaType:AVMediaTypeAudio].firstObject;
    if (videoTrack == nil) {
        return ferris_write_error(errbuf, errbufLen, @"No video track found for mux");
    }

    AVAssetReader *videoReader = [AVAssetReader assetReaderWithAsset:videoAsset error:&error];
    if (videoReader == nil) {
        return ferris_write_error(errbuf, errbufLen,
                                  [NSString stringWithFormat:@"Cannot read video: %@",
                                                             ferris_error_desc(error, @"unknown")]);
    }
    NSDictionary *videoOutputSettings = @{
        (id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange),
    };
    AVAssetReaderTrackOutput *videoOutput =
        [AVAssetReaderTrackOutput assetReaderTrackOutputWithTrack:videoTrack
                                                   outputSettings:videoOutputSettings];
    videoOutput.alwaysCopiesSampleData = NO;
    [videoReader addOutput:videoOutput];

    AVAssetReader *audioReader = nil;
    AVAssetReaderTrackOutput *audioOutput = nil;
    if (audioTrack != nil) {
        audioReader = [AVAssetReader assetReaderWithAsset:audioAsset error:&error];
        if (audioReader != nil) {
            audioOutput = [AVAssetReaderTrackOutput assetReaderTrackOutputWithTrack:audioTrack
                                                                     outputSettings:nil];
            audioOutput.alwaysCopiesSampleData = NO;
            if ([audioReader canAddOutput:audioOutput]) {
                [audioReader addOutput:audioOutput];
            } else {
                audioOutput = nil;
                audioReader = nil;
            }
        } else {
            audioOutput = nil;
        }
    }

    if (![videoReader startReading]) {
        return ferris_write_error(errbuf, errbufLen,
                                  [NSString stringWithFormat:@"Video reader failed to start: %@",
                                                             ferris_error_desc(videoReader.error, @"unknown")]);
    }
    if (audioReader != nil && ![audioReader startReading]) {
        return ferris_write_error(errbuf, errbufLen,
                                  [NSString stringWithFormat:@"Audio reader failed to start: %@",
                                                             ferris_error_desc(audioReader.error, @"unknown")]);
    }

    AVAssetWriter *writer =
        [AVAssetWriter assetWriterWithURL:[NSURL fileURLWithPath:outputPath]
                                 fileType:AVFileTypeMPEG4
                                    error:&error];
    if (writer == nil) {
        return ferris_write_error(errbuf, errbufLen,
                                  [NSString stringWithFormat:@"Cannot create mux writer: %@",
                                                             ferris_error_desc(error, @"unknown")]);
    }

    AVAssetWriterInput *videoInput =
        [AVAssetWriterInput assetWriterInputWithMediaType:AVMediaTypeVideo
                                           outputSettings:ferris_h264_settings(videoTrack.naturalSize, 0)];
    videoInput.expectsMediaDataInRealTime = NO;
    videoInput.transform = videoTrack.preferredTransform;
    if (![writer canAddInput:videoInput]) {
        return ferris_write_error(errbuf, errbufLen, @"Cannot add video writer input (mux)");
    }
    [writer addInput:videoInput];

    AVAssetWriterInput *audioInput = nil;
    if (audioOutput != nil && audioTrack != nil) {
        BOOL passthrough = ferris_is_aac_track(audioTrack);
        if (passthrough) {
            audioInput = [AVAssetWriterInput assetWriterInputWithMediaType:AVMediaTypeAudio
                                                            outputSettings:nil];
        } else {
            NSDictionary *audioSettings = @{
                AVFormatIDKey : @('mp4a'),
                AVNumberOfChannelsKey : @2,
                AVSampleRateKey : @44100,
            };
            audioInput = [AVAssetWriterInput assetWriterInputWithMediaType:AVMediaTypeAudio
                                                            outputSettings:audioSettings];
        }
        audioInput.expectsMediaDataInRealTime = NO;
        if ([writer canAddInput:audioInput]) {
            [writer addInput:audioInput];
        } else {
            audioInput = nil;
        }
    }

    if (![writer startWriting]) {
        return ferris_write_error(errbuf, errbufLen,
                                  [NSString stringWithFormat:@"Mux writer failed to start: %@",
                                                             ferris_error_desc(writer.error, @"unknown")]);
    }
    [writer startSessionAtSourceTime:kCMTimeZero];

    if (!ferris_drain(videoOutput, videoInput, videoReader, writer, errbuf, errbufLen)) {
        return NO;
    }
    if (audioOutput != nil && audioInput != nil && audioReader != nil) {
        if (!ferris_drain(audioOutput, audioInput, audioReader, writer, errbuf, errbufLen)) {
            return NO;
        }
    }

    if (videoReader.status == AVAssetReaderStatusReading) {
        [videoReader cancelReading];
    }
    if (audioReader != nil && audioReader.status == AVAssetReaderStatusReading) {
        [audioReader cancelReading];
    }
    return ferris_finish_writer(writer, expectedMs, errbuf, errbufLen);
}

#pragma mark - C entry points

int ferrisload_videotoolbox_available(void) {
    @autoreleasepool {
        // AVAssetWriter is backed by VideoToolbox on every iOS device;
        // H.264 is universally supported by the hardware encoder.
        return [AVAssetWriter isAvailable] ? 1 : 0;
    }
}

int ferrisload_videotoolbox_transcode(const char *input,
                                      const char *output,
                                      int video_bitrate,
                                      int audio_bitrate,
                                      long long expected_ms,
                                      char *errbuf,
                                      size_t errbuf_len) {
    if (input == NULL || output == NULL) {
        return ferris_write_error(errbuf, errbuf_len, @"Null input or output path");
    }
    @autoreleasepool {
        NSString *inputPath = [NSString stringWithUTF8String:input];
        NSString *outputPath = [NSString stringWithUTF8String:output];
        if (inputPath == nil || outputPath == nil) {
            return ferris_write_error(errbuf, errbuf_len, @"Paths are not valid UTF-8");
        }
        return ferris_transcode_impl(inputPath, outputPath, video_bitrate,
                                     audio_bitrate, expected_ms, errbuf, errbuf_len)
                   ? 1
                   : 0;
    }
}

int ferrisload_videotoolbox_mux(const char *video,
                                const char *audio,
                                const char *output,
                                long long expected_ms,
                                char *errbuf,
                                size_t errbuf_len) {
    if (video == NULL || audio == NULL || output == NULL) {
        return ferris_write_error(errbuf, errbuf_len, @"Null mux path");
    }
    @autoreleasepool {
        NSString *videoPath = [NSString stringWithUTF8String:video];
        NSString *audioPath = [NSString stringWithUTF8String:audio];
        NSString *outputPath = [NSString stringWithUTF8String:output];
        if (videoPath == nil || audioPath == nil || outputPath == nil) {
            return ferris_write_error(errbuf, errbuf_len, @"Mux paths are not valid UTF-8");
        }
        return ferris_mux_impl(videoPath, audioPath, outputPath, expected_ms, errbuf, errbuf_len)
                   ? 1
                   : 0;
    }
}
