import 'dart:async';
import 'dart:io';

import 'package:file_picker/file_picker.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:m3u8_downloader/src/rust/api/downloader.dart';
import 'package:m3u8_downloader/src/rust/frb_generated.dart';
import 'package:permission_handler/permission_handler.dart';

// ─── Platform Channel ───────────────────────────────────────────────────────
const _channel = MethodChannel('com.blue.ferrisload/media_store');

Future<void> _requestPermissions() async {
  if (!Platform.isAndroid) return;
  await [Permission.storage, Permission.manageExternalStorage].request();
  if (Platform.isAndroid) {
    // Android 13+ 需要通知权限
    await Permission.notification.request();
  }
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
  } catch (e) {
    debugPrint('saveToDownloads failed: $e');
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
  } catch (e) {
    debugPrint('saveToPath failed: $e');
    return null;
  }
}

// ── 前台服务 ──
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

// ── 电池优化 ──
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

// ─── Entry ──────────────────────────────────────────────────────────────────
Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  _requestPermissions();
  try {
    await RustLib.init();
  } catch (e) {
    debugPrint('⚠️ RustLib.init() failed: $e');
    // App can still launch; download functionality will fail gracefully
  }
  runApp(const FerrisLoadApp());
}

// ─── App Root ───────────────────────────────────────────────────────────────
class FerrisLoadApp extends StatefulWidget {
  const FerrisLoadApp({super.key});
  @override
  State<FerrisLoadApp> createState() => _FerrisLoadAppState();
}

class _FerrisLoadAppState extends State<FerrisLoadApp> {
  // 默认绿色主题
  Color _seed = const Color(0xFF2E7D32);
  ThemeMode _mode = ThemeMode.system;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      title: 'FerrisLoad',
      themeMode: _mode,
      theme: _buildTheme(Brightness.light),
      darkTheme: _buildTheme(Brightness.dark),
      home: HomePage(
        seed: _seed,
        mode: _mode,
        onSeedChanged: (c) => setState(() => _seed = c),
        onModeChanged: (m) => setState(() => _mode = m),
      ),
    );
  }

  ThemeData _buildTheme(Brightness brightness) {
    final scheme =
        ColorScheme.fromSeed(seedColor: _seed, brightness: brightness);
    return ThemeData(
      useMaterial3: true,
      colorScheme: scheme,
      pageTransitionsTheme: const PageTransitionsTheme(
        builders: {
          TargetPlatform.android: PredictiveBackPageTransitionsBuilder(),
          TargetPlatform.iOS: CupertinoPageTransitionsBuilder(),
        },
      ),
      inputDecorationTheme: InputDecorationTheme(
        border: OutlineInputBorder(borderRadius: BorderRadius.circular(12)),
        filled: true,
        isDense: true,
      ),
      cardTheme: CardThemeData(
        elevation: 0,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
        color: scheme.surfaceContainerLow,
      ),
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          minimumSize: const Size(double.infinity, 52),
          shape:
              RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
          textStyle: const TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
        ),
      ),
      segmentedButtonTheme: SegmentedButtonThemeData(
        style: ButtonStyle(
          shape: WidgetStatePropertyAll(
            RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
          ),
        ),
      ),
    );
  }
}

// ─── Home Page ──────────────────────────────────────────────────────────────
class HomePage extends StatefulWidget {
  const HomePage({
    super.key,
    required this.seed,
    required this.mode,
    required this.onSeedChanged,
    required this.onModeChanged,
  });
  final Color seed;
  final ThemeMode mode;
  final ValueChanged<Color> onSeedChanged;
  final ValueChanged<ThemeMode> onModeChanged;

  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage> with TickerProviderStateMixin {
  final _formKey = GlobalKey<FormState>();
  final _urlCtrl = TextEditingController();
  final _fileNameCtrl = TextEditingController(text: 'output.mp4');
  final _concurrencyCtrl = TextEditingController(text: '8');
  final _retriesCtrl = TextEditingController(text: '3');
  final _vBitrateCtrl = TextEditingController(text: '0');
  final _aBitrateCtrl = TextEditingController(text: '0');

  bool _keepTemp = false;
  bool _running = false;
  double _progress = 0;
  String? _status;
  String? _error;
  String? _resultPath;
  String? _chosenDir;
  bool _ignoringBattery = false;

  final _history = <_Record>[];

  // 动画控制器
  late final AnimationController _progressAnim = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 400),
  );
  late final AnimationController _resultAnim = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 500),
  );

  @override
  void initState() {
    super.initState();
    _checkBattery();
  }

  Future<void> _checkBattery() async {
    final v = await _isIgnoringBattery();
    if (mounted) setState(() => _ignoringBattery = v);
  }

  @override
  void dispose() {
    _urlCtrl.dispose();
    _fileNameCtrl.dispose();
    _concurrencyCtrl.dispose();
    _retriesCtrl.dispose();
    _vBitrateCtrl.dispose();
    _aBitrateCtrl.dispose();
    _progressAnim.dispose();
    _resultAnim.dispose();
    super.dispose();
  }

  Future<void> _pickDir() async {
    if (_running) return;
    final dir =
        await FilePicker.platform.getDirectoryPath(dialogTitle: '选择保存目录');
    if (dir != null && mounted) setState(() => _chosenDir = dir);
  }

  Future<String> _tempOutputPath() async {
    final name = _fileNameCtrl.text.trim();
    if (Platform.isAndroid) {
      final safe = await _getAppPrivateDir();
      return safe.isNotEmpty ? '$safe${Platform.pathSeparator}$name' : name;
    }
    if (_chosenDir != null && _chosenDir!.isNotEmpty) {
      final sep = Platform.pathSeparator;
      return _chosenDir!.endsWith(sep)
          ? '$_chosenDir$name'
          : '$_chosenDir$sep$name';
    }
    return name;
  }

  // ── 下载 ──
  Future<void> _download() async {
    if (!_formKey.currentState!.validate()) return;

    final url = _urlCtrl.text.trim();
    final fileName = _fileNameCtrl.text.trim();
    final output = await _tempOutputPath();
    final concurrency = int.parse(_concurrencyCtrl.text.trim());
    final retries = int.parse(_retriesCtrl.text.trim());
    final vBitrate = int.parse(_vBitrateCtrl.text.trim());
    final aBitrate = int.parse(_aBitrateCtrl.text.trim());

    setState(() {
      _running = true;
      _progress = 0;
      _status = '正在初始化…';
      _error = null;
      _resultPath = null;
    });
    _progressAnim.forward();
    _resultAnim.reset();

    // 启动前台服务
    await _startFgService();

    try {
      await for (final e in hls2Mp4Run(
        url: url,
        concurrency: concurrency,
        output: output,
        retries: retries,
        videoBitrate: vBitrate,
        audioBitrate: aBitrate,
        keepTemp: _keepTemp,
      )) {
        if (!mounted) return;
        setState(() {
          _status = e.message;
          _progress = e.progress;
        });
        await _updateFgProgress((e.progress * 100).round(), e.message);
      }
      if (!mounted) return;

      // 验证输出
      final outputFile = File(output);
      if (!await outputFile.exists() || await outputFile.length() == 0) {
        setState(() {
          _error = '转码失败：输出文件为空或不存在，请重试';
          _status = null;
        });
        return;
      }

      // 保存到用户可见位置
      String finalPath = output;
      if (Platform.isAndroid) {
        setState(() => _status = '正在保存文件…');
        await _updateFgProgress(98, '正在保存文件…');
        String? savedPath;
        if (_chosenDir != null && _chosenDir!.isNotEmpty) {
          savedPath = await _saveToPath(output, _chosenDir!, fileName);
        } else {
          savedPath = await _saveViaMediaStore(output, fileName);
        }
        if (savedPath == null) {
          setState(() {
            _error = '文件保存失败，转码文件保留在：$output';
            _status = null;
          });
          return;
        }
        finalPath = savedPath;
        try {
          final f = File(output);
          if (await f.exists()) await f.delete();
        } catch (_) {}
      }

      setState(() {
        _status = '下载完成';
        _resultPath = finalPath;
        _progress = 1;
      });
      _resultAnim.forward();
      _history.insert(
          0, _Record(name: fileName, path: finalPath, time: DateTime.now()));
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = '$e';
        _status = null;
      });
    } finally {
      await _stopFgService();
      if (mounted) {
        setState(() => _running = false);
        _progressAnim.reverse();
      }
    }
  }

  // ── 主题设置 ──
  void _showThemeSheet() {
    showModalBottomSheet(
      context: context,
      showDragHandle: true,
      useSafeArea: true,
      isScrollControlled: true,
      builder: (ctx) {
        const colors = <Color>[
          Color(0xFF2E7D32), // 绿色（默认）
          Color(0xFF1565C0), // 蓝色
          Color(0xFF6750A4), // 紫色
          Color(0xFF00695C), // 青色
          Color(0xFFC62828), // 红色
          Color(0xFFE65100), // 橙色
          Color(0xFF4E342E), // 棕色
          Color(0xFF37474F), // 灰蓝
        ];
        var mode = widget.mode;
        return StatefulBuilder(builder: (ctx, set) {
          final t = Theme.of(ctx);
          return Padding(
            padding: const EdgeInsets.fromLTRB(24, 0, 24, 32),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('外观设置', style: t.textTheme.titleLarge),
                const SizedBox(height: 20),
                Text('模式', style: t.textTheme.labelLarge),
                const SizedBox(height: 8),
                SegmentedButton<ThemeMode>(
                  segments: const [
                    ButtonSegment(
                        value: ThemeMode.system,
                        label: Text('系统'),
                        icon: Icon(Icons.brightness_auto)),
                    ButtonSegment(
                        value: ThemeMode.light,
                        label: Text('浅色'),
                        icon: Icon(Icons.light_mode)),
                    ButtonSegment(
                        value: ThemeMode.dark,
                        label: Text('深色'),
                        icon: Icon(Icons.dark_mode)),
                  ],
                  selected: {mode},
                  onSelectionChanged: (s) {
                    set(() => mode = s.first);
                    widget.onModeChanged(s.first);
                  },
                ),
                const SizedBox(height: 20),
                Text('主题色', style: t.textTheme.labelLarge),
                const SizedBox(height: 8),
                Wrap(
                  spacing: 10,
                  runSpacing: 10,
                  children: [
                    for (final c in colors)
                      GestureDetector(
                        onTap: () {
                          widget.onSeedChanged(c);
                          set(() {});
                        },
                        child: AnimatedContainer(
                          duration: const Duration(milliseconds: 250),
                          curve: Curves.easeOutCubic,
                          width: 44,
                          height: 44,
                          decoration: BoxDecoration(
                            color: c,
                            shape: BoxShape.circle,
                            border: Border.all(
                              color: widget.seed == c
                                  ? t.colorScheme.onSurface
                                  : Colors.transparent,
                              width: 3,
                            ),
                            boxShadow: widget.seed == c
                                ? [
                                    BoxShadow(
                                        color: c.withValues(alpha: 0.4),
                                        blurRadius: 8,
                                        spreadRadius: 1),
                                  ]
                                : null,
                          ),
                          child: AnimatedSwitcher(
                            duration: const Duration(milliseconds: 200),
                            child: widget.seed == c
                                ? const Icon(Icons.check,
                                    color: Colors.white,
                                    size: 20,
                                    key: ValueKey('check'))
                                : const SizedBox.shrink(key: ValueKey('empty')),
                          ),
                        ),
                      ),
                  ],
                ),
              ],
            ),
          );
        });
      },
    );
  }

  // ── Build ──
  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final cs = t.colorScheme;

    return Scaffold(
      appBar: AppBar(
        title: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.downloading_rounded, color: cs.primary),
            const SizedBox(width: 8),
            const Text('FerrisLoad'),
          ],
        ),
        actions: [
          IconButton(
            icon: const Icon(Icons.palette_outlined),
            tooltip: '外观',
            onPressed: _showThemeSheet,
          ),
        ],
      ),
      body: SafeArea(
        child: LayoutBuilder(builder: (context, box) {
          if (box.maxWidth >= 840) {
            return Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(flex: 5, child: _form(t, cs)),
                VerticalDivider(width: 1, color: cs.outlineVariant),
                Expanded(flex: 3, child: _statusPanel(t, cs)),
              ],
            );
          }
          return _form(t, cs, showStatus: true);
        }),
      ),
    );
  }

  // ═══════════════════════════════════════════════════════════════════════════
  //  FORM
  // ═══════════════════════════════════════════════════════════════════════════
  Widget _form(ThemeData t, ColorScheme cs, {bool showStatus = false}) {
    return Form(
      key: _formKey,
      child: ListView(
        padding: const EdgeInsets.fromLTRB(16, 8, 16, 32),
        children: [
          // ── 电池优化提示 ──
          if (Platform.isAndroid && !_ignoringBattery)
            _BatteryBanner(
              onRequest: () async {
                await _requestIgnoreBattery();
                // 延迟检查，等用户操作完
                Future.delayed(const Duration(seconds: 2), _checkBattery);
              },
            ),

          // ── URL & 文件名 ──
          _Section(icon: Icons.link_rounded, title: '下载源', children: [
            TextFormField(
              controller: _urlCtrl,
              enabled: !_running,
              maxLines: 1,
              decoration: const InputDecoration(
                labelText: 'M3U8 地址',
                hintText: 'https://example.com/index.m3u8',
                prefixIcon: Icon(Icons.language),
              ),
              validator: (v) =>
                  (v == null || v.trim().isEmpty) ? '请输入地址' : null,
            ),
            const SizedBox(height: 12),
            TextFormField(
              controller: _fileNameCtrl,
              enabled: !_running,
              decoration: const InputDecoration(
                labelText: '输出文件名',
                hintText: 'output.mp4',
                prefixIcon: Icon(Icons.insert_drive_file_outlined),
              ),
              validator: (v) {
                if (v == null || v.trim().isEmpty) return '请输入文件名';
                if (!v.trim().toLowerCase().endsWith('.mp4'))
                  return '需以 .mp4 结尾';
                return null;
              },
            ),
          ]),

          // ── 保存位置 ──
          _Section(icon: Icons.folder_outlined, title: '保存位置', children: [
            Material(
              color: Colors.transparent,
              child: InkWell(
                borderRadius: BorderRadius.circular(12),
                onTap: _running ? null : _pickDir,
                child: AnimatedContainer(
                  duration: const Duration(milliseconds: 200),
                  width: double.infinity,
                  padding:
                      const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
                  decoration: BoxDecoration(
                    border: Border.all(color: cs.outline),
                    borderRadius: BorderRadius.circular(12),
                    color: cs.surfaceContainerHighest.withValues(alpha: 0.35),
                  ),
                  child: Row(children: [
                    Icon(Icons.folder_open, color: cs.primary, size: 22),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Text(
                        _chosenDir ??
                            (Platform.isAndroid
                                ? '默认：内部存储/Download/FerrisLoad'
                                : '点击选择目录'),
                        style: t.textTheme.bodyMedium?.copyWith(
                          color: _chosenDir != null
                              ? cs.onSurface
                              : cs.onSurfaceVariant,
                        ),
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                    Icon(Icons.chevron_right, color: cs.onSurfaceVariant),
                  ]),
                ),
              ),
            ),
            if (_chosenDir != null)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: Align(
                  alignment: Alignment.centerRight,
                  child: TextButton.icon(
                    onPressed: _running
                        ? null
                        : () => setState(() => _chosenDir = null),
                    icon: const Icon(Icons.restart_alt, size: 18),
                    label: const Text('恢复默认'),
                  ),
                ),
              ),
          ]),

          // ── 参数 ──
          _Section(icon: Icons.tune, title: '参数', children: [
            Row(children: [
              Expanded(
                  child: TextFormField(
                controller: _concurrencyCtrl,
                enabled: !_running,
                keyboardType: TextInputType.number,
                decoration:
                    const InputDecoration(labelText: '并发', helperText: '1–64'),
                validator: (v) {
                  final n = int.tryParse(v?.trim() ?? '');
                  return (n != null && n >= 1 && n <= 64) ? null : '1–64';
                },
              )),
              const SizedBox(width: 12),
              Expanded(
                  child: TextFormField(
                controller: _retriesCtrl,
                enabled: !_running,
                keyboardType: TextInputType.number,
                decoration:
                    const InputDecoration(labelText: '重试', helperText: '0–10'),
                validator: (v) {
                  final n = int.tryParse(v?.trim() ?? '');
                  return (n != null && n >= 0 && n <= 10) ? null : '0–10';
                },
              )),
            ]),
            const SizedBox(height: 12),
            Row(children: [
              Expanded(
                  child: TextFormField(
                controller: _vBitrateCtrl,
                enabled: !_running,
                keyboardType: TextInputType.number,
                decoration: const InputDecoration(
                    labelText: '视频码率 kbps', helperText: '0=自动'),
                validator: (v) {
                  final n = int.tryParse(v?.trim() ?? '');
                  return (n != null && n >= 0) ? null : '≥0';
                },
              )),
              const SizedBox(width: 12),
              Expanded(
                  child: TextFormField(
                controller: _aBitrateCtrl,
                enabled: !_running,
                keyboardType: TextInputType.number,
                decoration: const InputDecoration(
                    labelText: '音频码率 kbps', helperText: '0=自动'),
                validator: (v) {
                  final n = int.tryParse(v?.trim() ?? '');
                  return (n != null && n >= 0) ? null : '≥0';
                },
              )),
            ]),
            const SizedBox(height: 4),
            SwitchListTile.adaptive(
              contentPadding: EdgeInsets.zero,
              title: const Text('保留临时 TS 文件'),
              subtitle: const Text('调试用'),
              value: _keepTemp,
              onChanged: _running ? null : (v) => setState(() => _keepTemp = v),
            ),
          ]),

          const SizedBox(height: 12),

          // ── 进度 ──
          AnimatedSize(
            duration: const Duration(milliseconds: 300),
            curve: Curves.easeOutCubic,
            child: _running
                ? Column(children: [
                    ClipRRect(
                      borderRadius: BorderRadius.circular(6),
                      child: TweenAnimationBuilder<double>(
                        tween: Tween(begin: 0, end: _progress),
                        duration: const Duration(milliseconds: 400),
                        curve: Curves.easeOutCubic,
                        builder: (_, v, __) => LinearProgressIndicator(
                          value: v > 0 ? v : null,
                          minHeight: 6,
                        ),
                      ),
                    ),
                    if (_status != null)
                      Padding(
                        padding: const EdgeInsets.only(top: 6, bottom: 4),
                        child: Text(
                          _status!,
                          style: t.textTheme.labelMedium
                              ?.copyWith(color: cs.primary),
                          textAlign: TextAlign.center,
                        ),
                      ),
                    const SizedBox(height: 8),
                  ])
                : const SizedBox.shrink(),
          ),

          // ── 按钮 ──
          FilledButton.icon(
            onPressed: _running ? null : _download,
            icon: _running
                ? SizedBox(
                    width: 20,
                    height: 20,
                    child: CircularProgressIndicator(
                        strokeWidth: 2.5, color: cs.onPrimary))
                : const Icon(Icons.download_rounded),
            label: AnimatedSwitcher(
              duration: const Duration(milliseconds: 200),
              child: Text(_running ? '下载中…' : '开始下载', key: ValueKey(_running)),
            ),
          ),

          // ── 内嵌状态（窄屏） ──
          if (showStatus) ...[
            const SizedBox(height: 20),
            _statusPanel(t, cs, embedded: true),
          ],
        ],
      ),
    );
  }

  // ═══════════════════════════════════════════════════════════════════════════
  //  STATUS PANEL
  // ═══════════════════════════════════════════════════════════════════════════
  Widget _statusPanel(ThemeData t, ColorScheme cs, {bool embedded = false}) {
    final content = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        // ── 状态卡片 ──
        Card(
          margin: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
          child: Padding(
            padding: const EdgeInsets.all(20),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Row(children: [
                  Icon(Icons.info_outline, color: cs.primary, size: 20),
                  const SizedBox(width: 8),
                  Text('状态',
                      style:
                          t.textTheme.titleSmall?.copyWith(color: cs.primary)),
                ]),
                const SizedBox(height: 12),

                // 错误
                if (_error != null)
                  _AnimatedStatusRow(
                    icon: Icons.error_outline,
                    color: cs.error,
                    label: _error!,
                    t: t,
                  ),

                // 完成
                if (_resultPath != null) ...[
                  _AnimatedStatusRow(
                    icon: Icons.check_circle_outline,
                    color: cs.primary,
                    label: '下载完成',
                    t: t,
                  ),
                  const SizedBox(height: 8),
                  AnimatedOpacity(
                    opacity: _resultPath != null ? 1 : 0,
                    duration: const Duration(milliseconds: 400),
                    child: Container(
                      width: double.infinity,
                      padding: const EdgeInsets.all(12),
                      decoration: BoxDecoration(
                        color: cs.primaryContainer.withValues(alpha: 0.3),
                        borderRadius: BorderRadius.circular(10),
                      ),
                      child: SelectableText(
                        _resultPath!,
                        style: t.textTheme.bodySmall?.copyWith(
                          fontFamily: 'monospace',
                          color: cs.onPrimaryContainer,
                        ),
                      ),
                    ),
                  ),
                ],

                // 空闲
                if (_error == null && _resultPath == null && !_running)
                  Text('等待任务开始',
                      style: t.textTheme.bodyMedium
                          ?.copyWith(color: cs.onSurfaceVariant)),

                // 运行中
                if (_running && _status != null)
                  _AnimatedStatusRow(
                    icon: Icons.sync,
                    color: cs.tertiary,
                    label: _status!,
                    t: t,
                  ),
              ],
            ),
          ),
        ),

        // ── 历史 ──
        AnimatedSize(
          duration: const Duration(milliseconds: 300),
          curve: Curves.easeOutCubic,
          child: _history.isNotEmpty
              ? Card(
                  margin:
                      const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
                  child: Padding(
                    padding: const EdgeInsets.all(20),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Row(children: [
                          Icon(Icons.history, color: cs.primary, size: 20),
                          const SizedBox(width: 8),
                          Text('历史',
                              style: t.textTheme.titleSmall
                                  ?.copyWith(color: cs.primary)),
                        ]),
                        const Divider(height: 20),
                        for (final r in _history.take(5))
                          Padding(
                            padding: const EdgeInsets.only(bottom: 10),
                            child: Row(children: [
                              Icon(Icons.video_file_outlined,
                                  size: 18, color: cs.onSurfaceVariant),
                              const SizedBox(width: 10),
                              Expanded(
                                  child: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  Text(r.name,
                                      style: t.textTheme.bodyMedium,
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis),
                                  Text(r.path,
                                      style: t.textTheme.bodySmall?.copyWith(
                                          color: cs.onSurfaceVariant,
                                          fontSize: 11),
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis),
                                ],
                              )),
                              Text(
                                '${r.time.hour.toString().padLeft(2, '0')}:${r.time.minute.toString().padLeft(2, '0')}',
                                style: t.textTheme.labelSmall
                                    ?.copyWith(color: cs.onSurfaceVariant),
                              ),
                            ]),
                          ),
                      ],
                    ),
                  ),
                )
              : const SizedBox.shrink(),
        ),
      ],
    );

    if (embedded) return content;
    return SingleChildScrollView(
        padding: const EdgeInsets.only(top: 8), child: content);
  }
}

// ─── Shared Widgets ─────────────────────────────────────────────────────────

/// 电池优化提示横幅
class _BatteryBanner extends StatelessWidget {
  const _BatteryBanner({required this.onRequest});
  final VoidCallback onRequest;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    return Card(
      margin: const EdgeInsets.only(bottom: 12),
      color: cs.tertiaryContainer,
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(Icons.battery_alert_rounded,
                    color: cs.onTertiaryContainer, size: 20),
                const SizedBox(width: 8),
                Text('电池优化',
                    style: TextStyle(
                      color: cs.onTertiaryContainer,
                      fontSize: 14,
                      fontWeight: FontWeight.w600,
                    )),
              ],
            ),
            const SizedBox(height: 8),
            Text(
              '建议关闭电池优化以确保后台下载不被系统中断',
              style: TextStyle(color: cs.onTertiaryContainer, fontSize: 13),
            ),
            const SizedBox(height: 12),
            Align(
              alignment: Alignment.centerRight,
              child: FilledButton.tonal(
                onPressed: onRequest,
                child: const Text('立即关闭'),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// 卡片内的分区标题 + 内容
class _Section extends StatelessWidget {
  const _Section(
      {required this.icon, required this.title, required this.children});
  final IconData icon;
  final String title;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final cs = t.colorScheme;
    return Card(
      margin: const EdgeInsets.only(bottom: 12),
      child: Padding(
        padding: const EdgeInsets.all(20),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(children: [
              Icon(icon, color: cs.primary, size: 20),
              const SizedBox(width: 8),
              Text(title,
                  style: t.textTheme.titleSmall?.copyWith(
                    color: cs.primary,
                    fontWeight: FontWeight.w600,
                  )),
            ]),
            const SizedBox(height: 16),
            ...children,
          ],
        ),
      ),
    );
  }
}

/// 带入场动画的状态行
class _AnimatedStatusRow extends StatelessWidget {
  const _AnimatedStatusRow({
    required this.icon,
    required this.color,
    required this.label,
    required this.t,
  });
  final IconData icon;
  final Color color;
  final String label;
  final ThemeData t;

  @override
  Widget build(BuildContext context) {
    return TweenAnimationBuilder<double>(
      tween: Tween(begin: 0, end: 1),
      duration: const Duration(milliseconds: 350),
      curve: Curves.easeOutCubic,
      builder: (_, v, child) => Opacity(
        opacity: v,
        child: Transform.translate(
          offset: Offset(0, 8 * (1 - v)),
          child: child,
        ),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, color: color, size: 18),
          const SizedBox(width: 8),
          Expanded(
              child: Text(label,
                  style: t.textTheme.bodyMedium?.copyWith(color: color))),
        ],
      ),
    );
  }
}

class _Record {
  _Record({required this.name, required this.path, required this.time});
  final String name;
  final String path;
  final DateTime time;
}
