/// Web (WASM) platform helpers.
///
/// `dart:io` does not exist on the web, so every helper returns the web
/// defaults. Selected by the conditional import in `platform_utils.dart` when
/// `dart.library.io` is unavailable.
//
// References come from the conditional import in `platform_utils.dart`, which
// the standalone analysis of this file cannot see, hence the ignore.
// ignore_for_file: unused_element
bool ferrisHostIsAndroid() => false;
bool ferrisHostIsIOS() => false;
bool ferrisHostIsWindows() => false;
bool ferrisHostIsMacOS() => false;
bool ferrisHostIsLinux() => false;
String ferrisHostOperatingSystem() => 'web';
String ferrisHostPathSeparator() => '/';
