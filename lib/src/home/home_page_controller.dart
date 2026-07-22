import 'package:flutter/foundation.dart';
import 'package:m3u8_downloader/src/home/home_widgets.dart';
import 'package:m3u8_downloader/src/rust/api/downloader.dart';

enum HomeWorkflowStage {
  idle,
  inspecting,
  ready,
  authRedirect,
  preparing,
  backend,
  playlist,
  transfer,
  segments,
  merge,
  transcode,
  exporting,
  completed,
  failed,
}

enum HomeRetryAction { none, analyze, download }

class HomePageViewModel {
  const HomePageViewModel({
    required this.keepTemp,
    required this.running,
    required this.analyzing,
    required this.ignoringBattery,
    required this.autoOpeningAuthBrowser,
    required this.progress,
    required this.status,
    required this.error,
    required this.chosenDir,
    required this.resultPath,
    required this.inspection,
    required this.selectedCandidate,
    required this.downloadTasks,
    required this.history,
    required this.stage,
    required this.stageRevision,
    required this.selectionRevision,
    required this.recoveryRevision,
    required this.recoveryMessage,
    required this.retryAction,
  });

  const HomePageViewModel.initial()
      : keepTemp = false,
        running = false,
        analyzing = false,
        ignoringBattery = false,
        autoOpeningAuthBrowser = false,
        progress = 0,
        status = null,
        error = null,
        chosenDir = null,
        resultPath = null,
        inspection = null,
        selectedCandidate = null,
        downloadTasks = const [],
        history = const [],
        stage = HomeWorkflowStage.idle,
        stageRevision = 0,
        selectionRevision = 0,
        recoveryRevision = 0,
        recoveryMessage = null,
        retryAction = HomeRetryAction.none;

  final bool keepTemp;
  final bool running;
  final bool analyzing;
  final bool ignoringBattery;
  final bool autoOpeningAuthBrowser;
  final double progress;
  final String? status;
  final String? error;
  final String? chosenDir;
  final String? resultPath;
  final MediaInspectionResult? inspection;
  final MediaCandidate? selectedCandidate;
  final List<HomeDownloadTask> downloadTasks;
  final List<DownloadRecord> history;
  final HomeWorkflowStage stage;
  final int stageRevision;
  final int selectionRevision;
  final int recoveryRevision;
  final String? recoveryMessage;
  final HomeRetryAction retryAction;

  bool get busy => running || analyzing;

  static const Object _sentinel = Object();

  HomePageViewModel copyWith({
    bool? keepTemp,
    bool? running,
    bool? analyzing,
    bool? ignoringBattery,
    bool? autoOpeningAuthBrowser,
    double? progress,
    Object? status = _sentinel,
    Object? error = _sentinel,
    Object? chosenDir = _sentinel,
    Object? resultPath = _sentinel,
    Object? inspection = _sentinel,
    Object? selectedCandidate = _sentinel,
    List<HomeDownloadTask>? downloadTasks,
    List<DownloadRecord>? history,
    HomeWorkflowStage? stage,
    int? stageRevision,
    int? selectionRevision,
    int? recoveryRevision,
    Object? recoveryMessage = _sentinel,
    HomeRetryAction? retryAction,
  }) {
    return HomePageViewModel(
      keepTemp: keepTemp ?? this.keepTemp,
      running: running ?? this.running,
      analyzing: analyzing ?? this.analyzing,
      ignoringBattery: ignoringBattery ?? this.ignoringBattery,
      autoOpeningAuthBrowser:
          autoOpeningAuthBrowser ?? this.autoOpeningAuthBrowser,
      progress: progress ?? this.progress,
      status: identical(status, _sentinel) ? this.status : status as String?,
      error: identical(error, _sentinel) ? this.error : error as String?,
      chosenDir: identical(chosenDir, _sentinel)
          ? this.chosenDir
          : chosenDir as String?,
      resultPath: identical(resultPath, _sentinel)
          ? this.resultPath
          : resultPath as String?,
      inspection: identical(inspection, _sentinel)
          ? this.inspection
          : inspection as MediaInspectionResult?,
      selectedCandidate: identical(selectedCandidate, _sentinel)
          ? this.selectedCandidate
          : selectedCandidate as MediaCandidate?,
      downloadTasks: downloadTasks ?? this.downloadTasks,
      history: history ?? this.history,
      stage: stage ?? this.stage,
      stageRevision: stageRevision ?? this.stageRevision,
      selectionRevision: selectionRevision ?? this.selectionRevision,
      recoveryRevision: recoveryRevision ?? this.recoveryRevision,
      recoveryMessage: identical(recoveryMessage, _sentinel)
          ? this.recoveryMessage
          : recoveryMessage as String?,
      retryAction: retryAction ?? this.retryAction,
    );
  }
}

class HomeDownloadTask {
  const HomeDownloadTask({
    required this.id,
    required this.fileName,
    required this.sourcePage,
    required this.status,
    required this.progress,
    required this.stage,
    required this.startedAt,
    required this.running,
    this.error,
    this.resultPath,
    this.backend,
  });

  final String id;
  final String fileName;
  final String sourcePage;
  final String status;
  final double progress;
  final HomeWorkflowStage stage;
  final DateTime startedAt;
  final bool running;
  final String? error;
  final String? resultPath;
  final String? backend;

  HomeDownloadTask copyWith({
    String? status,
    double? progress,
    HomeWorkflowStage? stage,
    bool? running,
    Object? error = HomePageViewModel._sentinel,
    Object? resultPath = HomePageViewModel._sentinel,
    Object? backend = HomePageViewModel._sentinel,
  }) {
    return HomeDownloadTask(
      id: id,
      fileName: fileName,
      sourcePage: sourcePage,
      status: status ?? this.status,
      progress: progress ?? this.progress,
      stage: stage ?? this.stage,
      startedAt: startedAt,
      running: running ?? this.running,
      error: identical(error, HomePageViewModel._sentinel)
          ? this.error
          : error as String?,
      resultPath: identical(resultPath, HomePageViewModel._sentinel)
          ? this.resultPath
          : resultPath as String?,
      backend: identical(backend, HomePageViewModel._sentinel)
          ? this.backend
          : backend as String?,
    );
  }
}

class HomePageController extends ChangeNotifier {
  HomePageViewModel _value = const HomePageViewModel.initial();

  HomePageViewModel get value => _value;

  void _apply(HomePageViewModel next, {HomeWorkflowStage? stage}) {
    final resolvedStage = stage ?? next.stage;
    final nextValue = next.copyWith(
      stage: resolvedStage,
      stageRevision: resolvedStage == _value.stage
          ? _value.stageRevision
          : _value.stageRevision + 1,
    );
    _value = nextValue;
    notifyListeners();
  }

  HomePageViewModel _withRecoveredError(HomePageViewModel next) {
    final previousError = _value.error;
    if (previousError == null || previousError.isEmpty) {
      return next.copyWith(recoveryMessage: null);
    }
    return next.copyWith(
      recoveryMessage: previousError,
      recoveryRevision: _value.recoveryRevision + 1,
    );
  }

  HomeWorkflowStage _classifyProgressStage(String status) {
    final lower = status.toLowerCase();
    if (lower.contains('authorization challenge')) {
      return HomeWorkflowStage.authRedirect;
    }
    if (lower.contains('transcoder backend') ||
        lower.contains('selected site engine') ||
        lower.contains('selected hardware encoder') ||
        lower.contains('selected ffmpeg stream copy') ||
        lower.contains('selected android mediamuxer') ||
        lower.contains('using ') && lower.contains(' backend')) {
      return HomeWorkflowStage.backend;
    }
    if (lower.contains('downloading m3u8 playlist')) {
      return HomeWorkflowStage.playlist;
    }
    if (lower.contains('downloading segments')) {
      return HomeWorkflowStage.segments;
    }
    if (lower.contains('merging segments') ||
        lower.contains('merging streams')) {
      return HomeWorkflowStage.merge;
    }
    if (lower.contains('converting to mp4') ||
        lower.contains('transcode') ||
        lower.contains(' encoder')) {
      return HomeWorkflowStage.transcode;
    }
    if (lower.contains('export')) {
      return HomeWorkflowStage.exporting;
    }
    if (lower.contains('all tasks completed') || lower.contains('completed')) {
      return HomeWorkflowStage.completed;
    }
    if (lower.contains('preparing')) {
      return HomeWorkflowStage.preparing;
    }
    if (lower.contains('downloading video stream') ||
        lower.contains('downloading audio stream') ||
        lower.contains('downloading media') ||
        lower.contains('yt-dlp download') ||
        lower.contains('direct media')) {
      return HomeWorkflowStage.transfer;
    }
    if (_value.analyzing) {
      return HomeWorkflowStage.inspecting;
    }
    if (_value.running) {
      return HomeWorkflowStage.transfer;
    }
    if (_value.selectedCandidate != null) {
      return HomeWorkflowStage.ready;
    }
    return HomeWorkflowStage.idle;
  }

  String? _backendFromStatus(String status) {
    for (final prefix in <String>[
      'Selected site engine: ',
      'Selected transcoder backend: ',
      'Selected hardware encoder: ',
    ]) {
      if (status.startsWith(prefix)) {
        return status.substring(prefix.length).trim();
      }
    }
    if (status.contains('unavailable; retrying') &&
        status.contains('CPU libx264')) {
      return 'CPU libx264';
    }
    if (status.startsWith('Selected FFmpeg stream copy') ||
        status.startsWith('Remuxing with FFmpeg stream copy') ||
        status.startsWith('Merging streams with FFmpeg stream copy')) {
      return 'FFmpeg stream copy';
    }
    if (status.startsWith('Selected Android MediaMuxer') ||
        status.startsWith('Remuxing with Android MediaMuxer') ||
        status.startsWith('Merging streams with Android MediaMuxer')) {
      return 'Android MediaMuxer';
    }
    if (status.startsWith('Using ') && status.endsWith(' backend')) {
      return status.substring(6, status.length - 8).trim();
    }
    if (status.startsWith('Using ') && status.endsWith(' encoder')) {
      return status.substring(6, status.length - 8).trim();
    }
    const mergeEncodeSuffix = ' to merge and encode streams';
    if (status.startsWith('Using ') && status.endsWith(mergeEncodeSuffix)) {
      return status.substring(6, status.length - mergeEncodeSuffix.length);
    }
    return null;
  }

  List<HomeDownloadTask> _replaceTask(
    String id,
    HomeDownloadTask Function(HomeDownloadTask task) update,
  ) {
    return [
      for (final task in _value.downloadTasks)
        if (task.id == id) update(task) else task,
    ];
  }

  bool _hasRunningTasks(List<HomeDownloadTask> tasks) {
    return tasks.any((task) => task.running);
  }

  void setIgnoringBattery(bool value) {
    _apply(_value.copyWith(ignoringBattery: value));
  }

  void setChosenDir(String? value) {
    _apply(_value.copyWith(chosenDir: value));
  }

  void setKeepTemp(bool value) {
    _apply(_value.copyWith(keepTemp: value));
  }

  void selectCandidate(MediaCandidate candidate, {String? status}) {
    _apply(
      _value.copyWith(
        selectedCandidate: candidate,
        selectionRevision: _value.selectionRevision + 1,
        status: _value.busy ? _value.status : status ?? _value.status,
      ),
      stage: _value.busy ? _value.stage : HomeWorkflowStage.ready,
    );
  }

  void setAutoOpeningAuthBrowser(bool value) {
    _apply(_value.copyWith(autoOpeningAuthBrowser: value));
  }

  void setError(String error, {String? status, double? progress}) {
    _apply(
      _value.copyWith(
        error: error,
        status: status,
        progress: progress,
        recoveryMessage: null,
      ),
      stage: HomeWorkflowStage.failed,
    );
  }

  void importAuthSession(String status) {
    _apply(
      _withRecoveredError(
        _value.copyWith(
          status: status,
          error: null,
          retryAction: HomeRetryAction.none,
        ),
      ),
      stage: _value.selectedCandidate != null
          ? HomeWorkflowStage.ready
          : HomeWorkflowStage.idle,
    );
  }

  void beginAnalyze(String status) {
    _apply(
      _withRecoveredError(
        _value.copyWith(
          analyzing: true,
          error: null,
          status: status,
          inspection: null,
          selectedCandidate: null,
          progress: 0.08,
          resultPath: null,
          retryAction: HomeRetryAction.analyze,
        ),
      ),
      stage: HomeWorkflowStage.inspecting,
    );
  }

  void completeAnalyze(
    MediaInspectionResult inspection, {
    required MediaCandidate? selectedCandidate,
    required String readyStatus,
    required String emptyStatus,
  }) {
    _apply(
      _value.copyWith(
        inspection: inspection,
        selectedCandidate: selectedCandidate,
        status: inspection.candidates.isNotEmpty ? readyStatus : emptyStatus,
        progress: inspection.candidates.isNotEmpty ? 0.16 : 0,
      ),
      stage: selectedCandidate != null
          ? HomeWorkflowStage.ready
          : HomeWorkflowStage.idle,
    );
  }

  void markAuthRedirecting(String status) {
    _apply(
      _value.copyWith(status: status, error: null),
      stage: HomeWorkflowStage.authRedirect,
    );
  }

  void finishAnalyze() {
    _apply(_value.copyWith(analyzing: false));
  }

  void beginDownload(String status) {
    _apply(
      _withRecoveredError(
        _value.copyWith(
          running: true,
          error: null,
          resultPath: null,
          progress: 0.02,
          status: status,
          retryAction: HomeRetryAction.download,
        ),
      ),
      stage: HomeWorkflowStage.preparing,
    );
  }

  String beginDownloadTask({
    required String fileName,
    required String sourcePage,
    required String status,
  }) {
    final startedAt = DateTime.now();
    final task = HomeDownloadTask(
      id: '${startedAt.microsecondsSinceEpoch}-${_value.downloadTasks.length}',
      fileName: fileName,
      sourcePage: sourcePage,
      status: status,
      progress: 0.02,
      stage: HomeWorkflowStage.preparing,
      startedAt: startedAt,
      running: true,
    );
    _apply(
      _withRecoveredError(
        _value.copyWith(
          running: true,
          error: null,
          resultPath: null,
          progress: 0.02,
          status: status,
          retryAction: HomeRetryAction.download,
          downloadTasks: [task, ..._value.downloadTasks].take(8).toList(),
        ),
      ),
      stage: HomeWorkflowStage.preparing,
    );
    return task.id;
  }

  void updateProgress(String status, double progress) {
    _apply(
      _value.copyWith(status: status, progress: progress.clamp(0.0, 1.0)),
      stage: _classifyProgressStage(status),
    );
  }

  void updateDownloadTask(String id, String status, double progress) {
    final stage = _classifyProgressStage(status);
    final backend = _backendFromStatus(status);
    final tasks = _replaceTask(
      id,
      (task) => task.copyWith(
        status: status,
        progress: progress.clamp(0.0, 1.0),
        stage: stage,
        backend: backend ?? task.backend,
      ),
    );
    _apply(
      _value.copyWith(
        running: _hasRunningTasks(tasks),
        status: status,
        progress: progress.clamp(0.0, 1.0),
        downloadTasks: tasks,
      ),
      stage: stage,
    );
  }

  void completeDownload({
    required String finalPath,
    required String fileName,
    required String sourcePage,
    required String completedStatus,
  }) {
    _apply(
      _value.copyWith(
        resultPath: finalPath,
        status: completedStatus,
        progress: 1,
        history: [
          DownloadRecord(
            name: fileName,
            source: sourcePage,
            path: finalPath,
            time: DateTime.now(),
          ),
          ..._value.history,
        ],
        retryAction: HomeRetryAction.none,
      ),
      stage: HomeWorkflowStage.completed,
    );
  }

  void completeDownloadTask({
    required String id,
    required String finalPath,
    required String fileName,
    required String sourcePage,
    required String completedStatus,
  }) {
    final tasks = _replaceTask(
      id,
      (task) => task.copyWith(
        status: completedStatus,
        progress: 1,
        stage: HomeWorkflowStage.completed,
        running: false,
        error: null,
        resultPath: finalPath,
      ),
    );
    final hasRunningTasks = _hasRunningTasks(tasks);
    _apply(
      _value.copyWith(
        running: hasRunningTasks,
        resultPath: finalPath,
        status: completedStatus,
        progress: 1,
        downloadTasks: tasks,
        history: [
          DownloadRecord(
            name: fileName,
            source: sourcePage,
            path: finalPath,
            time: DateTime.now(),
          ),
          ..._value.history,
        ],
        retryAction:
            hasRunningTasks ? _value.retryAction : HomeRetryAction.none,
      ),
      stage: HomeWorkflowStage.completed,
    );
  }

  void failDownloadTask(
    String id,
    String error, {
    String? status,
    double progress = 0,
  }) {
    final tasks = _replaceTask(
      id,
      (task) => task.copyWith(
        status: status ?? error,
        progress: progress,
        stage: HomeWorkflowStage.failed,
        running: false,
        error: error,
      ),
    );
    _apply(
      _value.copyWith(
        running: _hasRunningTasks(tasks),
        error: error,
        status: status,
        progress: progress,
        recoveryMessage: null,
        downloadTasks: tasks,
      ),
      stage: HomeWorkflowStage.failed,
    );
  }

  void finishDownload() {
    _apply(_value.copyWith(running: false));
  }
}
