import 'dart:async';
import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:m3u8_downloader/src/app/auth_browser_page.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/app/app_theme.dart';
import 'package:m3u8_downloader/src/rust/api/downloader.dart';
import 'package:m3u8_downloader/src/rust/frb_generated.dart';
import 'package:permission_handler/permission_handler.dart';
import 'package:shared_preferences/shared_preferences.dart';

const _channel = MethodChannel('com.blue.ferrisload/media_store');

Future<void> _requestPermissions() async {
  if (!Platform.isAndroid) return;
  await [Permission.storage, Permission.manageExternalStorage].request();
  await Permission.notification.request();
}

Future<String> _getAppPrivateDir() async {
  if (!Platform.isAndroid) return '';
  try {
    return await _channel.invokeMethod<String>('getAppExternalFilesDir') ?? '';
  } catch (_) {
    return '';
  }
}

Future<String?> _saveViaMediaStore(String srcPath, String fileName,
    {String subDir = 'FerrisLoad'}) async {
  if (!Platform.isAndroid) return srcPath;
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

Future<String?> _saveToPath(
    String srcPath, String destDir, String fileName) async {
  if (!Platform.isAndroid) return srcPath;
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

Future<void> _startFgService() async {
  if (!Platform.isAndroid) return;
  try {
    await _channel.invokeMethod('startForegroundService');
  } catch (_) {}
}

Future<void> _stopFgService() async {
  if (!Platform.isAndroid) return;
  try {
    await _channel.invokeMethod('stopForegroundService');
  } catch (_) {}
}

Future<void> _updateFgProgress(int progress, String status) async {
  if (!Platform.isAndroid) return;
  try {
    await _channel.invokeMethod('updateServiceProgress', {
      'progress': progress,
      'status': status,
    });
  } catch (_) {}
}

Future<bool> _isIgnoringBattery() async {
  if (!Platform.isAndroid) return true;
  try {
    return await _channel
            .invokeMethod<bool>('isIgnoringBatteryOptimizations') ??
        false;
  } catch (_) {
    return false;
  }
}

Future<void> _requestIgnoreBattery() async {
  if (!Platform.isAndroid) return;
  try {
    await _channel.invokeMethod('requestIgnoreBatteryOptimizations');
  } catch (_) {}
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await _requestPermissions();
  final settings = await AppSettings.load();
  try {
    await RustLib.init();
  } catch (error) {
    debugPrint('RustLib.init() failed: $error');
  }
  runApp(FerrisLoadApp(initialSettings: settings));
}

class _FerrisScrollBehavior extends MaterialScrollBehavior {
  const _FerrisScrollBehavior();

  @override
  ScrollPhysics getScrollPhysics(BuildContext context) {
    return const BouncingScrollPhysics(
      parent: AlwaysScrollableScrollPhysics(),
    );
  }
}

class AppSettings {
  const AppSettings({
    required this.themeProfileId,
    required this.themeMode,
    required this.localeTag,
    required this.autoOpenAuthBrowser,
  });

  final String themeProfileId;
  final ThemeMode themeMode;
  final String localeTag;
  final bool autoOpenAuthBrowser;

  Locale get locale {
    final parts = localeTag.split('_');
    if (parts.length == 2) {
      return Locale.fromSubtags(languageCode: parts[0], scriptCode: parts[1]);
    }
    return Locale(parts.first);
  }

  AppThemeProfile get themeProfile {
    return appThemeProfiles.firstWhere(
      (profile) => profile.id == themeProfileId,
      orElse: () => appThemeProfiles.first,
    );
  }

  AppSettings copyWith({
    String? themeProfileId,
    ThemeMode? themeMode,
    String? localeTag,
    bool? autoOpenAuthBrowser,
  }) {
    return AppSettings(
      themeProfileId: themeProfileId ?? this.themeProfileId,
      themeMode: themeMode ?? this.themeMode,
      localeTag: localeTag ?? this.localeTag,
      autoOpenAuthBrowser: autoOpenAuthBrowser ?? this.autoOpenAuthBrowser,
    );
  }

  static Future<AppSettings> load() async {
    final prefs = await SharedPreferences.getInstance();
    final modeName = prefs.getString('themeMode') ?? ThemeMode.system.name;
    final themeProfileId =
        prefs.getString('themeProfileId') ?? appThemeProfiles.first.id;
    final localeTag = prefs.getString('localeTag') ?? 'zh';
    final autoOpenAuthBrowser = prefs.getBool('autoOpenAuthBrowser') ?? true;
    return AppSettings(
      themeProfileId: themeProfileId,
      themeMode: ThemeMode.values.firstWhere(
        (item) => item.name == modeName,
        orElse: () => ThemeMode.system,
      ),
      localeTag: localeTag,
      autoOpenAuthBrowser: autoOpenAuthBrowser,
    );
  }

  Future<void> save() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString('themeMode', themeMode.name);
    await prefs.setString('themeProfileId', themeProfileId);
    await prefs.setString('localeTag', localeTag);
    await prefs.setBool('autoOpenAuthBrowser', autoOpenAuthBrowser);
  }
}

class FerrisLoadApp extends StatefulWidget {
  const FerrisLoadApp({super.key, required this.initialSettings});

  final AppSettings initialSettings;

  @override
  State<FerrisLoadApp> createState() => _FerrisLoadAppState();
}

class _FerrisLoadAppState extends State<FerrisLoadApp> {
  late AppSettings _settings = widget.initialSettings;

  Future<void> _updateSettings(AppSettings next) async {
    setState(() => _settings = next);
    await next.save();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      title: 'FerrisLoad',
      scrollBehavior: const _FerrisScrollBehavior(),
      locale: _settings.locale,
      supportedLocales: AppLocalizations.supportedLocales,
      localizationsDelegates: const [
        AppLocalizations.delegate,
        GlobalMaterialLocalizations.delegate,
        GlobalWidgetsLocalizations.delegate,
        GlobalCupertinoLocalizations.delegate,
      ],
      themeMode: _settings.themeMode,
      theme: buildAppTheme(_settings.themeProfile, Brightness.light),
      darkTheme: buildAppTheme(_settings.themeProfile, Brightness.dark),
      home: HomePage(
        settings: _settings,
        onSettingsChanged: _updateSettings,
      ),
    );
  }
}

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
  late final Listenable _authContextListenable = Listenable.merge([
    _userAgentCtrl,
    _refererCtrl,
    _originCtrl,
    _cookieCtrl,
    _headersCtrl,
  ]);

  bool _keepTemp = false;
  bool _running = false;
  bool _analyzing = false;
  bool _ignoringBattery = false;
  bool _autoOpeningAuthBrowser = false;
  double _progress = 0;
  String? _status;
  String? _error;
  String? _chosenDir;
  String? _resultPath;
  MediaInspectionResult? _inspection;
  MediaCandidate? _selectedCandidate;
  final _history = <DownloadRecord>[];

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
    super.dispose();
  }

  Future<void> _checkBattery() async {
    final value = await _isIgnoringBattery();
    if (mounted) setState(() => _ignoringBattery = value);
  }

  Future<void> _pickDir() async {
    if (_running || _analyzing) return;
    final dir = await FilePicker.platform.getDirectoryPath();
    if (dir != null && mounted) {
      setState(() => _chosenDir = dir);
    }
  }

  Future<String> _tempOutputPath() async {
    final name = _normalizeOutputName(_fileNameCtrl.text.trim());
    if (Platform.isAndroid) {
      final safe = await _getAppPrivateDir();
      return safe.isNotEmpty ? '$safe${Platform.pathSeparator}$name' : name;
    }
    if (_chosenDir != null && _chosenDir!.isNotEmpty) {
      final separator = Platform.pathSeparator;
      return _chosenDir!.endsWith(separator)
          ? '$_chosenDir$name'
          : '$_chosenDir$separator$name';
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
      if (trimmed.isEmpty) continue;
      final index = trimmed.indexOf(':');
      if (index <= 0) continue;
      final name = trimmed.substring(0, index).trim();
      final value = trimmed.substring(index + 1).trim();
      if (name.isEmpty || value.isEmpty) continue;
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
    if (skipAutoAuth ||
        _autoOpeningAuthBrowser ||
        !widget.settings.autoOpenAuthBrowser) {
      return false;
    }
    if (inspection?.authRequired ?? false) {
      return true;
    }
    return errorText != null && _looksLikeAuthChallenge(errorText);
  }

  Future<void> _openAuthBrowser({bool reanalyzeAfterImport = false}) async {
    if (_autoOpeningAuthBrowser) return;
    final l = AppLocalizations.of(context);
    final targetUrl = _urlCtrl.text.trim();
    if (targetUrl.isEmpty) {
      setState(() => _error = l.text('input_source'));
      return;
    }

    _autoOpeningAuthBrowser = true;
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

      if (!mounted || session == null) return;

      setState(() {
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
        _status = l.text('auth_session_imported');
        _error = null;
      });

      if (reanalyzeAfterImport) {
        await _analyze(skipAutoAuth: true);
      }
    } finally {
      _autoOpeningAuthBrowser = false;
    }
  }

  Future<void> _analyze({bool skipAutoAuth = false}) async {
    if (!_formKey.currentState!.validate()) return;
    final l = AppLocalizations.of(context);
    final targetUrl = _urlCtrl.text.trim();
    final requestContext = _requestContext();
    var shouldAutoOpenAuthBrowser = false;

    setState(() {
      _analyzing = true;
      _error = null;
      _status = l.text('analyzing');
      _inspection = null;
      _selectedCandidate = null;
      _progress = 0.08;
      _resultPath = null;
    });

    try {
      final inspection = await inspectMediaWithContext(
        url: targetUrl,
        requestContext: requestContext,
      );
      if (!mounted) return;
      final selectedCandidate =
          inspection.candidates.isNotEmpty ? inspection.candidates.first : null;
      setState(() {
        _inspection = inspection;
        _selectedCandidate = selectedCandidate;
        _status = inspection.candidates.isNotEmpty
            ? l.text('ready')
            : l.text('no_candidates');
        _progress = inspection.candidates.isNotEmpty ? 0.16 : 0;
      });
      if (selectedCandidate != null) {
        _fileNameCtrl.text = _suggestFileName(selectedCandidate);
      }
      shouldAutoOpenAuthBrowser = _shouldAutoOpenAuthBrowser(
        skipAutoAuth: skipAutoAuth,
        inspection: inspection,
      );
      if (shouldAutoOpenAuthBrowser && mounted) {
        setState(() {
          _status = l.text('auth_redirecting');
          _error = null;
        });
      }
    } catch (error) {
      if (!mounted) return;
      final errorText = '$error';
      shouldAutoOpenAuthBrowser = _shouldAutoOpenAuthBrowser(
        skipAutoAuth: skipAutoAuth,
        errorText: errorText,
      );
      setState(() {
        _error = errorText;
        _status = shouldAutoOpenAuthBrowser ? l.text('auth_redirecting') : null;
        _progress = 0;
      });
    } finally {
      if (mounted) {
        setState(() => _analyzing = false);
      }
    }

    if (shouldAutoOpenAuthBrowser && mounted) {
      await _openAuthBrowser(reanalyzeAfterImport: true);
    }
  }

  Future<void> _download() async {
    if (!_formKey.currentState!.validate()) return;
    final l = AppLocalizations.of(context);
    final requestContext = _requestContext();
    var shouldAutoOpenAuthBrowser = false;

    final candidate = _selectedCandidate;
    final enteredUrl = _urlCtrl.text.trim();
    final String pageUrl;
    final String mediaUrl;
    final String? audioUrl;
    if (candidate != null) {
      pageUrl = candidate.pageUrl;
      mediaUrl = candidate.mediaUrl;
      audioUrl = candidate.audioUrl;
    } else {
      if (enteredUrl.isEmpty) {
        setState(() => _error = l.text('input_source'));
        return;
      }
      pageUrl = enteredUrl;
      mediaUrl = enteredUrl;
      audioUrl = null;
    }
    final sourcePage = pageUrl;

    final output = await _tempOutputPath();
    final fileName = _normalizeOutputName(_fileNameCtrl.text.trim());
    final concurrency = int.parse(_concurrencyCtrl.text.trim());
    final retries = int.parse(_retriesCtrl.text.trim());
    final vBitrate = int.parse(_vBitrateCtrl.text.trim());
    final aBitrate = int.parse(_aBitrateCtrl.text.trim());

    setState(() {
      _running = true;
      _error = null;
      _resultPath = null;
      _progress = 0.02;
      _status = l.text('preparing');
    });

    await _startFgService();

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
        keepTemp: _keepTemp,
        requestContext: requestContext,
      )) {
        if (!mounted) return;
        setState(() {
          _status = event.message;
          _progress = event.progress.clamp(0, 1);
        });
        await _updateFgProgress((event.progress * 100).round(), event.message);
      }

      if (!mounted) return;
      final outputFile = File(output);
      if (!await outputFile.exists() || await outputFile.length() == 0) {
        setState(() {
          _error = l.text('output_not_created');
          _status = null;
          _progress = 0;
        });
        return;
      }

      String finalPath = output;
      if (Platform.isAndroid) {
        String? savedPath;
        if (_chosenDir != null && _chosenDir!.isNotEmpty) {
          savedPath = await _saveToPath(output, _chosenDir!, fileName);
        } else {
          savedPath = await _saveViaMediaStore(output, fileName);
        }
        if (savedPath == null) {
          setState(() {
            _error = '${l.text('export_failed')} $output';
            _status = null;
          });
          return;
        }
        finalPath = savedPath;
        try {
          if (await outputFile.exists()) await outputFile.delete();
        } catch (_) {}
      }

      setState(() {
        _resultPath = finalPath;
        _status = l.text('completed');
        _progress = 1;
      });
      _history.insert(
        0,
        DownloadRecord(
          name: fileName,
          source: sourcePage,
          path: finalPath,
          time: DateTime.now(),
        ),
      );
    } catch (error) {
      if (!mounted) return;
      final errorText = '$error';
      shouldAutoOpenAuthBrowser = _shouldAutoOpenAuthBrowser(
        skipAutoAuth: false,
        errorText: errorText,
      );
      setState(() {
        _error = errorText;
        _status = shouldAutoOpenAuthBrowser ? l.text('auth_redirecting') : null;
        _progress = 0;
      });
    } finally {
      await _stopFgService();
      if (mounted) setState(() => _running = false);
    }

    if (shouldAutoOpenAuthBrowser && mounted) {
      await _openAuthBrowser(reanalyzeAfterImport: true);
    }
  }

  void _showSettingsSheet() {
    final l = AppLocalizations.of(context);
    showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      showDragHandle: true,
      useSafeArea: true,
      builder: (context) {
        var draft = widget.settings;
        return StatefulBuilder(
          builder: (context, setSheetState) {
            final t = Theme.of(context);
            final cs = t.colorScheme;
            final mode = draft.themeMode;
            final profile = draft.themeProfile;
            final locale = draft.locale;
            return Padding(
              padding: const EdgeInsets.fromLTRB(20, 0, 20, 28),
              child: SingleChildScrollView(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(l.text('appearance'), style: t.textTheme.titleLarge),
                    const SizedBox(height: 18),
                    _DisplayPreviewCard(
                      profile: profile,
                      mode: mode,
                      localeLabel: l.localeLabel(locale),
                    ),
                    const SizedBox(height: 18),
                    Text(l.text('mode'), style: t.textTheme.labelLarge),
                    const SizedBox(height: 10),
                    AnimatedContainer(
                      duration: const Duration(milliseconds: 240),
                      curve: Curves.easeOutCubic,
                      padding: const EdgeInsets.all(12),
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(24),
                        color: cs.surfaceContainerHigh.withValues(alpha: 0.52),
                      ),
                      child: SegmentedButton<ThemeMode>(
                        selected: {mode},
                        onSelectionChanged: (selection) {
                          final next = draft.copyWith(
                            themeMode: selection.first,
                          );
                          draft = next;
                          setSheetState(() {});
                          widget.onSettingsChanged(next);
                        },
                        segments: [
                          ButtonSegment(
                            value: ThemeMode.system,
                            label: Text(l.text('system')),
                            icon: const Icon(Icons.brightness_auto),
                          ),
                          ButtonSegment(
                            value: ThemeMode.light,
                            label: Text(l.text('light')),
                            icon: const Icon(Icons.light_mode),
                          ),
                          ButtonSegment(
                            value: ThemeMode.dark,
                            label: Text(l.text('dark')),
                            icon: const Icon(Icons.dark_mode),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(height: 20),
                    Text(l.text('theme'), style: t.textTheme.labelLarge),
                    const SizedBox(height: 10),
                    SizedBox(
                      height: 142,
                      child: ListView.separated(
                        scrollDirection: Axis.horizontal,
                        itemCount: appThemeProfiles.length,
                        separatorBuilder: (_, __) => const SizedBox(width: 12),
                        itemBuilder: (context, index) {
                          final item = appThemeProfiles[index];
                          return _ThemePaletteCard(
                            profile: item,
                            selected: profile.id == item.id,
                            onTap: () {
                              final next = draft.copyWith(
                                themeProfileId: item.id,
                              );
                              draft = next;
                              setSheetState(() {});
                              widget.onSettingsChanged(next);
                            },
                          );
                        },
                      ),
                    ),
                    const SizedBox(height: 14),
                    Row(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        for (final item in appThemeProfiles)
                          AnimatedContainer(
                            duration: const Duration(milliseconds: 220),
                            curve: Curves.easeOutCubic,
                            margin: const EdgeInsets.symmetric(horizontal: 3),
                            width: profile.id == item.id ? 20 : 6,
                            height: 6,
                            decoration: BoxDecoration(
                              borderRadius: BorderRadius.circular(999),
                              color: profile.id == item.id
                                  ? cs.primary
                                  : cs.outlineVariant,
                            ),
                          ),
                      ],
                    ),
                    const SizedBox(height: 20),
                    AnimatedContainer(
                      duration: const Duration(milliseconds: 240),
                      curve: Curves.easeOutCubic,
                      padding: const EdgeInsets.all(16),
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(24),
                        color: cs.surfaceContainerHigh.withValues(alpha: 0.52),
                      ),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(l.text('language'),
                              style: t.textTheme.labelLarge),
                          const SizedBox(height: 10),
                          DropdownMenu<Locale>(
                            initialSelection: locale,
                            width: double.infinity,
                            hintText: l.text('language'),
                            onSelected: (value) {
                              if (value == null) return;
                              final tag = value.scriptCode == null
                                  ? value.languageCode
                                  : '${value.languageCode}_${value.scriptCode}';
                              final next = draft.copyWith(localeTag: tag);
                              draft = next;
                              setSheetState(() {});
                              widget.onSettingsChanged(next);
                            },
                            dropdownMenuEntries: [
                              for (final item
                                  in AppLocalizations.supportedLocales)
                                DropdownMenuEntry(
                                  value: item,
                                  label: l.localeLabel(item),
                                ),
                            ],
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(height: 20),
                    _buildAuthSettingsSection(
                      context,
                      autoOpenAuthBrowser: draft.autoOpenAuthBrowser,
                      onAutoOpenChanged: (value) {
                        final next = draft.copyWith(
                          autoOpenAuthBrowser: value,
                        );
                        draft = next;
                        setSheetState(() {});
                        widget.onSettingsChanged(next);
                      },
                      onOpenBrowser: () async {
                        await _openAuthBrowser(
                          reanalyzeAfterImport:
                              _inspection?.authRequired ?? false,
                        );
                        setSheetState(() {});
                      },
                      onClearContext: () {
                        _clearAuthContext();
                        setSheetState(() {});
                      },
                    ),
                  ],
                ),
              ),
            );
          },
        );
      },
    );
  }

  @override
  Widget build(BuildContext context) {
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
      body: DecoratedBox(
        decoration: BoxDecoration(
          gradient: LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [
              Theme.of(context).brightness == Brightness.light
                  ? widget.settings.themeProfile.canvasLight
                  : cs.surfaceContainerLow,
              cs.surface,
              cs.surfaceContainerLowest,
            ],
          ),
        ),
        child: SafeArea(
          child: LayoutBuilder(
            builder: (context, constraints) {
              if (constraints.maxWidth >= 1080) {
                return Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Expanded(
                      flex: 7,
                      child: _RevealMotion(
                        delay: const Duration(milliseconds: 40),
                        child: _buildPrimaryPane(
                          context,
                          scrollable: true,
                          padding: const EdgeInsets.fromLTRB(16, 10, 16, 28),
                        ),
                      ),
                    ),
                    const SizedBox(width: 12),
                    SizedBox(
                      width: 360,
                      child: _RevealMotion(
                        delay: const Duration(milliseconds: 120),
                        offset: const Offset(0.06, 0),
                        child: _buildSidePane(
                          context,
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
                  _RevealMotion(
                    delay: const Duration(milliseconds: 40),
                    child: _buildPrimaryPane(
                      context,
                      scrollable: false,
                    ),
                  ),
                  const SizedBox(height: 12),
                  _RevealMotion(
                    delay: const Duration(milliseconds: 120),
                    child: _buildSidePane(
                      context,
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
  }

  Widget _buildPrimaryPane(
    BuildContext context, {
    required bool scrollable,
    EdgeInsetsGeometry padding = EdgeInsets.zero,
  }) {
    final l = AppLocalizations.of(context);
    final children = <Widget>[
      if (Platform.isAndroid && !_ignoringBattery)
        Padding(
          padding: const EdgeInsets.only(bottom: 12),
          child: _BatteryBanner(
            title: l.text('battery'),
            body: l.text('battery_hint'),
            buttonLabel: l.text('disable_now'),
            onRequest: () async {
              await _requestIgnoreBattery();
              Future.delayed(const Duration(seconds: 2), _checkBattery);
            },
          ),
        ),
      _buildInputCard(context),
      const SizedBox(height: 12),
      _buildCandidatesCard(context),
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
    required bool scrollable,
    EdgeInsetsGeometry padding = EdgeInsets.zero,
  }) {
    final children = <Widget>[
      _buildStatusCard(context),
      const SizedBox(height: 12),
      _buildHistoryCard(context),
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

  Widget _buildInputCard(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;
    return _SectionCard(
      title: l.text('input_source'),
      subtitle: l.text('app_subtitle'),
      icon: Icons.travel_explore_rounded,
      child: Form(
        key: _formKey,
        child: Column(
          children: [
            TextFormField(
              controller: _urlCtrl,
              enabled: !_running && !_analyzing,
              decoration: InputDecoration(
                labelText: l.text('input_source'),
                hintText: l.text('source_hint'),
                prefixIcon: const Icon(Icons.language_rounded),
              ),
              validator: (value) {
                if (value == null || value.trim().isEmpty) {
                  return l.text('input_source');
                }
                return null;
              },
            ),
            const SizedBox(height: 14),
            TextFormField(
              controller: _fileNameCtrl,
              enabled: !_running,
              decoration: InputDecoration(
                labelText: l.text('file_name'),
                prefixIcon: const Icon(Icons.video_file_outlined),
              ),
              validator: (value) {
                if (value == null || value.trim().isEmpty) {
                  return l.text('file_name');
                }
                return null;
              },
            ),
            const SizedBox(height: 14),
            InkWell(
              borderRadius: BorderRadius.circular(18),
              onTap: _running ? null : _pickDir,
              child: Ink(
                padding:
                    const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(18),
                  color: cs.surfaceContainerHigh.withValues(alpha: 0.55),
                  border: Border.all(color: cs.outlineVariant),
                ),
                child: Row(
                  children: [
                    Icon(Icons.folder_open_rounded, color: cs.primary),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Text(
                        _chosenDir ?? l.text('default_location'),
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: t.textTheme.bodySmall?.copyWith(
                          color: _chosenDir == null
                              ? cs.onSurfaceVariant
                              : cs.onSurface,
                        ),
                      ),
                    ),
                    if (_chosenDir != null)
                      IconButton(
                        onPressed: () => setState(() => _chosenDir = null),
                        icon: const Icon(Icons.restart_alt_rounded),
                      ),
                  ],
                ),
              ),
            ),
            const SizedBox(height: 16),
            LayoutBuilder(
              builder: (context, constraints) {
                final compact = constraints.maxWidth < 420;
                final analyzeButton = AnimatedScale(
                  duration: const Duration(milliseconds: 220),
                  curve: Curves.easeOutCubic,
                  scale: _analyzing ? 0.98 : 1,
                  child: OutlinedButton.icon(
                    onPressed: _running || _analyzing ? null : _analyze,
                    icon: _analyzing
                        ? const SizedBox(
                            width: 18,
                            height: 18,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.manage_search_rounded),
                    label: Text(l.text('analyze')),
                  ),
                );
                final downloadButton = AnimatedScale(
                  duration: const Duration(milliseconds: 220),
                  curve: Curves.easeOutCubic,
                  scale: _running ? 0.98 : 1,
                  child: FilledButton.icon(
                    onPressed: _running || _analyzing ? null : _download,
                    icon: _running
                        ? const SizedBox(
                            width: 18,
                            height: 18,
                            child: CircularProgressIndicator(
                              strokeWidth: 2,
                              color: Colors.white,
                            ),
                          )
                        : const Icon(Icons.download_rounded),
                    label: Text(l.text('download')),
                  ),
                );

                if (compact) {
                  return Column(
                    children: [
                      SizedBox(width: double.infinity, child: analyzeButton),
                      const SizedBox(height: 12),
                      SizedBox(width: double.infinity, child: downloadButton),
                    ],
                  );
                }

                return Row(
                  children: [
                    Expanded(child: analyzeButton),
                    const SizedBox(width: 12),
                    Expanded(child: downloadButton),
                  ],
                );
              },
            ),
            const SizedBox(height: 12),
            Row(
              children: [
                Icon(Icons.info_outline_rounded,
                    size: 16, color: cs.onSurfaceVariant),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    l.text('download_hint'),
                    style: t.textTheme.bodySmall?.copyWith(
                      color: cs.onSurfaceVariant,
                    ),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 18),
            Container(
              padding: const EdgeInsets.all(16),
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(22),
                color: cs.primaryContainer.withValues(alpha: 0.38),
              ),
              child: Column(
                children: [
                  Row(
                    children: [
                      Icon(Icons.tune_rounded, color: cs.primary),
                      const SizedBox(width: 8),
                      Text(l.text('download_options'),
                          style: t.textTheme.titleSmall?.copyWith(
                            color: cs.primary,
                            fontWeight: FontWeight.w700,
                          )),
                    ],
                  ),
                  const SizedBox(height: 14),
                  LayoutBuilder(
                    builder: (context, constraints) {
                      final compact = constraints.maxWidth < 420;
                      final concurrencyField = TextFormField(
                        controller: _concurrencyCtrl,
                        enabled: !_running,
                        keyboardType: TextInputType.number,
                        decoration: InputDecoration(
                          labelText: l.text('concurrency'),
                        ),
                        validator: (value) {
                          final number = int.tryParse(value?.trim() ?? '');
                          return number != null && number >= 1 && number <= 64
                              ? null
                              : '1-64';
                        },
                      );
                      final retriesField = TextFormField(
                        controller: _retriesCtrl,
                        enabled: !_running,
                        keyboardType: TextInputType.number,
                        decoration: InputDecoration(
                          labelText: l.text('retries'),
                        ),
                        validator: (value) {
                          final number = int.tryParse(value?.trim() ?? '');
                          return number != null && number >= 0 && number <= 10
                              ? null
                              : '0-10';
                        },
                      );

                      if (compact) {
                        return Column(
                          children: [
                            concurrencyField,
                            const SizedBox(height: 12),
                            retriesField,
                          ],
                        );
                      }

                      return Row(
                        children: [
                          Expanded(child: concurrencyField),
                          const SizedBox(width: 12),
                          Expanded(child: retriesField),
                        ],
                      );
                    },
                  ),
                  const SizedBox(height: 12),
                  LayoutBuilder(
                    builder: (context, constraints) {
                      final compact = constraints.maxWidth < 420;
                      final videoBitrateField = TextFormField(
                        controller: _vBitrateCtrl,
                        enabled: !_running,
                        keyboardType: TextInputType.number,
                        decoration: InputDecoration(
                          labelText: l.text('video_bitrate'),
                        ),
                        validator: (value) {
                          final number = int.tryParse(value?.trim() ?? '');
                          return number != null && number >= 0 ? null : '>=0';
                        },
                      );
                      final audioBitrateField = TextFormField(
                        controller: _aBitrateCtrl,
                        enabled: !_running,
                        keyboardType: TextInputType.number,
                        decoration: InputDecoration(
                          labelText: l.text('audio_bitrate'),
                        ),
                        validator: (value) {
                          final number = int.tryParse(value?.trim() ?? '');
                          return number != null && number >= 0 ? null : '>=0';
                        },
                      );

                      if (compact) {
                        return Column(
                          children: [
                            videoBitrateField,
                            const SizedBox(height: 12),
                            audioBitrateField,
                          ],
                        );
                      }

                      return Row(
                        children: [
                          Expanded(child: videoBitrateField),
                          const SizedBox(width: 12),
                          Expanded(child: audioBitrateField),
                        ],
                      );
                    },
                  ),
                  const SizedBox(height: 10),
                  SwitchListTile.adaptive(
                    value: _keepTemp,
                    contentPadding: EdgeInsets.zero,
                    title: Text(l.text('keep_temp')),
                    subtitle: Text(l.text('keep_temp_hint')),
                    onChanged: _running
                        ? null
                        : (value) => setState(() => _keepTemp = value),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildAuthSettingsSection(
    BuildContext context, {
    required bool autoOpenAuthBrowser,
    required ValueChanged<bool> onAutoOpenChanged,
    required Future<void> Function() onOpenBrowser,
    required VoidCallback onClearContext,
  }) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;
    final enabled = !_running && !_analyzing;

    return AnimatedContainer(
      duration: const Duration(milliseconds: 240),
      curve: Curves.easeOutCubic,
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(24),
        color: cs.surfaceContainerHigh.withValues(alpha: 0.52),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Container(
                width: 42,
                height: 42,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: cs.secondaryContainer.withValues(alpha: 0.72),
                ),
                child: Icon(
                  Icons.verified_user_outlined,
                  color: cs.secondary,
                ),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      l.text('authorization'),
                      style: t.textTheme.titleSmall?.copyWith(
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      l.text('authorization_hint'),
                      style: t.textTheme.bodySmall?.copyWith(
                        color: cs.onSurfaceVariant,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          SwitchListTile.adaptive(
            value: autoOpenAuthBrowser,
            contentPadding: EdgeInsets.zero,
            title: Text(l.text('auth_auto_open')),
            subtitle: Text(l.text('auth_auto_open_hint')),
            onChanged: enabled ? onAutoOpenChanged : null,
          ),
          const SizedBox(height: 4),
          AnimatedBuilder(
            animation: _authContextListenable,
            builder: (context, _) {
              final badges = _authContextBadges();
              if (badges.isEmpty) {
                return const SizedBox.shrink();
              }
              return Padding(
                padding: const EdgeInsets.only(bottom: 12),
                child: Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    for (final badge in badges) _MiniChip(label: badge),
                  ],
                ),
              );
            },
          ),
          LayoutBuilder(
            builder: (context, constraints) {
              final compact = constraints.maxWidth < 440;
              final browserButton = FilledButton.tonalIcon(
                onPressed: enabled ? () async => onOpenBrowser() : null,
                icon: const Icon(Icons.open_in_browser_rounded),
                label: Text(l.text('auth_browser_open')),
              );
              final clearButton = OutlinedButton.icon(
                onPressed:
                    enabled && _hasAuthContextOverrides ? onClearContext : null,
                icon: const Icon(Icons.restart_alt_rounded),
                label: Text(l.text('clear_auth_context')),
              );

              if (compact) {
                return Column(
                  children: [
                    SizedBox(width: double.infinity, child: browserButton),
                    const SizedBox(height: 12),
                    SizedBox(width: double.infinity, child: clearButton),
                  ],
                );
              }

              return Row(
                children: [
                  Expanded(child: browserButton),
                  const SizedBox(width: 12),
                  Expanded(child: clearButton),
                ],
              );
            },
          ),
          const SizedBox(height: 12),
          _buildAuthContextEditor(context, enabled: enabled),
          const SizedBox(height: 12),
          Text(
            l.text('auth_browser_hint_inline'),
            style: t.textTheme.bodySmall?.copyWith(
              color: cs.onSurfaceVariant,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildAuthContextEditor(
    BuildContext context, {
    required bool enabled,
  }) {
    final l = AppLocalizations.of(context);

    return Column(
      children: [
        TextFormField(
          controller: _userAgentCtrl,
          enabled: enabled,
          decoration: const InputDecoration(
            labelText: 'User-Agent',
            prefixIcon: Icon(Icons.devices_rounded),
          ),
        ),
        const SizedBox(height: 12),
        LayoutBuilder(
          builder: (context, constraints) {
            final compact = constraints.maxWidth < 460;
            if (compact) {
              return Column(
                children: [
                  TextFormField(
                    controller: _refererCtrl,
                    enabled: enabled,
                    decoration: const InputDecoration(
                      labelText: 'Referer',
                      prefixIcon: Icon(Icons.link_rounded),
                    ),
                  ),
                  const SizedBox(height: 12),
                  TextFormField(
                    controller: _originCtrl,
                    enabled: enabled,
                    decoration: const InputDecoration(
                      labelText: 'Origin',
                      prefixIcon: Icon(Icons.public_rounded),
                    ),
                  ),
                ],
              );
            }

            return Row(
              children: [
                Expanded(
                  child: TextFormField(
                    controller: _refererCtrl,
                    enabled: enabled,
                    decoration: const InputDecoration(
                      labelText: 'Referer',
                      prefixIcon: Icon(Icons.link_rounded),
                    ),
                  ),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: TextFormField(
                    controller: _originCtrl,
                    enabled: enabled,
                    decoration: const InputDecoration(
                      labelText: 'Origin',
                      prefixIcon: Icon(Icons.public_rounded),
                    ),
                  ),
                ),
              ],
            );
          },
        ),
        const SizedBox(height: 12),
        TextFormField(
          controller: _cookieCtrl,
          enabled: enabled,
          minLines: 2,
          maxLines: 4,
          decoration: const InputDecoration(
            labelText: 'Cookie',
            prefixIcon: Icon(Icons.cookie_outlined),
          ).copyWith(hintText: l.text('cookie_hint')),
        ),
        const SizedBox(height: 12),
        TextFormField(
          controller: _headersCtrl,
          enabled: enabled,
          minLines: 3,
          maxLines: 6,
          decoration: InputDecoration(
            labelText: 'Custom headers',
            hintText: l.text('headers_hint'),
            prefixIcon: const Icon(Icons.notes_rounded),
          ),
        ),
      ],
    );
  }

  Widget _buildCandidatesCard(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;
    return _SectionCard(
      title: l.text('analysis_results'),
      subtitle: _inspection == null
          ? l.text('no_candidates')
          : '${_inspection!.pageTitle} · ${_inspection!.candidates.length}',
      icon: Icons.video_collection_outlined,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (_inspection?.warnings.isNotEmpty ?? false)
            Padding(
              padding: const EdgeInsets.only(bottom: 14),
              child: Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [
                  for (final warning in _inspection!.warnings)
                    Chip(
                      avatar: Icon(Icons.warning_amber_rounded,
                          size: 18, color: cs.tertiary),
                      label: Text(warning),
                    ),
                ],
              ),
            ),
          if (_inspection?.authRequired ?? false)
            Padding(
              padding: const EdgeInsets.only(bottom: 14),
              child: Container(
                width: double.infinity,
                padding: const EdgeInsets.all(16),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(20),
                  color: cs.errorContainer,
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      l.text('auth_challenge_detected'),
                      style: t.textTheme.titleSmall?.copyWith(
                        color: cs.onErrorContainer,
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                    const SizedBox(height: 8),
                    Text(
                      _inspection!.challengeReason,
                      style: t.textTheme.bodyMedium?.copyWith(
                        color: cs.onErrorContainer,
                      ),
                    ),
                    const SizedBox(height: 8),
                    Text(
                      l.text('auth_challenge_help'),
                      style: t.textTheme.bodySmall?.copyWith(
                        color: cs.onErrorContainer,
                      ),
                    ),
                    const SizedBox(height: 12),
                    FilledButton.tonalIcon(
                      onPressed: _running || _analyzing
                          ? null
                          : () => _openAuthBrowser(reanalyzeAfterImport: true),
                      icon: const Icon(Icons.open_in_browser_rounded),
                      label: Text(l.text('auth_browser_open')),
                    ),
                  ],
                ),
              ),
            ),
          AnimatedSwitcher(
            duration: const Duration(milliseconds: 320),
            switchInCurve: Curves.easeOutCubic,
            switchOutCurve: Curves.easeInCubic,
            child: _inspection == null || _inspection!.candidates.isEmpty
                ? Container(
                    key: const ValueKey('empty-candidates'),
                    width: double.infinity,
                    padding: const EdgeInsets.all(20),
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(22),
                      color: cs.surfaceContainerHigh.withValues(alpha: 0.55),
                    ),
                    child: Text(
                      l.text('no_candidates'),
                      style: t.textTheme.bodyMedium?.copyWith(
                        color: cs.onSurfaceVariant,
                      ),
                    ),
                  )
                : Column(
                    key: ValueKey(
                        'candidate-list-${_inspection!.candidates.length}'),
                    children: [
                      for (final entry in _inspection!.candidates.indexed)
                        Padding(
                          padding: const EdgeInsets.only(bottom: 10),
                          child: _RevealMotion(
                            key: ValueKey(entry.$2.id),
                            delay: Duration(milliseconds: 28 * entry.$1),
                            child: _CandidateTile(
                              candidate: entry.$2,
                              selected: _selectedCandidate?.id == entry.$2.id,
                              onTap: () {
                                setState(() {
                                  _selectedCandidate = entry.$2;
                                  _fileNameCtrl.text =
                                      _suggestFileName(entry.$2);
                                });
                              },
                            ),
                          ),
                        ),
                    ],
                  ),
          ),
        ],
      ),
    );
  }

  Widget _buildStatusCard(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;
    return _SectionCard(
      title: l.text('status'),
      subtitle: _status ?? l.text('status'),
      icon: Icons.podcasts_rounded,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          ClipRRect(
            borderRadius: BorderRadius.circular(999),
            child: LinearProgressIndicator(
              minHeight: 8,
              value: (_running || _analyzing)
                  ? (_progress > 0 ? _progress : null)
                  : _progress.clamp(0.0, 1.0),
            ),
          ),
          const SizedBox(height: 14),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              _StatusBadge(
                label:
                    '${l.text('source_page')}: ${_selectedCandidate?.pageUrl ?? '-'}',
                color: cs.primaryContainer,
              ),
              _StatusBadge(
                label:
                    '${l.text('extractor')}: ${_selectedCandidate?.extractor ?? _inspection?.extractor ?? '-'}',
                color: cs.secondaryContainer,
              ),
              _StatusBadge(
                label:
                    '${l.text('stream')}: ${_selectedCandidate?.protocol ?? '-'}',
                color: cs.tertiaryContainer,
              ),
            ],
          ),
          AnimatedSwitcher(
            duration: const Duration(milliseconds: 260),
            switchInCurve: Curves.easeOutCubic,
            child: _status != null
                ? Padding(
                    key: ValueKey('status-${_status!}'),
                    padding: const EdgeInsets.only(top: 16),
                    child: Text(_status!, style: t.textTheme.bodyLarge),
                  )
                : const SizedBox.shrink(),
          ),
          AnimatedSwitcher(
            duration: const Duration(milliseconds: 260),
            switchInCurve: Curves.easeOutCubic,
            child: _error != null
                ? Container(
                    key: ValueKey('error-${_error!}'),
                    width: double.infinity,
                    margin: const EdgeInsets.only(top: 12),
                    padding: const EdgeInsets.all(14),
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(18),
                      color: cs.errorContainer,
                    ),
                    child: Text(
                      _error!,
                      style: t.textTheme.bodyMedium?.copyWith(
                        color: cs.onErrorContainer,
                      ),
                    ),
                  )
                : const SizedBox.shrink(),
          ),
          AnimatedSwitcher(
            duration: const Duration(milliseconds: 260),
            switchInCurve: Curves.easeOutCubic,
            child: _resultPath != null
                ? Container(
                    key: ValueKey('result-${_resultPath!}'),
                    width: double.infinity,
                    margin: const EdgeInsets.only(top: 12),
                    padding: const EdgeInsets.all(14),
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(18),
                      color: cs.primaryContainer.withValues(alpha: 0.45),
                    ),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(l.text('output_path'),
                            style: t.textTheme.labelLarge?.copyWith(
                              color: cs.primary,
                            )),
                        const SizedBox(height: 6),
                        SelectableText(_resultPath!),
                      ],
                    ),
                  )
                : const SizedBox.shrink(),
          ),
        ],
      ),
    );
  }

  Widget _buildHistoryCard(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;
    return _SectionCard(
      title: l.text('history'),
      subtitle: '${_history.length}',
      icon: Icons.history_rounded,
      child: AnimatedSwitcher(
        duration: const Duration(milliseconds: 320),
        switchInCurve: Curves.easeOutCubic,
        child: _history.isEmpty
            ? Text(
                key: const ValueKey('empty-history'),
                l.text('no_candidates'),
                style: t.textTheme.bodyMedium?.copyWith(
                  color: cs.onSurfaceVariant,
                ),
              )
            : Column(
                key: ValueKey('history-${_history.length}'),
                children: [
                  for (final entry in _history.take(6).indexed)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 10),
                      child: _RevealMotion(
                        key: ValueKey('${entry.$2.path}-${entry.$1}'),
                        delay: Duration(milliseconds: 34 * entry.$1),
                        child: Container(
                          width: double.infinity,
                          padding: const EdgeInsets.all(14),
                          decoration: BoxDecoration(
                            borderRadius: BorderRadius.circular(18),
                            color: cs.surfaceContainerHighest
                                .withValues(alpha: 0.45),
                          ),
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Text(entry.$2.name,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: t.textTheme.labelLarge?.copyWith(
                                    fontWeight: FontWeight.w700,
                                  )),
                              const SizedBox(height: 4),
                              Text(entry.$2.source,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: t.textTheme.bodySmall?.copyWith(
                                    color: cs.onSurfaceVariant,
                                  )),
                              const SizedBox(height: 4),
                              Text(entry.$2.path,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: t.textTheme.bodySmall),
                            ],
                          ),
                        ),
                      ),
                    ),
                ],
              ),
      ),
    );
  }
}

class _SectionCard extends StatelessWidget {
  const _SectionCard({
    required this.title,
    required this.subtitle,
    required this.icon,
    required this.child,
  });

  final String title;
  final String subtitle;
  final IconData icon;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final cs = t.colorScheme;
    return AnimatedContainer(
      duration: const Duration(milliseconds: 260),
      curve: Curves.easeOutCubic,
      child: Card(
        elevation: 0,
        shadowColor: Colors.transparent,
        child: Padding(
          padding: const EdgeInsets.all(20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Container(
                    width: 42,
                    height: 42,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: cs.primaryContainer.withValues(alpha: 0.55),
                    ),
                    child: Icon(icon, color: cs.primary),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(title,
                            style: t.textTheme.titleMedium?.copyWith(
                              fontWeight: FontWeight.w800,
                            )),
                        const SizedBox(height: 2),
                        Text(subtitle,
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: t.textTheme.bodySmall?.copyWith(
                              color: cs.onSurfaceVariant,
                            )),
                      ],
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 18),
              child,
            ],
          ),
        ),
      ),
    );
  }
}

class _CandidateTile extends StatelessWidget {
  const _CandidateTile({
    required this.candidate,
    required this.selected,
    required this.onTap,
  });

  final MediaCandidate candidate;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;
    return AnimatedScale(
      duration: const Duration(milliseconds: 240),
      curve: Curves.easeOutCubic,
      scale: selected ? 1 : 0.988,
      child: InkWell(
        borderRadius: BorderRadius.circular(22),
        onTap: onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 240),
          curve: Curves.easeOutCubic,
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(22),
            border: Border.all(
              color: selected ? cs.primary : cs.outlineVariant,
              width: selected ? 1.5 : 1,
            ),
            color: selected
                ? cs.primaryContainer.withValues(alpha: 0.46)
                : cs.surfaceContainerHigh.withValues(alpha: 0.32),
            boxShadow: [
              BoxShadow(
                color: selected
                    ? cs.primary.withValues(alpha: 0.16)
                    : cs.shadow.withValues(alpha: 0.05),
                blurRadius: selected ? 22 : 10,
                offset: Offset(0, selected ? 12 : 4),
              ),
            ],
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Expanded(
                    child: Text(
                      candidate.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: t.textTheme.titleSmall?.copyWith(
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                  ),
                  AnimatedSwitcher(
                    duration: const Duration(milliseconds: 220),
                    transitionBuilder: (child, animation) => ScaleTransition(
                      scale: animation,
                      child: FadeTransition(opacity: animation, child: child),
                    ),
                    child: selected
                        ? Icon(
                            Icons.check_circle_rounded,
                            key: const ValueKey('selected-check'),
                            color: cs.primary,
                          )
                        : const SizedBox(
                            key: ValueKey('selected-empty'), width: 24),
                  ),
                ],
              ),
              const SizedBox(height: 8),
              Wrap(
                spacing: 8,
                runSpacing: 8,
                children: [
                  _MiniChip(label: candidate.protocol.toUpperCase()),
                  if (candidate.primary) _MiniChip(label: l.text('core')),
                  _MiniChip(label: '${l.text('score')} ${candidate.score}'),
                  _MiniChip(label: candidate.qualityLabel),
                  _MiniChip(label: candidate.container.toUpperCase()),
                  if (candidate.width > 0 && candidate.height > 0)
                    _MiniChip(label: '${candidate.width}x${candidate.height}'),
                  if (candidate.durationSeconds > 0)
                    _MiniChip(label: '${candidate.durationSeconds.round()}s'),
                  if (candidate.segmentCount > 0)
                    _MiniChip(
                        label:
                            '${candidate.segmentCount} ${l.text('segments')}'),
                  if (candidate.audioUrl != null) _MiniChip(label: 'AV'),
                  if (candidate.requiresFfmpeg) _MiniChip(label: 'FFmpeg'),
                ],
              ),
              const SizedBox(height: 10),
              if (candidate.reason.isNotEmpty) ...[
                Text(candidate.reason,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: t.textTheme.bodySmall?.copyWith(
                      color: cs.primary,
                      fontWeight: FontWeight.w700,
                    )),
                const SizedBox(height: 8),
              ],
              Text(candidate.mediaUrl,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: t.textTheme.bodySmall?.copyWith(
                    color: cs.onSurfaceVariant,
                  )),
            ],
          ),
        ),
      ),
    );
  }
}

class _RevealMotion extends StatefulWidget {
  const _RevealMotion({
    super.key,
    required this.child,
    this.delay = Duration.zero,
    this.offset = const Offset(0, 0.04),
  });

  final Widget child;
  final Duration delay;
  final Offset offset;

  @override
  State<_RevealMotion> createState() => _RevealMotionState();
}

class _RevealMotionState extends State<_RevealMotion> {
  bool _visible = false;
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _timer = Timer(widget.delay, () {
      if (mounted) {
        setState(() => _visible = true);
      }
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedSlide(
      duration: const Duration(milliseconds: 420),
      curve: Curves.easeOutCubic,
      offset: _visible ? Offset.zero : widget.offset,
      child: AnimatedOpacity(
        duration: const Duration(milliseconds: 360),
        curve: Curves.easeOutCubic,
        opacity: _visible ? 1 : 0,
        child: widget.child,
      ),
    );
  }
}

class _MiniChip extends StatelessWidget {
  const _MiniChip({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final cs = t.colorScheme;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(999),
        color: cs.surfaceContainerHighest,
      ),
      child: Text(label,
          style: t.textTheme.labelSmall?.copyWith(fontWeight: FontWeight.w700)),
    );
  }
}

class _DisplayPreviewCard extends StatelessWidget {
  const _DisplayPreviewCard({
    required this.profile,
    required this.mode,
    required this.localeLabel,
  });

  final AppThemeProfile profile;
  final ThemeMode mode;
  final String localeLabel;

  @override
  Widget build(BuildContext context) {
    final brightness =
        mode == ThemeMode.dark ? Brightness.dark : Brightness.light;
    final previewTheme = buildAppTheme(profile, brightness);
    final cs = previewTheme.colorScheme;
    final isDark = brightness == Brightness.dark;

    return AnimatedContainer(
      duration: const Duration(milliseconds: 320),
      curve: Curves.easeOutCubic,
      height: 234,
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(30),
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            cs.surface,
            cs.primaryContainer.withValues(alpha: isDark ? 0.52 : 0.76),
            cs.secondaryContainer.withValues(alpha: isDark ? 0.46 : 0.72),
          ],
        ),
        boxShadow: [
          BoxShadow(
            color: cs.shadow.withValues(alpha: isDark ? 0.22 : 0.12),
            blurRadius: 28,
            offset: const Offset(0, 16),
          ),
        ],
      ),
      child: Stack(
        children: [
          Positioned(
            top: -24,
            left: -12,
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 320),
              width: 118,
              height: 118,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: cs.primary.withValues(alpha: isDark ? 0.16 : 0.22),
              ),
            ),
          ),
          Positioned(
            right: -10,
            top: 18,
            child: Transform.rotate(
              angle: -0.26,
              child: AnimatedContainer(
                duration: const Duration(milliseconds: 320),
                width: 126,
                height: 126,
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(34),
                  gradient: LinearGradient(
                    colors: [
                      cs.tertiaryContainer.withValues(alpha: 0.92),
                      cs.primary.withValues(alpha: 0.34),
                    ],
                  ),
                ),
              ),
            ),
          ),
          Positioned(
            left: 22,
            bottom: 72,
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 320),
              width: 84,
              height: 84,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                border: Border.all(
                  color: cs.onSurface.withValues(alpha: 0.18),
                  width: 10,
                ),
              ),
            ),
          ),
          Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(999),
                      color: cs.surface.withValues(alpha: 0.72),
                    ),
                    child: Text(
                      profile.name,
                      style: previewTheme.textTheme.labelLarge?.copyWith(
                        color: cs.onSurface,
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                  ),
                  const Spacer(),
                  AnimatedSwitcher(
                    duration: const Duration(milliseconds: 220),
                    child: Icon(
                      isDark
                          ? Icons.dark_mode_rounded
                          : Icons.light_mode_rounded,
                      key: ValueKey(isDark),
                      color: cs.onSurface,
                    ),
                  ),
                ],
              ),
              const Spacer(),
              Container(
                width: double.infinity,
                padding: const EdgeInsets.all(14),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(22),
                  color: cs.surface.withValues(alpha: isDark ? 0.72 : 0.84),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      'FerrisLoad',
                      style: previewTheme.textTheme.titleMedium?.copyWith(
                        color: cs.onSurface,
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                    const SizedBox(height: 4),
                    Text(
                      localeLabel,
                      style: previewTheme.textTheme.bodySmall?.copyWith(
                        color: cs.onSurfaceVariant,
                      ),
                    ),
                    const SizedBox(height: 12),
                    Row(
                      children: [
                        _PreviewMiniSwatch(color: profile.seed),
                        const SizedBox(width: 8),
                        _PreviewMiniSwatch(color: profile.accent),
                        const Spacer(),
                        Container(
                          padding: const EdgeInsets.symmetric(
                              horizontal: 10, vertical: 6),
                          decoration: BoxDecoration(
                            borderRadius: BorderRadius.circular(999),
                            color: cs.primaryContainer,
                          ),
                          child: Text(
                            isDark ? 'Dark' : 'Light',
                            style: previewTheme.textTheme.labelMedium?.copyWith(
                              color: cs.onPrimaryContainer,
                              fontWeight: FontWeight.w700,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ],
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _ThemePaletteCard extends StatelessWidget {
  const _ThemePaletteCard({
    required this.profile,
    required this.selected,
    required this.onTap,
  });

  final AppThemeProfile profile;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final cs = t.colorScheme;
    return AnimatedScale(
      duration: const Duration(milliseconds: 220),
      curve: Curves.easeOutCubic,
      scale: selected ? 1 : 0.97,
      child: InkWell(
        borderRadius: BorderRadius.circular(24),
        onTap: onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 220),
          curve: Curves.easeOutCubic,
          width: 172,
          padding: const EdgeInsets.all(14),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(24),
            color: selected
                ? cs.primaryContainer.withValues(alpha: 0.54)
                : cs.surfaceContainerHigh.withValues(alpha: 0.5),
            border: Border.all(
              color: selected ? cs.primary : cs.outlineVariant,
              width: selected ? 1.6 : 1,
            ),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Container(
                height: 58,
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(18),
                  gradient: LinearGradient(
                    colors: [
                      profile.canvasLight,
                      profile.seed.withValues(alpha: 0.88),
                      profile.accent.withValues(alpha: 0.92),
                    ],
                  ),
                ),
                child: Row(
                  children: [
                    const SizedBox(width: 12),
                    _ThemeDot(color: profile.seed),
                    const SizedBox(width: 8),
                    _ThemeDot(color: profile.accent),
                    const Spacer(),
                    AnimatedSwitcher(
                      duration: const Duration(milliseconds: 200),
                      child: selected
                          ? Icon(
                              Icons.check_circle_rounded,
                              key: ValueKey(profile.id),
                              color: Colors.white.withValues(alpha: 0.95),
                            )
                          : const SizedBox(width: 24),
                    ),
                    const SizedBox(width: 10),
                  ],
                ),
              ),
              const SizedBox(height: 12),
              Text(
                profile.name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: t.textTheme.labelLarge
                    ?.copyWith(fontWeight: FontWeight.w800),
              ),
              const SizedBox(height: 4),
              Text(
                profile.description,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style:
                    t.textTheme.bodySmall?.copyWith(color: cs.onSurfaceVariant),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _PreviewMiniSwatch extends StatelessWidget {
  const _PreviewMiniSwatch({required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 18,
      height: 18,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: color,
      ),
    );
  }
}

class _ThemeDot extends StatelessWidget {
  const _ThemeDot({required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 18,
      height: 18,
      decoration: BoxDecoration(color: color, shape: BoxShape.circle),
    );
  }
}

class _StatusBadge extends StatelessWidget {
  const _StatusBadge({required this.label, required this.color});

  final String label;
  final Color color;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(999),
        color: color,
      ),
      child: Text(label, maxLines: 1, overflow: TextOverflow.ellipsis),
    );
  }
}

class _BatteryBanner extends StatelessWidget {
  const _BatteryBanner({
    required this.title,
    required this.body,
    required this.buttonLabel,
    required this.onRequest,
  });

  final String title;
  final String body;
  final String buttonLabel;
  final VoidCallback onRequest;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final cs = t.colorScheme;
    return Card(
      color: cs.tertiaryContainer,
      child: Padding(
        padding: const EdgeInsets.all(18),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(Icons.battery_saver_rounded,
                    color: cs.onTertiaryContainer),
                const SizedBox(width: 8),
                Text(title,
                    style: t.textTheme.titleSmall?.copyWith(
                      color: cs.onTertiaryContainer,
                      fontWeight: FontWeight.w700,
                    )),
              ],
            ),
            const SizedBox(height: 8),
            Text(body,
                style: t.textTheme.bodyMedium?.copyWith(
                  color: cs.onTertiaryContainer,
                )),
            const SizedBox(height: 14),
            Align(
              alignment: Alignment.centerRight,
              child: FilledButton.tonal(
                onPressed: onRequest,
                child: Text(buttonLabel),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class DownloadRecord {
  const DownloadRecord({
    required this.name,
    required this.source,
    required this.path,
    required this.time,
  });

  final String name;
  final String source;
  final String path;
  final DateTime time;
}
