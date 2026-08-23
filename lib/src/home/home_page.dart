import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/app/app_settings.dart';
import 'package:m3u8_downloader/src/app/auth_browser_page.dart';
import 'package:m3u8_downloader/src/app/download_engine.dart';
import 'package:m3u8_downloader/src/app/local_file_utils.dart';
import 'package:m3u8_downloader/src/app/platform_bridge.dart';
import 'package:m3u8_downloader/src/app/platform_utils.dart';
import 'package:m3u8_downloader/src/home/home_candidates_card.dart';
import 'package:m3u8_downloader/src/home/home_download_tasks_card.dart';
import 'package:m3u8_downloader/src/home/home_page_controller.dart';
import 'package:m3u8_downloader/src/home/home_history_card.dart';
import 'package:m3u8_downloader/src/home/home_input_card.dart';
import 'package:m3u8_downloader/src/home/home_settings_sheet.dart';
import 'package:m3u8_downloader/src/home/home_status_card.dart';
import 'package:m3u8_downloader/src/home/home_widgets.dart';
import 'package:m3u8_downloader/src/home/source_input.dart';
import 'package:m3u8_downloader/src/rust/api/downloader.dart';

class HomePage extends StatefulWidget {
  const HomePage({
    super.key,
    required this.settings,
    required this.onSettingsChanged,
  });

  final AppSettings settings;
  final ValueChanged<AppSettings> onSettingsChanged;

  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage> {
  // Not `final`: recreated in `didUpdateWidget` when the web API settings
  // change, so it must be reassignable.
  late DownloadEngine _engine;
  final _formKey = GlobalKey<FormState>();
  final _urlCtrl = TextEditingController();
  final _fileNameCtrl = TextEditingController(text: 'video.mp4');
  final _concurrencyCtrl = TextEditingController(text: '8');
  final _retriesCtrl = TextEditingController(text: '3');
  final _vBitrateCtrl = TextEditingController(text: '0');
  final _aBitrateCtrl = TextEditingController(text: '0');
  final _userAgentCtrl = TextEditingController();
  final _refererCtrl = TextEditingController();
  final _originCtrl = TextEditingController();
  final _cookieCtrl = TextEditingController();
  final _headersCtrl = TextEditingController();
  final _pageController = HomePageController();
  late final Listenable _authContextListenable = Listenable.merge([
    _userAgentCtrl,
    _refererCtrl,
    _originCtrl,
    _cookieCtrl,
    _headersCtrl,
  ]);

  @override
  void initState() {
    super.initState();
    _engine = createDownloadEngine(
      apiBaseUrl: widget.settings.apiBaseUrl,
      apiToken: widget.settings.apiToken,
    );
    _checkBattery();
  }

  @override
  void didUpdateWidget(HomePage oldWidget) {
    super.didUpdateWidget(oldWidget);
    // On the web the engine is bound to the API base URL + token; recreate it
    // when the user changes those settings so requests use the new endpoint.
    if (oldWidget.settings.apiBaseUrl != widget.settings.apiBaseUrl ||
        oldWidget.settings.apiToken != widget.settings.apiToken) {
      _engine = createDownloadEngine(
        apiBaseUrl: widget.settings.apiBaseUrl,
        apiToken: widget.settings.apiToken,
      );
    }
  }

  @override
  void dispose() {
    _urlCtrl.dispose();
    _fileNameCtrl.dispose();
    _concurrencyCtrl.dispose();
    _retriesCtrl.dispose();
    _vBitrateCtrl.dispose();
    _aBitrateCtrl.dispose();
    _userAgentCtrl.dispose();
    _refererCtrl.dispose();
    _originCtrl.dispose();
    _cookieCtrl.dispose();
    _headersCtrl.dispose();
    _pageController.dispose();
    super.dispose();
  }

  Future<void> _checkBattery() async {
    final value = await MediaStoreBridge.isIgnoringBatteryOptimizations();
    _pageController.setIgnoringBattery(value);
  }

  Future<void> _pickDir() async {
    if (_pageController.value.analyzing || FerrisPlatform.isWeb) {
      // The web build saves to the API server, so there is no local directory.
      return;
    }
    final dir = await FilePicker.getDirectoryPath();
    if (dir != null) {
      _pageController.setChosenDir(dir);
    }
  }

  Future<String> _tempOutputPathFor(String name, String? chosenDir) async {
    if (FerrisPlatform.isWeb) {
      // The web build sends only the file name; the API server owns the path.
      return name;
    }
    if (FerrisPlatform.isAndroid) {
      final safe = await MediaStoreBridge.getAppPrivateDir();
      return safe.isNotEmpty
          ? '$safe${FerrisPlatform.pathSeparator}$name'
          : name;
    }
    if (chosenDir != null && chosenDir.isNotEmpty) {
      final separator = FerrisPlatform.pathSeparator;
      return chosenDir.endsWith(separator)
          ? '$chosenDir$name'
          : '$chosenDir$separator$name';
    }
    return name;
  }

  String _normalizeOutputName(String value) {
    final trimmed = value.trim().isEmpty ? 'video.mp4' : value.trim();
    // Strip any directory components so the file name cannot escape the chosen
    // output directory (path traversal), then replace reserved and control
    // characters with underscores so it is safe on every platform.
    final basename = trimmed.split(RegExp(r'[\\/]')).last.trim();
    final sanitized = basename.replaceAll(
      RegExp(r'[\\/:*?"<>|\x00-\x1f]'),
      '_',
    );
    final cleaned = (sanitized.isEmpty || sanitized == '.' || sanitized == '..')
        ? 'video'
        : sanitized;
    return cleaned.toLowerCase().endsWith('.mp4') ? cleaned : '$cleaned.mp4';
  }

  String _suggestFileName(MediaCandidate candidate) {
    final raw =
        candidate.title.trim().isEmpty ? 'video' : candidate.title.trim();
    final sanitized = raw.replaceAll(RegExp(r'[\\/:*?"<>|]'), '_');
    return _normalizeOutputName(sanitized);
  }

  /// Strip CR/LF and other control bytes from a pasted header value so a
  /// multi-line clipboard paste can never reach the Rust engine and trigger
  /// a header-injection guard (or corrupt an upstream request). Non-ASCII
  /// characters are preserved.
  String _sanitizeHeaderValue(String value) {
    return value.replaceAll(RegExp(r'[\x00-\x08\x0A-\x1F\x7F]'), '');
  }

  RequestContext _requestContext() {
    return RequestContext(
      userAgent: _sanitizeHeaderValue(_userAgentCtrl.text.trim()),
      referer: _sanitizeHeaderValue(_refererCtrl.text.trim()),
      origin: _sanitizeHeaderValue(_originCtrl.text.trim()),
      cookie: _sanitizeHeaderValue(_cookieCtrl.text.trim()),
      headers: _parseHeaderEntries(_headersCtrl.text),
    );
  }

  List<HeaderEntry> _parseHeaderEntries(String text) {
    final entries = <HeaderEntry>[];
    for (final line in text.split('\n')) {
      final trimmed = line.trim();
      if (trimmed.isEmpty) {
        continue;
      }
      final index = trimmed.indexOf(':');
      if (index <= 0) {
        continue;
      }
      final name = trimmed.substring(0, index).trim();
      final value = _sanitizeHeaderValue(trimmed.substring(index + 1).trim());
      if (name.isEmpty || value.isEmpty) {
        continue;
      }
      entries.add(HeaderEntry(name: name, value: value));
    }
    return entries;
  }

  bool get _hasAuthContextOverrides {
    return _userAgentCtrl.text.trim().isNotEmpty ||
        _refererCtrl.text.trim().isNotEmpty ||
        _originCtrl.text.trim().isNotEmpty ||
        _cookieCtrl.text.trim().isNotEmpty ||
        _parseHeaderEntries(_headersCtrl.text).isNotEmpty;
  }

  List<String> _authContextBadges() {
    final badges = <String>[];
    if (_userAgentCtrl.text.trim().isNotEmpty) {
      badges.add('User-Agent');
    }
    if (_refererCtrl.text.trim().isNotEmpty) {
      badges.add('Referer');
    }
    if (_originCtrl.text.trim().isNotEmpty) {
      badges.add('Origin');
    }
    if (_cookieCtrl.text.trim().isNotEmpty) {
      badges.add('Cookie');
    }
    final headerCount = _parseHeaderEntries(_headersCtrl.text).length;
    if (headerCount > 0) {
      badges.add('Headers x$headerCount');
    }
    return badges;
  }

  void _clearAuthContext() {
    _userAgentCtrl.clear();
    _refererCtrl.clear();
    _originCtrl.clear();
    _cookieCtrl.clear();
    _headersCtrl.clear();
  }

  bool _looksLikeAuthChallenge(String message) {
    final lower = message.toLowerCase();
    return lower.contains('authorization required') ||
        lower.contains('access challenge') ||
        lower.contains('cloudflare') ||
        lower.contains('captcha') ||
        lower.contains('checking your browser') ||
        lower.contains('checking if the site connection is secure') ||
        lower.contains('cf-chl-') ||
        lower.contains('forbidden') ||
        lower.contains('too many requests') ||
        lower.contains('http 401') ||
        lower.contains('http 403') ||
        lower.contains('http 429');
  }

  bool _shouldAutoOpenAuthBrowser({
    required bool skipAutoAuth,
    MediaInspectionResult? inspection,
    String? errorText,
  }) {
    final vm = _pageController.value;
    if (skipAutoAuth ||
        vm.autoOpeningAuthBrowser ||
        !widget.settings.autoOpenAuthBrowser) {
      return false;
    }
    if (inspection?.authRequired ?? false) {
      return true;
    }
    return errorText != null && _looksLikeAuthChallenge(errorText);
  }

  Future<void> _openAuthBrowser({bool reanalyzeAfterImport = false}) async {
    if (_pageController.value.autoOpeningAuthBrowser) {
      return;
    }
    final l = AppLocalizations.of(context);
    final targetUrl = extractSourceUrl(_urlCtrl.text)!;
    _replaceSourceText(targetUrl);
    if (targetUrl.isEmpty) {
      _pageController.setError(l.text('input_source'));
      return;
    }

    _pageController.setAutoOpeningAuthBrowser(true);
    try {
      final session = await Navigator.of(context).push<AuthSessionBundle>(
        MaterialPageRoute(
          builder: (context) => AuthBrowserPage(
            initialUrl: targetUrl,
            seedContext: _requestContext(),
          ),
          fullscreenDialog: true,
        ),
      );

      if (!mounted || session == null) {
        return;
      }

      if (session.userAgent.isNotEmpty) {
        _userAgentCtrl.text = session.userAgent;
      }
      if (session.referer.isNotEmpty) {
        _refererCtrl.text = session.referer;
      }
      if (session.origin.isNotEmpty) {
        _originCtrl.text = session.origin;
      }
      if (session.cookie.isNotEmpty) {
        _cookieCtrl.text = session.cookie;
      }
      _pageController.importAuthSession(l.text('auth_session_imported'));

      if (reanalyzeAfterImport) {
        await _analyze(skipAutoAuth: true);
      }
    } finally {
      _pageController.setAutoOpeningAuthBrowser(false);
    }
  }

  Future<void> _analyze({bool skipAutoAuth = false}) async {
    if (!_formKey.currentState!.validate()) {
      return;
    }
    final l = AppLocalizations.of(context);
    final targetUrl = _urlCtrl.text.trim();
    final requestContext = _requestContext();
    var shouldAutoOpenAuthBrowser = false;

    _pageController.beginAnalyze(l.text('analyzing'));

    try {
      final inspection = await _engine.inspect(
        url: targetUrl,
        requestContext: requestContext,
      );
      if (!mounted) {
        return;
      }
      if (inspection.pageUrl.isNotEmpty) {
        _replaceSourceText(inspection.pageUrl);
      }
      final selectedCandidate =
          inspection.candidates.isNotEmpty ? inspection.candidates.first : null;
      _pageController.completeAnalyze(
        inspection,
        selectedCandidate: selectedCandidate,
        readyStatus: l.text('ready'),
        emptyStatus: l.text('no_candidates'),
      );
      if (selectedCandidate != null) {
        _fileNameCtrl.text = _suggestFileName(selectedCandidate);
      }
      shouldAutoOpenAuthBrowser = _shouldAutoOpenAuthBrowser(
        skipAutoAuth: skipAutoAuth,
        inspection: inspection,
      );
      if (shouldAutoOpenAuthBrowser && mounted) {
        _pageController.markAuthRedirecting(l.text('auth_redirecting'));
      }
    } catch (error) {
      if (!mounted) {
        return;
      }
      final errorText = '$error';
      shouldAutoOpenAuthBrowser = _shouldAutoOpenAuthBrowser(
        skipAutoAuth: skipAutoAuth,
        errorText: errorText,
      );
      _pageController.setError(
        errorText,
        status: shouldAutoOpenAuthBrowser ? l.text('auth_redirecting') : null,
        progress: 0,
      );
    } finally {
      _pageController.finishAnalyze();
    }

    if (shouldAutoOpenAuthBrowser && mounted) {
      await _openAuthBrowser(reanalyzeAfterImport: true);
    }
  }

  Future<void> _download() async {
    if (!_formKey.currentState!.validate()) {
      return;
    }
    final l = AppLocalizations.of(context);
    final requestContext = _requestContext();
    var shouldAutoOpenAuthBrowser = false;

    var vm = _pageController.value;
    final enteredUrl = extractSourceUrl(_urlCtrl.text)!;
    _replaceSourceText(enteredUrl);
    final selectedCandidate = vm.selectedCandidate;
    final candidateMatchesInput = selectedCandidate != null &&
        (selectedCandidate.pageUrl == enteredUrl ||
            selectedCandidate.mediaUrl == enteredUrl);
    final pageUrl =
        candidateMatchesInput ? selectedCandidate.pageUrl : enteredUrl;
    final mediaUrl =
        candidateMatchesInput ? selectedCandidate.mediaUrl : enteredUrl;
    final audioUrl = candidateMatchesInput ? selectedCandidate.audioUrl : null;
    final sourcePage = pageUrl;

    final fileName = _normalizeOutputName(_fileNameCtrl.text.trim());
    final chosenDir = vm.chosenDir;
    final keepTemp = vm.keepTemp;
    final output = await _tempOutputPathFor(fileName, chosenDir);
    final concurrency = int.parse(_concurrencyCtrl.text.trim());
    final retries = int.parse(_retriesCtrl.text.trim());
    final vBitrate = int.parse(_vBitrateCtrl.text.trim());
    final aBitrate = int.parse(_aBitrateCtrl.text.trim());

    final taskId = _pageController.beginDownloadTask(
      fileName: fileName,
      sourcePage: sourcePage,
      status: l.text('preparing'),
    );

    await MediaStoreBridge.startForegroundService();
    var lastNotifiedPercent = -1;
    var lastNotifiedMessage = <String>{};

    try {
      await for (final event in _engine.download(
        pageUrl: pageUrl,
        mediaUrl: mediaUrl,
        audioUrl: audioUrl,
        output: output,
        concurrency: concurrency,
        retries: retries,
        videoBitrate: vBitrate,
        audioBitrate: aBitrate,
        keepTemp: keepTemp,
        requestContext: requestContext,
      )) {
        if (!mounted) {
          return;
        }
        final terminalError = event.error;
        if (terminalError != null && terminalError.isNotEmpty) {
          _pageController.failDownloadTask(
            taskId,
            terminalError,
            status: null,
            progress: 0,
          );
          shouldAutoOpenAuthBrowser = _shouldAutoOpenAuthBrowser(
            skipAutoAuth: false,
            errorText: terminalError,
          );
          return;
        }
        _pageController.updateDownloadTask(
          taskId,
          event.message,
          event.progress,
        );
        final percent = (event.progress * 100).round();
        final isNewMessage = !lastNotifiedMessage.contains(event.message);
        if (isNewMessage || percent - lastNotifiedPercent >= 1) {
          lastNotifiedPercent = percent;
          lastNotifiedMessage = {event.message};
          await MediaStoreBridge.updateForegroundProgress(
            percent,
            event.message,
          );
        }
      }

      if (!mounted) {
        return;
      }

      String finalPath;
      if (_engine.writesLocalFiles) {
        if (!await localFileExists(output) ||
            await localFileLength(output) == 0) {
          _pageController.failDownloadTask(
            taskId,
            l.text('output_not_created'),
            status: null,
            progress: 0,
          );
          return;
        }
        finalPath = output;
        if (FerrisPlatform.isAndroid) {
          String? savedPath;
          if (chosenDir != null && chosenDir.isNotEmpty) {
            savedPath = await MediaStoreBridge.saveToPath(
              output,
              chosenDir,
              fileName,
            );
          } else {
            savedPath = await MediaStoreBridge.saveViaMediaStore(
              output,
              fileName,
            );
          }
          if (savedPath == null) {
            _pageController.failDownloadTask(
              taskId,
              '${l.text('export_failed')} $output',
              status: null,
            );
            return;
          }
          finalPath = savedPath;
          await localFileDelete(output);
        }
      } else {
        // Web: the file lives on the API server; surface its path.
        finalPath = _engine.lastResultPath ?? output;
      }

      _pageController.completeDownloadTask(
        id: taskId,
        finalPath: finalPath,
        fileName: fileName,
        sourcePage: sourcePage,
        completedStatus: l.text('completed'),
      );
    } catch (error) {
      if (!mounted) {
        return;
      }
      final errorText = '$error';
      shouldAutoOpenAuthBrowser = _shouldAutoOpenAuthBrowser(
        skipAutoAuth: false,
        errorText: errorText,
      );
      _pageController.failDownloadTask(
        taskId,
        errorText,
        status: shouldAutoOpenAuthBrowser ? l.text('auth_redirecting') : null,
        progress: 0,
      );
    } finally {
      if (!_pageController.value.running) {
        await MediaStoreBridge.stopForegroundService();
      }
    }

    if (shouldAutoOpenAuthBrowser && mounted) {
      await _openAuthBrowser(reanalyzeAfterImport: true);
    }
  }

  Future<void> _showSettingsSheet() async {
    await showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      showDragHandle: true,
      useSafeArea: true,
      builder: (context) => HomeSettingsSheet(
        settings: widget.settings,
        enabled: !_pageController.value.busy,
        inspection: _pageController.value.inspection,
        onSettingsChanged: widget.onSettingsChanged,
        onOpenBrowser: () => _openAuthBrowser(
          reanalyzeAfterImport:
              _pageController.value.inspection?.authRequired ?? false,
        ),
        onClearContext: _clearAuthContext,
        authContextListenable: _authContextListenable,
        authContextBadges: _authContextBadges,
        hasAuthContextOverrides: () => _hasAuthContextOverrides,
        userAgentController: _userAgentCtrl,
        refererController: _refererCtrl,
        originController: _originCtrl,
        cookieController: _cookieCtrl,
        headersController: _headersCtrl,
      ),
    );
  }

  void _replaceSourceText(String source) {
    if (_urlCtrl.text == source) {
      return;
    }
    _urlCtrl.value = TextEditingValue(
      text: source,
      selection: TextSelection.collapsed(offset: source.length),
    );
  }

  Future<void> _retryCurrentAction(HomeRetryAction action) async {
    switch (action) {
      case HomeRetryAction.none:
        return;
      case HomeRetryAction.analyze:
        await _analyze();
      case HomeRetryAction.download:
        await _download();
    }
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _pageController,
      builder: (context, _) {
        final vm = _pageController.value;
        final l = AppLocalizations.of(context);
        final t = Theme.of(context);
        final cs = t.colorScheme;

        return Scaffold(
          appBar: AppBar(
            title: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(l.text('app_title')),
                Text(
                  l.text('app_subtitle'),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: t.textTheme.bodySmall?.copyWith(
                    color: cs.onSurfaceVariant,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ],
            ),
            actions: [
              IconButton(
                onPressed: _showSettingsSheet,
                icon: const Icon(Icons.tune_rounded),
                tooltip: l.text('appearance'),
              ),
            ],
          ),
          body: ColoredBox(
            color: cs.surface,
            child: SafeArea(
              child: LayoutBuilder(
                builder: (context, constraints) {
                  if (constraints.maxWidth >= 1080) {
                    return Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Expanded(
                          flex: 7,
                          child: RevealMotion(
                            delay: const Duration(milliseconds: 40),
                            child: _buildPrimaryPane(
                              context,
                              vm: vm,
                              scrollable: true,
                              padding: const EdgeInsets.fromLTRB(
                                16,
                                10,
                                16,
                                28,
                              ),
                            ),
                          ),
                        ),
                        const SizedBox(width: 12),
                        SizedBox(
                          width: 360,
                          child: RevealMotion(
                            delay: const Duration(milliseconds: 120),
                            offset: const Offset(0.06, 0),
                            child: _buildSidePane(
                              context,
                              vm: vm,
                              scrollable: true,
                              padding: const EdgeInsets.fromLTRB(0, 10, 16, 28),
                            ),
                          ),
                        ),
                      ],
                    );
                  }
                  return ListView(
                    physics: const BouncingScrollPhysics(
                      parent: AlwaysScrollableScrollPhysics(),
                    ),
                    padding: const EdgeInsets.fromLTRB(16, 10, 16, 28),
                    children: [
                      RevealMotion(
                        delay: const Duration(milliseconds: 40),
                        child: _buildPrimaryPane(
                          context,
                          vm: vm,
                          scrollable: false,
                        ),
                      ),
                      const SizedBox(height: 12),
                      RevealMotion(
                        delay: const Duration(milliseconds: 120),
                        child: _buildSidePane(
                          context,
                          vm: vm,
                          scrollable: false,
                        ),
                      ),
                    ],
                  );
                },
              ),
            ),
          ),
        );
      },
    );
  }

  Widget _buildPrimaryPane(
    BuildContext context, {
    required HomePageViewModel vm,
    required bool scrollable,
    EdgeInsetsGeometry padding = EdgeInsets.zero,
  }) {
    final l = AppLocalizations.of(context);
    final children = <Widget>[
      if (FerrisPlatform.isAndroid && !vm.ignoringBattery)
        Padding(
          padding: const EdgeInsets.only(bottom: 12),
          child: BatteryBanner(
            title: l.text('battery'),
            body: l.text('battery_hint'),
            buttonLabel: l.text('disable_now'),
            onRequest: () async {
              await MediaStoreBridge.requestIgnoreBatteryOptimizations();
              Future.delayed(const Duration(seconds: 2), _checkBattery);
            },
          ),
        ),
      HomeInputCard(
        formKey: _formKey,
        urlController: _urlCtrl,
        fileNameController: _fileNameCtrl,
        concurrencyController: _concurrencyCtrl,
        retriesController: _retriesCtrl,
        videoBitrateController: _vBitrateCtrl,
        audioBitrateController: _aBitrateCtrl,
        running: vm.running,
        analyzing: vm.analyzing,
        keepTemp: vm.keepTemp,
        chosenDir: vm.chosenDir,
        onPickDir: _pickDir,
        onResetDir: () => _pageController.setChosenDir(null),
        onAnalyze: _analyze,
        onDownload: _download,
        onKeepTempChanged: _pageController.setKeepTemp,
      ),
      const SizedBox(height: 12),
      HomeCandidatesCard(
        inspection: vm.inspection,
        selectedCandidate: vm.selectedCandidate,
        running: vm.running,
        analyzing: vm.analyzing,
        selectionRevision: vm.selectionRevision,
        onCandidateSelected: (candidate) {
          _pageController.selectCandidate(
            candidate,
            status: l.text('candidate_switched'),
          );
          _fileNameCtrl.text = _suggestFileName(candidate);
        },
        onOpenAuthBrowser: () => _openAuthBrowser(reanalyzeAfterImport: true),
      ),
    ];

    if (scrollable) {
      return ListView(
        physics: const BouncingScrollPhysics(
          parent: AlwaysScrollableScrollPhysics(),
        ),
        padding: padding,
        children: children,
      );
    }

    return Padding(
      padding: padding,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: children,
      ),
    );
  }

  Widget _buildSidePane(
    BuildContext context, {
    required HomePageViewModel vm,
    required bool scrollable,
    EdgeInsetsGeometry padding = EdgeInsets.zero,
  }) {
    final children = <Widget>[
      HomeStatusCard(
        status: vm.status,
        error: vm.error,
        resultPath: vm.resultPath,
        progress: vm.progress,
        running: vm.running,
        analyzing: vm.analyzing,
        stage: vm.stage,
        stageRevision: vm.stageRevision,
        recoveryMessage: vm.recoveryMessage,
        recoveryRevision: vm.recoveryRevision,
        retryAction: vm.retryAction,
        onRetry: vm.busy || vm.retryAction == HomeRetryAction.none
            ? null
            : () {
                _retryCurrentAction(vm.retryAction);
              },
      ),
      const SizedBox(height: 12),
      HomeDownloadTasksCard(tasks: vm.downloadTasks),
      const SizedBox(height: 12),
      HomeHistoryCard(history: vm.history),
    ];

    if (scrollable) {
      return ListView(
        physics: const BouncingScrollPhysics(
          parent: AlwaysScrollableScrollPhysics(),
        ),
        padding: padding,
        children: children,
      );
    }

    return Padding(
      padding: padding,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: children,
      ),
    );
  }
}
