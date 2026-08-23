import 'package:flutter/material.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/app/app_theme.dart';
import 'package:m3u8_downloader/src/home/home_widgets.dart';

class HomeHistoryCard extends StatelessWidget {
  const HomeHistoryCard({
    super.key,
    required this.history,
  });

  final List<DownloadRecord> history;

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;

    return SectionCard(
      title: l.text('history'),
      subtitle: '${history.length}',
      icon: Icons.history_rounded,
      child: AnimatedSwitcher(
        duration: FerrisMotion.slow,
        switchInCurve: FerrisMotion.decelerate,
        switchOutCurve: FerrisMotion.accelerate,
        transitionBuilder: ferrisFadeScaleTransition,
        child: history.isEmpty
            ? Text(
                key: const ValueKey('empty-history'),
                l.text('no_candidates'),
                style: t.textTheme.bodyMedium?.copyWith(
                  color: cs.onSurfaceVariant,
                ),
              )
            : Column(
                key: ValueKey('history-${history.length}'),
                children: [
                  for (final entry in history.take(6).indexed)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 10),
                      child: RevealMotion(
                        key: ValueKey('${entry.$2.path}-${entry.$1}'),
                        delay: Duration(milliseconds: 34 * entry.$1),
                        child: AnimatedContainer(
                          duration: const Duration(milliseconds: 240),
                          curve: Curves.easeOutCubic,
                          width: double.infinity,
                          padding: const EdgeInsets.all(14),
                          decoration: BoxDecoration(
                            borderRadius: FerrisShapes.of(context).md,
                            color: cs.surfaceContainerHighest
                                .withValues(alpha: 0.45),
                          ),
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Text(
                                entry.$2.name,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: t.textTheme.labelLarge?.copyWith(
                                  fontWeight: FontWeight.w700,
                                ),
                              ),
                              const SizedBox(height: 4),
                              Text(
                                entry.$2.source,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: t.textTheme.bodySmall?.copyWith(
                                  color: cs.onSurfaceVariant,
                                ),
                              ),
                              const SizedBox(height: 4),
                              Text(
                                entry.$2.path,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: t.textTheme.bodySmall,
                              ),
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
