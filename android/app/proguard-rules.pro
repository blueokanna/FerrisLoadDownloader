# Keep MediaTranscoder class for JNI access from Rust
-keep class com.bluevale.m3u8_downloader.MediaTranscoder {
    public static *;
}

# Keep the transcode / mux / capabilityReport methods specifically with their
# exact JNI-visible signatures (must match the Rust JNI call signatures):
#   transcode(String input, String output, int vBitrate, int aBitrate, long expectedDurationMs) -> boolean
#   transcodeDir(String dir, String prefix, int total, String output, int vBitrate, int aBitrate, long expectedDurationMs) -> boolean
#   mux(String video, String audio, String output, long expectedDurationMs) -> boolean
#   muxDirs(String videoDir, String videoPrefix, int videoTotal, String audioDir, String audioPrefix, int audioTotal, String output, long expectedDurationMs) -> boolean
#   capabilityReport() -> String
-keepclassmembers class com.bluevale.m3u8_downloader.MediaTranscoder {
    public static boolean transcode(java.lang.String, java.lang.String, int, int, long);
    public static boolean transcodeDir(java.lang.String, java.lang.String, int, java.lang.String, int, int, long);
    public static boolean mux(java.lang.String, java.lang.String, java.lang.String, long);
    public static boolean muxDirs(java.lang.String, java.lang.String, int, java.lang.String, java.lang.String, int, java.lang.String, long);
    public static java.lang.String capabilityReport();
}

# Keep MainActivity (loads native Rust library)
-keep class com.bluevale.m3u8_downloader.MainActivity { *; }

# Keep all JNI / native methods
-keepclasseswithmembernames class * {
    native <methods>;
}

# flutter_rust_bridge generated code
-keep class com.aspect.** { *; }
-keep class ** extends io.flutter.embedding.engine.FlutterJNI { *; }

# Keep DownloadForegroundService
-keep class com.bluevale.m3u8_downloader.DownloadForegroundService { *; }

# Keep MediaStoreHelper
-keep class com.bluevale.m3u8_downloader.MediaStoreHelper { *; }

# Keep Flutter embedding
-keep class io.flutter.** { *; }
-keep class io.flutter.plugins.** { *; }

# Keep AndroidX core used by NotificationCompat
-keep class androidx.core.app.** { *; }

# Suppress R8 warnings for Google Play Core classes referenced by Flutter
# (not needed at runtime unless using deferred components)
-dontwarn com.google.android.play.core.splitcompat.SplitCompatApplication
-dontwarn com.google.android.play.core.splitinstall.**
-dontwarn com.google.android.play.core.tasks.**
