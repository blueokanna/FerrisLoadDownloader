import 'package:flutter/material.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/home/home_page_controller.dart';
import 'package:m3u8_downloader/src/home/home_widgets.dart';

class HomeDownloadTasksCard extends StatelessWidget {
  const HomeDownloadTasksCard({
    super.key,
    required this.tasks,
  });

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
                    'download-tasks-${tasks.length}-${tasks.first.id}'),
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
        borderRadius: BorderRadius.circular(18),
        color: cs.surfaceContainerHigh.withValues(alpha: 0.48),
      ),
      child: Text(
        l.text('no_active_downloads'),
        style: t.textTheme.bodyMedium?.copyWith(
          color: cs.onSurfaceVariant,
        ),
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
    final tone = task.error != null
        ? cs.errorContainer
        : task.resultPath != null
            ? cs.secondaryContainer
            : cs.primaryContainer.withValues(alpha: 0.74);
    final foreground = task.error != null
        ? cs.onErrorContainer
        : task.resultPath != null
            ? cs.onSecondaryContainer
            : cs.onPrimaryContainer;

    return AnimatedContainer(
      duration: FerrisMotion.medium,
      curve: FerrisMotion.emphasized,
      width: double.infinity,
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(20),
        color: tone,
        border: Border.all(color: foreground.withValues(alpha: 0.08)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              StatusPulseDot(
                active: task.running,
                color: foreground,
                size: 10,
              ),
              const SizedBox(width: 10),
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
              Text(
                '${(task.progress.clamp(0.0, 1.0) * 100).round()}%',
                style: t.textTheme.labelLarge?.copyWith(
                  color: foreground,
                  fontWeight: FontWeight.w800,
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
          ClipRRect(
            borderRadius: BorderRadius.circular(999),
            child: LinearProgressIndicator(
              minHeight: 6,
              value:
                  task.running ? task.progress.clamp(0.0, 1.0) : task.progress,
              backgroundColor: foreground.withValues(alpha: 0.16),
            ),
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
