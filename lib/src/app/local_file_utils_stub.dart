/// Web local-file helpers (safe no-ops — the browser has no filesystem).
///
/// Selected by the conditional import in `local_file_utils.dart` on the web.
//
// References come from the conditional import in `local_file_utils.dart`; see
// `platform_utils_stub.dart` for why this ignore is required.
// ignore_for_file: unused_element
Future<bool> fileExists(String path) async => false;

Future<int> fileLength(String path) async => 0;

Future<void> deleteFile(String path) async {}
