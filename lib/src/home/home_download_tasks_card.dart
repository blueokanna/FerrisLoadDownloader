import 'package:flutter/material.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/app/app_theme.dart';
import 'package:m3u8_downloader/src/home/home_page_controller.dart';
import 'package:m3u8_downloader/src/home/home_widgets.dart';

class HomeDownloadTasksCard extends StatelessWidget {
  const HomeDownloadTasksCard({super.key, required this.tasks});

  final List<HomeDownloadTask> tasks;

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final runningCount = tasks.where((task) => task.running).length;

    return SectionCard(
      title: l.text('active_downloads'),
      subtitle: '${l.text('running_downloads')}: $runningCount',
      icon: Icons.downloading_rounded,
      child: AnimatedSwitcher(
        duration: FerrisMotion.slow,
        switchInCurve: FerrisMotion.decelerate,
        switchOutCurve: FerrisMotion.accelerate,
        transitionBuilder: ferrisFadeScaleTransition,
        child: tasks.isEmpty
            ? _EmptyDownloadTasks(key: const ValueKey('empty-download-tasks'))
            : Column(
                key: ValueKey(
                  'download-tasks-${tasks.length}-${tasks.first.id}',
                ),
                children: [
                  for (final entry in tasks.indexed)
                    Padding(
                      padding: EdgeInsets.only(
                        bottom: entry.$1 == tasks.length - 1 ? 0 : 10,
                      ),
                      child: _DownloadTaskTile(
                        key: ValueKey(entry.$2.id),
                        task: entry.$2,
                      ),
                    ),
                ],
              ),
      ),
    );
  }
}

class _EmptyDownloadTasks extends StatelessWidget {
  const _EmptyDownloadTasks({super.key});

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;

    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        borderRadius: FerrisShapes.of(context).md,
        color: cs.surfaceContainerHigh.withValues(alpha: 0.48),
      ),
      child: Row(
        children: [
          Icon(
            Icons.download_for_offline_outlined,
            color: cs.onSurfaceVariant,
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              l.text('no_active_downloads'),
              style: t.textTheme.bodyMedium?.copyWith(
                color: cs.onSurfaceVariant,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _DownloadTaskTile extends StatelessWidget {
  const _DownloadTaskTile({super.key, required this.task});

  final HomeDownloadTask task;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final cs = t.colorScheme;
    final shapes = FerrisShapes.of(context);
    final failed = task.error != null;
    final completed = task.resultPath != null && !failed;
    final (tone, foreground, icon) = failed
        ? (cs.errorContainer, cs.onErrorContainer, Icons.error_outline_rounded)
        : completed
            ? (
                cs.secondaryContainer,
                cs.onSecondaryContainer,
                Icons.check_circle_outline_rounded,
              )
            : (
                cs.primaryContainer.withValues(alpha: 0.74),
                cs.onPrimaryContainer,
                Icons.downloading_rounded,
              );

    return AnimatedContainer(
      duration: FerrisMotion.medium,
      curve: FerrisMotion.emphasized,
      width: double.infinity,
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        borderRadius: shapes.md,
        color: tone,
        border: Border.all(color: foreground.withValues(alpha: 0.08)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              StatusPulseDot(active: task.running, color: foreground, size: 10),
              const SizedBox(width: 10),
              Icon(icon, size: 18, color: foreground),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  task.fileName,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: t.textTheme.titleSmall?.copyWith(
                    color: foreground,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              Container(
                padding: const EdgeInsets.symmetric(
                  horizontal: 10,
                  vertical: 4,
                ),
                decoration: BoxDecoration(
                  borderRadius: shapes.pill,
                  color: foreground.withValues(alpha: 0.12),
                ),
                child: Text(
                  '${(task.progress.clamp(0.0, 1.0) * 100).round()}%',
                  style: t.textTheme.labelMedium?.copyWith(
                    color: foreground,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
          SmoothLinearProgressIndicator(
            value: task.progress,
            color: foreground,
            backgroundColor: foreground.withValues(alpha: 0.16),
          ),
          const SizedBox(height: 10),
          Text(
            task.status,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: t.textTheme.bodySmall?.copyWith(
              color: foreground.withValues(alpha: 0.9),
              height: 1.35,
            ),
          ),
          if (task.backend != null) ...[
            const SizedBox(height: 8),
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  task.backend == 'yt-dlp'
                      ? Icons.hub_rounded
                      : Icons.memory_rounded,
                  size: 16,
                  color: foreground,
                ),
                const SizedBox(width: 6),
                Flexible(
                  child: Text(
                    task.backend!,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: t.textTheme.labelMedium?.copyWith(
                      color: foreground,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ),
              ],
            ),
          ],
          const SizedBox(height: 6),
          Text(
            task.resultPath ?? task.sourcePage,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: t.textTheme.bodySmall?.copyWith(
              color: foreground.withValues(alpha: 0.72),
            ),
          ),
        ],
      ),
    );
  }
}
