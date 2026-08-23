import 'dart:io' show Platform;

/// Native (non-web) platform helpers backed by `dart:io`.
///
/// Selected by the conditional import in `platform_utils.dart` when
/// `dart.library.io` is available.
//
// References come from the conditional import in `platform_utils.dart`, which
// the standalone analysis of this file cannot see, hence the ignore.
// ignore_for_file: unused_element
bool ferrisHostIsAndroid() => Platform.isAndroid;
bool ferrisHostIsIOS() => Platform.isIOS;
bool ferrisHostIsWindows() => Platform.isWindows;
bool ferrisHostIsMacOS() => Platform.isMacOS;
bool ferrisHostIsLinux() => Platform.isLinux;
String ferrisHostOperatingSystem() => Platform.operatingSystem;
String ferrisHostPathSeparator() => Platform.pathSeparator;
