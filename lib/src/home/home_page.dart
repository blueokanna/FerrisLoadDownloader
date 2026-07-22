import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/app/app_settings.dart';
import 'package:m3u8_downloader/src/app/auth_browser_page.dart';
import 'package:m3u8_downloader/src/app/platform_bridge.dart';
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
    _checkBattery();
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
    if (_pageController.value.analyzing) {
      return;
    }
    final dir = await FilePicker.getDirectoryPath();
    if (dir != null) {
      _pageController.setChosenDir(dir);
    }
  }

  Future<String> _tempOutputPathFor(String name, String? chosenDir) async {
    if (Platform.isAndroid) {
      final safe = await MediaStoreBridge.getAppPrivateDir();
      return safe.isNotEmpty ? '$safe${Platform.pathSeparator}$name' : name;
    }
    if (chosenDir != null && chosenDir.isNotEmpty) {
      final separator = Platform.pathSeparator;
      return chosenDir.endsWith(separator)
          ? '$chosenDir$name'
          : '$chosenDir$separator$name';
    }
    return name;
  }

  String _normalizeOutputName(String value) {
    final trimmed = value.trim().isEmpty ? 'video.mp4' : value.trim();
    return trimmed.toLowerCase().endsWith('.mp4') ? trimmed : '$trimmed.mp4';
  }

  String _suggestFileName(MediaCandidate candidate) {
    final raw =
        candidate.title.trim().isEmpty ? 'video' : candidate.title.trim();
    final sanitized = raw.replaceAll(RegExp(r'[\\/:*?"<>|]'), '_');
    return _normalizeOutputName(sanitized);
  }

  RequestContext _requestContext() {
    return RequestContext(
      userAgent: _userAgentCtrl.text.trim(),
      referer: _refererCtrl.text.trim(),
      origin: _originCtrl.text.trim(),
      cookie: _cookieCtrl.text.trim(),
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
      final value = trimmed.substring(index + 1).trim();
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
      final inspection = await inspectMediaWithContext(
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

    try {
      await for (final event in downloadMediaWithContext(
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
        _pageController.updateDownloadTask(
          taskId,
          event.message,
          event.progress,
        );
        await MediaStoreBridge.updateForegroundProgress(
          (event.progress * 100).round(),
          event.message,
        );
      }

      if (!mounted) {
        return;
      }
      final outputFile = File(output);
      if (!await outputFile.exists() || await outputFile.length() == 0) {
        _pageController.failDownloadTask(
          taskId,
          l.text('output_not_created'),
          status: null,
          progress: 0,
        );
        return;
      }

      String finalPath = output;
      if (Platform.isAndroid) {
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
        try {
          if (await outputFile.exists()) {
            await outputFile.delete();
          }
        } catch (_) {}
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
      if (Platform.isAndroid && !vm.ignoringBattery)
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
        selectedCandidate: vm.selectedCandidate,
        inspection: vm.inspection,
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
