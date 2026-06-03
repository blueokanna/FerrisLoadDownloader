import 'dart:io';

import 'package:flutter/services.dart';
import 'package:permission_handler/permission_handler.dart';

class MediaStoreBridge {
  static const MethodChannel _channel =
      MethodChannel('com.blue.ferrisload/media_store');

  static Future<void> requestPermissions() async {
    if (!Platform.isAndroid) {
      return;
    }
    await [Permission.storage, Permission.manageExternalStorage].request();
    await Permission.notification.request();
  }

  static Future<String> getAppPrivateDir() async {
    if (!Platform.isAndroid) {
      return '';
    }
    try {
      return await _channel.invokeMethod<String>('getAppExternalFilesDir') ??
          '';
    } catch (_) {
      return '';
    }
  }

  static Future<String?> saveViaMediaStore(
    String srcPath,
    String fileName, {
    String subDir = 'FerrisLoad',
  }) async {
    if (!Platform.isAndroid) {
      return srcPath;
    }
    try {
      return await _channel.invokeMethod<String>('saveToDownloads', {
        'srcPath': srcPath,
        'fileName': fileName,
        'mimeType': 'video/mp4',
        'subDir': subDir,
      });
    } catch (_) {
      return null;
    }
  }

  static Future<String?> saveToPath(
    String srcPath,
    String destDir,
    String fileName,
  ) async {
    if (!Platform.isAndroid) {
      return srcPath;
    }
    try {
      return await _channel.invokeMethod<String>('saveToPath', {
        'srcPath': srcPath,
        'destDir': destDir,
        'fileName': fileName,
      });
    } catch (_) {
      return null;
    }
  }

  static Future<void> startForegroundService() async {
    if (!Platform.isAndroid) {
      return;
    }
    try {
      await _channel.invokeMethod('startForegroundService');
    } catch (_) {}
  }

  static Future<void> stopForegroundService() async {
    if (!Platform.isAndroid) {
      return;
    }
    try {
      await _channel.invokeMethod('stopForegroundService');
    } catch (_) {}
  }

  static Future<void> updateForegroundProgress(
      int progress, String status) async {
    if (!Platform.isAndroid) {
      return;
    }
    try {
      await _channel.invokeMethod('updateServiceProgress', {
        'progress': progress,
        'status': status,
      });
    } catch (_) {}
  }

  static Future<bool> isIgnoringBatteryOptimizations() async {
    if (!Platform.isAndroid) {
      return true;
    }
    try {
      return await _channel
              .invokeMethod<bool>('isIgnoringBatteryOptimizations') ??
          false;
    } catch (_) {
      return false;
    }
  }

  static Future<void> requestIgnoreBatteryOptimizations() async {
    if (!Platform.isAndroid) {
      return;
    }
    try {
      await _channel.invokeMethod('requestIgnoreBatteryOptimizations');
    } catch (_) {}
  }
}
