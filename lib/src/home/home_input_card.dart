import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/app/app_theme.dart';
import 'package:m3u8_downloader/src/home/home_widgets.dart';
import 'package:m3u8_downloader/src/home/source_input.dart';

class HomeInputCard extends StatelessWidget {
  const HomeInputCard({
    super.key,
    required this.formKey,
    required this.urlController,
    required this.fileNameController,
    required this.concurrencyController,
    required this.retriesController,
    required this.videoBitrateController,
    required this.audioBitrateController,
    required this.running,
    required this.analyzing,
    required this.keepTemp,
    required this.chosenDir,
    required this.onPickDir,
    required this.onResetDir,
    required this.onAnalyze,
    required this.onDownload,
    required this.onKeepTempChanged,
  });

  final GlobalKey<FormState> formKey;
  final TextEditingController urlController;
  final TextEditingController fileNameController;
  final TextEditingController concurrencyController;
  final TextEditingController retriesController;
  final TextEditingController videoBitrateController;
  final TextEditingController audioBitrateController;
  final bool running;
  final bool analyzing;
  final bool keepTemp;
  final String? chosenDir;
  final VoidCallback onPickDir;
  final VoidCallback onResetDir;
  final VoidCallback onAnalyze;
  final VoidCallback onDownload;
  final ValueChanged<bool> onKeepTempChanged;

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;

    return SectionCard(
      title: l.text('input_source'),
      subtitle: l.text('app_subtitle'),
      icon: Icons.travel_explore_rounded,
      child: Form(
        key: formKey,
        child: Column(
          children: [
            ValueListenableBuilder<TextEditingValue>(
              valueListenable: urlController,
              builder: (context, value, _) => TextFormField(
                controller: urlController,
                enabled: !analyzing,
                minLines: 1,
                maxLines: 2,
                keyboardType: TextInputType.url,
                textInputAction: TextInputAction.done,
                // Do NOT disable autocorrect/suggestions here: on many Android
                // IMEs (Gboard / Chinese keyboards) those flags switch the
                // field into a "privacy keyboard" mode that hides the normal
                // candidate bar and is unpleasant to type URLs in.
                decoration: InputDecoration(
                  labelText: l.text('input_source'),
                  hintText: l.text('source_hint'),
                  prefixIcon: const Icon(Icons.link_rounded),
                  suffixIcon: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      if (value.text.isNotEmpty)
                        IconButton(
                          onPressed: analyzing ? null : urlController.clear,
                          icon: const Icon(Icons.close_rounded),
                          tooltip: l.text('clear'),
                        ),
                      IconButton(
                        onPressed: analyzing
                            ? null
                            : () async {
                                final clipboard = await Clipboard.getData(
                                  Clipboard.kTextPlain,
                                );
                                final text = clipboard?.text?.trim();
                                if (text == null || text.isEmpty) {
                                  return;
                                }
                                urlController.value = TextEditingValue(
                                  text: text,
                                  selection: TextSelection.collapsed(
                                    offset: text.length,
                                  ),
                                );
                              },
                        icon: const Icon(Icons.content_paste_rounded),
                        tooltip: l.text('paste'),
                      ),
                    ],
                  ),
                ),
                validator: (value) => extractSourceUrl(value ?? '') == null
                    ? l.text('source_invalid')
                    : null,
              ),
            ),
            const SizedBox(height: 14),
            TextFormField(
              controller: fileNameController,
              enabled: !analyzing,
              keyboardType: TextInputType.text,
              textInputAction: TextInputAction.done,
              // Same as the URL field: no autocorrect/suggestions flags so the
              // normal keyboard (not the IME "privacy keyboard") is shown.
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
              borderRadius: FerrisShapes.of(context).md,
              onTap: analyzing ? null : onPickDir,
              child: Ink(
                padding: const EdgeInsets.symmetric(
                  horizontal: 16,
                  vertical: 14,
                ),
                decoration: BoxDecoration(
                  borderRadius: FerrisShapes.of(context).md,
                  color: cs.surfaceContainerHigh,
                  border: Border.all(color: cs.outlineVariant),
                ),
                child: Row(
                  children: [
                    AnimatedRotation(
                      duration: const Duration(milliseconds: 260),
                      turns: chosenDir == null ? 0 : 0.02,
                      child: Icon(Icons.folder_open_rounded, color: cs.primary),
                    ),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Text(
                        chosenDir ?? l.text('default_location'),
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: t.textTheme.bodySmall?.copyWith(
                          color: chosenDir == null
                              ? cs.onSurfaceVariant
                              : cs.onSurface,
                        ),
                      ),
                    ),
                    AnimatedSwitcher(
                      duration: FerrisMotion.fast,
                      transitionBuilder: ferrisFadeScaleTransition,
                      child: chosenDir != null
                          ? IconButton(
                              key: const ValueKey('reset-directory'),
                              onPressed: onResetDir,
                              icon: const Icon(Icons.restart_alt_rounded),
                            )
                          : const SizedBox(
                              key: ValueKey('directory-placeholder'),
                              width: 24,
                              height: 24,
                            ),
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
                  duration: FerrisMotion.medium,
                  curve: FerrisMotion.emphasized,
                  scale: analyzing ? 0.98 : 1,
                  child: OutlinedButton.icon(
                    onPressed: analyzing ? null : onAnalyze,
                    icon: analyzing
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
                  duration: FerrisMotion.medium,
                  curve: FerrisMotion.emphasized,
                  scale: running ? 0.98 : 1,
                  child: FilledButton.icon(
                    onPressed: analyzing ? null : onDownload,
                    icon: const Icon(Icons.download_rounded),
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
                Icon(
                  Icons.info_outline_rounded,
                  size: 16,
                  color: cs.onSurfaceVariant,
                ),
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
            AnimatedContainer(
              duration: FerrisMotion.medium,
              curve: FerrisMotion.emphasized,
              padding: const EdgeInsets.all(16),
              decoration: BoxDecoration(
                borderRadius: FerrisShapes.of(context).md,
                color: cs.surfaceContainer,
                border: Border.all(
                  color: cs.outlineVariant.withValues(alpha: 0.6),
                ),
              ),
              child: Column(
                children: [
                  Row(
                    children: [
                      Icon(Icons.tune_rounded, color: cs.primary),
                      const SizedBox(width: 8),
                      Text(
                        l.text('download_options'),
                        style: t.textTheme.titleSmall?.copyWith(
                          color: cs.primary,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 14),
                  LayoutBuilder(
                    builder: (context, constraints) {
                      final compact = constraints.maxWidth < 420;
                      final concurrencyField = TextFormField(
                        controller: concurrencyController,
                        enabled: !analyzing,
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
                        controller: retriesController,
                        enabled: !analyzing,
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
                        controller: videoBitrateController,
                        enabled: !analyzing,
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
                        controller: audioBitrateController,
                        enabled: !analyzing,
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
                  Material(
                    type: MaterialType.transparency,
                    child: SwitchListTile.adaptive(
                      value: keepTemp,
                      contentPadding: EdgeInsets.zero,
                      title: Text(l.text('keep_temp')),
                      subtitle: Text(l.text('keep_temp_hint')),
                      onChanged: analyzing ? null : onKeepTempChanged,
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}
