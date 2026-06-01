# Keep MediaTranscoder class for JNI access from Rust
-keep class com.bluevale.m3u8_downloader.MediaTranscoder {
    public static *;
}

# Keep the transcode method specifically
-keepclassmembers class com.bluevale.m3u8_downloader.MediaTranscoder {
    public static boolean transcode(java.lang.String, java.lang.String, int, int);
    public static boolean mux(java.lang.String, java.lang.String, java.lang.String);
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
