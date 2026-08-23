import 'dart:io';

/// Native local-file helpers backed by `dart:io`.
///
/// Selected by the conditional import in `local_file_utils.dart` when
/// `dart.library.io` is available.
//
// References come from the conditional import in `local_file_utils.dart`; see
// `platform_utils_stub.dart` for why this ignore is required.
// ignore_for_file: unused_element
Future<bool> fileExists(String path) async => File(path).exists();

Future<int> fileLength(String path) async => File(path).length();

Future<void> deleteFile(String path) async {
  try {
    await File(path).delete();
  } catch (_) {
    // Best-effort cleanup; a missing file is not an error here.
  }
}
