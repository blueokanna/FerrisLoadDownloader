import 'package:flutter/material.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/home/home_page_controller.dart';
import 'package:m3u8_downloader/src/home/home_widgets.dart';
import 'package:m3u8_downloader/src/rust/api/downloader.dart';

class HomeStatusCard extends StatelessWidget {
  const HomeStatusCard({
    super.key,
    required this.status,
    required this.error,
    required this.resultPath,
    required this.progress,
    required this.running,
    required this.analyzing,
    required this.selectedCandidate,
    required this.inspection,
    required this.stage,
    required this.stageRevision,
    required this.recoveryMessage,
    required this.recoveryRevision,
    required this.retryAction,
    required this.onRetry,
  });

  final String? status;
  final String? error;
  final String? resultPath;
  final double progress;
  final bool running;
  final bool analyzing;
  final MediaCandidate? selectedCandidate;
  final MediaInspectionResult? inspection;
  final HomeWorkflowStage stage;
  final int stageRevision;
  final String? recoveryMessage;
  final int recoveryRevision;
  final HomeRetryAction retryAction;
  final VoidCallback? onRetry;

  String _stageLabel(AppLocalizations l) {
    return switch (stage) {
      HomeWorkflowStage.idle => l.text('status_stage_idle'),
      HomeWorkflowStage.inspecting => l.text('status_stage_inspecting'),
      HomeWorkflowStage.ready => l.text('status_stage_ready'),
      HomeWorkflowStage.authRedirect => l.text('status_stage_auth'),
      HomeWorkflowStage.preparing => l.text('status_stage_preparing'),
      HomeWorkflowStage.backend => l.text('status_stage_backend'),
      HomeWorkflowStage.playlist => l.text('status_stage_playlist'),
      HomeWorkflowStage.transfer => l.text('status_stage_transfer'),
      HomeWorkflowStage.segments => l.text('status_stage_segments'),
      HomeWorkflowStage.merge => l.text('status_stage_merge'),
      HomeWorkflowStage.transcode => l.text('status_stage_transcode'),
      HomeWorkflowStage.exporting => l.text('status_stage_exporting'),
      HomeWorkflowStage.completed => l.text('status_stage_completed'),
      HomeWorkflowStage.failed => l.text('status_stage_failed'),
    };
  }

  String _stageSemanticLabel(AppLocalizations l) {
    final statusText = status?.trim();
    final errorText = error?.trim();
    if (errorText != null && errorText.isNotEmpty) {
      return '${_stageLabel(l)}. $errorText';
    }
    if (statusText != null && statusText.isNotEmpty) {
      return '${_stageLabel(l)}. $statusText';
    }
    return _stageLabel(l);
  }

  String _retryLabel(AppLocalizations l) {
    return switch (retryAction) {
      HomeRetryAction.none => l.text('retry'),
      HomeRetryAction.analyze => l.text('retry_analyze'),
      HomeRetryAction.download => l.text('retry_download'),
    };
  }

  IconData _stageIcon() {
    return switch (stage) {
      HomeWorkflowStage.idle => Icons.hourglass_bottom_rounded,
      HomeWorkflowStage.inspecting => Icons.travel_explore_rounded,
      HomeWorkflowStage.ready => Icons.task_alt_rounded,
      HomeWorkflowStage.authRedirect => Icons.shield_rounded,
      HomeWorkflowStage.preparing => Icons.tune_rounded,
      HomeWorkflowStage.backend => Icons.memory_rounded,
      HomeWorkflowStage.playlist => Icons.playlist_play_rounded,
      HomeWorkflowStage.transfer => Icons.downloading_rounded,
      HomeWorkflowStage.segments => Icons.view_stream_rounded,
      HomeWorkflowStage.merge => Icons.merge_type_rounded,
      HomeWorkflowStage.transcode => Icons.movie_filter_rounded,
      HomeWorkflowStage.exporting => Icons.save_alt_rounded,
      HomeWorkflowStage.completed => Icons.check_circle_rounded,
      HomeWorkflowStage.failed => Icons.error_outline_rounded,
    };
  }

  Color _stageTone(ColorScheme cs) {
    return switch (stage) {
      HomeWorkflowStage.idle => cs.surfaceContainerHighest,
      HomeWorkflowStage.inspecting => cs.secondaryContainer,
      HomeWorkflowStage.ready => cs.primaryContainer,
      HomeWorkflowStage.authRedirect => cs.tertiaryContainer,
      HomeWorkflowStage.preparing => cs.surfaceContainerHigh,
      HomeWorkflowStage.backend => cs.tertiaryContainer,
      HomeWorkflowStage.playlist => cs.secondaryContainer,
      HomeWorkflowStage.transfer => cs.primaryContainer,
      HomeWorkflowStage.segments => cs.primaryContainer,
      HomeWorkflowStage.merge => cs.tertiaryContainer,
      HomeWorkflowStage.transcode => cs.secondaryContainer,
      HomeWorkflowStage.exporting => cs.primaryContainer,
      HomeWorkflowStage.completed => cs.secondaryContainer,
      HomeWorkflowStage.failed => cs.errorContainer,
    };
  }

  Color _stageForeground(ColorScheme cs) {
    return switch (stage) {
      HomeWorkflowStage.failed => cs.onErrorContainer,
      HomeWorkflowStage.inspecting ||
      HomeWorkflowStage.playlist ||
      HomeWorkflowStage.transcode ||
      HomeWorkflowStage.completed =>
        cs.onSecondaryContainer,
      HomeWorkflowStage.authRedirect ||
      HomeWorkflowStage.backend ||
      HomeWorkflowStage.merge =>
        cs.onTertiaryContainer,
      HomeWorkflowStage.ready ||
      HomeWorkflowStage.transfer ||
      HomeWorkflowStage.segments ||
      HomeWorkflowStage.exporting =>
        cs.onPrimaryContainer,
      _ => cs.onSurface,
    };
  }

  int _phaseIndex() {
    return switch (stage) {
      HomeWorkflowStage.idle ||
      HomeWorkflowStage.inspecting ||
      HomeWorkflowStage.ready ||
      HomeWorkflowStage.authRedirect =>
        0,
      HomeWorkflowStage.preparing ||
      HomeWorkflowStage.backend ||
      HomeWorkflowStage.playlist ||
      HomeWorkflowStage.transfer ||
      HomeWorkflowStage.segments =>
        1,
      HomeWorkflowStage.merge || HomeWorkflowStage.transcode => 2,
      HomeWorkflowStage.exporting ||
      HomeWorkflowStage.completed ||
      HomeWorkflowStage.failed =>
        3,
    };
  }

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;

    return SectionCard(
      title: l.text('status'),
      subtitle: status ?? l.text('status'),
      icon: Icons.podcasts_rounded,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          AnimatedContainer(
            duration: FerrisMotion.medium,
            curve: FerrisMotion.emphasized,
            width: double.infinity,
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(20),
              gradient: LinearGradient(
                colors: [
                  cs.surfaceContainerHigh.withValues(alpha: 0.74),
                  cs.surface.withValues(alpha: 0.94),
                ],
              ),
              border:
                  Border.all(color: cs.outlineVariant.withValues(alpha: 0.18)),
            ),
            child: Row(
              children: [
                Semantics(
                  label: _stageSemanticLabel(l),
                  liveRegion: true,
                  child: Tooltip(
                    message: _stageSemanticLabel(l),
                    child: TweenAnimationBuilder<double>(
                      tween: Tween<double>(begin: 0.88, end: 1),
                      duration: FerrisMotion.medium,
                      curve: FerrisMotion.emphasized,
                      key: ValueKey('stage-icon-$stageRevision-$stage'),
                      builder: (context, value, child) {
                        return Transform.scale(scale: value, child: child);
                      },
                      child: Container(
                        width: 36,
                        height: 36,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: _stageTone(cs),
                          boxShadow: [
                            BoxShadow(
                              color: _stageTone(cs).withValues(alpha: 0.32),
                              blurRadius: running || analyzing ? 20 : 10,
                              spreadRadius: running || analyzing ? 1 : 0,
                            ),
                          ],
                        ),
                        child: Stack(
                          alignment: Alignment.center,
                          children: [
                            StatusPulseDot(
                              active: running || analyzing,
                              color: _stageForeground(cs),
                              size: 10,
                            ),
                            Icon(
                              _stageIcon(),
                              size: 18,
                              color: _stageForeground(cs),
                            ),
                          ],
                        ),
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: AnimatedSwitcher(
                    duration: FerrisMotion.medium,
                    switchInCurve: FerrisMotion.decelerate,
                    switchOutCurve: FerrisMotion.accelerate,
                    transitionBuilder: ferrisFadeScaleTransition,
                    child: Row(
                      key: ValueKey('stage-$stageRevision-$stage'),
                      children: [
                        Icon(_stageIcon(), size: 18),
                        const SizedBox(width: 8),
                        Expanded(
                          child: Text(
                            _stageLabel(l),
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: t.textTheme.titleSmall?.copyWith(
                              fontWeight: FontWeight.w800,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
                StatusBadge(
                  label: _stageLabel(l),
                  color: _stageTone(cs),
                ),
              ],
            ),
          ),
          const SizedBox(height: 16),
          LayoutBuilder(
            builder: (context, constraints) {
              final compact = constraints.maxWidth < 360;
              final pills = [
                _WorkflowPhasePill(
                  label: l.text('status_phase_inspect'),
                  state: _phaseIndex() > 0
                      ? _WorkflowPhaseState.completed
                      : _phaseIndex() == 0
                          ? _WorkflowPhaseState.active
                          : _WorkflowPhaseState.idle,
                ),
                _WorkflowPhasePill(
                  label: l.text('status_phase_acquire'),
                  state: _phaseIndex() > 1
                      ? _WorkflowPhaseState.completed
                      : _phaseIndex() == 1
                          ? _WorkflowPhaseState.active
                          : _WorkflowPhaseState.idle,
                ),
                _WorkflowPhasePill(
                  label: l.text('status_phase_assemble'),
                  state: _phaseIndex() > 2
                      ? _WorkflowPhaseState.completed
                      : _phaseIndex() == 2
                          ? _WorkflowPhaseState.active
                          : _WorkflowPhaseState.idle,
                ),
                _WorkflowPhasePill(
                  label: l.text('status_phase_deliver'),
                  state: _phaseIndex() == 3
                      ? _WorkflowPhaseState.active
                      : _phaseIndex() > 3
                          ? _WorkflowPhaseState.completed
                          : _WorkflowPhaseState.idle,
                ),
              ];

              if (compact) {
                final itemWidth = (constraints.maxWidth - 8) / 2;
                return Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    for (final pill in pills)
                      SizedBox(width: itemWidth, child: pill),
                  ],
                );
              }

              return Row(
                children: [
                  for (final pill in pills) ...[
                    Expanded(child: pill),
                    if (pill != pills.last) const SizedBox(width: 8),
                  ],
                ],
              );
            },
          ),
          AnimatedSwitcher(
            duration: FerrisMotion.medium,
            switchInCurve: FerrisMotion.decelerate,
            switchOutCurve: FerrisMotion.accelerate,
            transitionBuilder: ferrisFadeScaleTransition,
            child: recoveryMessage == null
                ? const SizedBox.shrink()
                : Container(
                    key:
                        ValueKey('recovery-$recoveryRevision-$recoveryMessage'),
                    width: double.infinity,
                    margin: const EdgeInsets.only(top: 14),
                    padding: const EdgeInsets.all(16),
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(18),
                      color: cs.tertiaryContainer.withValues(alpha: 0.72),
                    ),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          l.text('recovery_started'),
                          style: t.textTheme.labelLarge?.copyWith(
                            color: cs.onTertiaryContainer,
                            fontWeight: FontWeight.w800,
                          ),
                        ),
                        const SizedBox(height: 6),
                        Text(
                          recoveryMessage!,
                          style: t.textTheme.bodyMedium?.copyWith(
                            color: cs.onTertiaryContainer,
                          ),
                        ),
                      ],
                    ),
                  ),
          ),
          const SizedBox(height: 16),
          ClipRRect(
            borderRadius: BorderRadius.circular(999),
            child: TweenAnimationBuilder<double?>(
              tween: Tween<double?>(
                begin: 0,
                end: (running || analyzing)
                    ? (progress > 0 ? progress : null)
                    : progress.clamp(0.0, 1.0),
              ),
              duration: FerrisMotion.medium,
              curve: FerrisMotion.emphasized,
              builder: (context, value, _) {
                return LinearProgressIndicator(minHeight: 8, value: value);
              },
            ),
          ),
          const SizedBox(height: 16),
          Wrap(
            spacing: 10,
            runSpacing: 10,
            children: [
              _StatusMetaPill(
                label: l.text('source_page'),
                value: _sourceLabel(),
                color: cs.primaryContainer,
              ),
              _StatusMetaPill(
                label: l.text('extractor'),
                value: selectedCandidate?.extractor ??
                    inspection?.extractor ??
                    '-',
                color: cs.secondaryContainer,
              ),
              _StatusMetaPill(
                label: l.text('stream'),
                value: selectedCandidate?.protocol ?? '-',
                color: cs.tertiaryContainer,
              ),
            ],
          ),
          AnimatedSwitcher(
            duration: FerrisMotion.medium,
            switchInCurve: FerrisMotion.decelerate,
            switchOutCurve: FerrisMotion.accelerate,
            transitionBuilder: ferrisFadeScaleTransition,
            child: status != null
                ? Padding(
                    key: ValueKey('status-$status'),
                    padding: const EdgeInsets.only(top: 16),
                    child: Text(status!, style: t.textTheme.bodyLarge),
                  )
                : const SizedBox.shrink(),
          ),
          AnimatedSwitcher(
            duration: FerrisMotion.medium,
            switchInCurve: FerrisMotion.decelerate,
            switchOutCurve: FerrisMotion.accelerate,
            transitionBuilder: ferrisFadeScaleTransition,
            child: error != null
                ? Container(
                    key: ValueKey('error-$error'),
                    width: double.infinity,
                    margin: const EdgeInsets.only(top: 14),
                    padding: const EdgeInsets.all(16),
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(18),
                      gradient: LinearGradient(
                        begin: Alignment.topLeft,
                        end: Alignment.bottomRight,
                        colors: [
                          cs.errorContainer,
                          cs.error.withValues(alpha: 0.92),
                        ],
                      ),
                    ),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          error!,
                          style: t.textTheme.bodyMedium?.copyWith(
                            color: cs.onErrorContainer,
                          ),
                        ),
                        if (onRetry != null) ...[
                          const SizedBox(height: 12),
                          SizedBox(
                            width: double.infinity,
                            child: FilledButton.icon(
                              onPressed: onRetry,
                              icon: const Icon(Icons.refresh_rounded),
                              label: Text(_retryLabel(l)),
                            ),
                          ),
                        ],
                      ],
                    ),
                  )
                : const SizedBox.shrink(),
          ),
          AnimatedSwitcher(
            duration: FerrisMotion.medium,
            switchInCurve: FerrisMotion.decelerate,
            switchOutCurve: FerrisMotion.accelerate,
            transitionBuilder: ferrisFadeScaleTransition,
            child: onRetry != null && error == null && recoveryMessage != null
                ? Align(
                    key: ValueKey('recovery-action-$recoveryRevision'),
                    alignment: Alignment.centerLeft,
                    child: TextButton.icon(
                      onPressed: onRetry,
                      icon: const Icon(Icons.restart_alt_rounded),
                      label: Text(l.text('retry')),
                    ),
                  )
                : const SizedBox.shrink(),
          ),
          AnimatedSwitcher(
            duration: FerrisMotion.medium,
            switchInCurve: FerrisMotion.decelerate,
            switchOutCurve: FerrisMotion.accelerate,
            transitionBuilder: ferrisFadeScaleTransition,
            child: resultPath != null
                ? Container(
                    key: ValueKey('result-$resultPath'),
                    width: double.infinity,
                    margin: const EdgeInsets.only(top: 14),
                    padding: const EdgeInsets.all(16),
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(18),
                      color: cs.primaryContainer.withValues(alpha: 0.45),
                    ),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          l.text('output_path'),
                          style: t.textTheme.labelLarge?.copyWith(
                            color: cs.primary,
                          ),
                        ),
                        const SizedBox(height: 6),
                        SelectableText(resultPath!),
                      ],
                    ),
                  )
                : const SizedBox.shrink(),
          ),
        ],
      ),
    );
  }

  String _sourceLabel() {
    final pageUrl = selectedCandidate?.pageUrl ?? inspection?.pageUrl;
    if (pageUrl == null || pageUrl.isEmpty) {
      return '-';
    }
    final parsed = Uri.tryParse(pageUrl);
    return parsed?.host.isNotEmpty == true ? parsed!.host : pageUrl;
  }
}

enum _WorkflowPhaseState { idle, active, completed }

class _WorkflowPhasePill extends StatelessWidget {
  const _WorkflowPhasePill({required this.label, required this.state});

  final String label;
  final _WorkflowPhaseState state;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final cs = t.colorScheme;
    final background = switch (state) {
      _WorkflowPhaseState.idle =>
        cs.surfaceContainerHigh.withValues(alpha: 0.48),
      _WorkflowPhaseState.active => cs.primaryContainer,
      _WorkflowPhaseState.completed => cs.secondaryContainer,
    };
    final foreground = switch (state) {
      _WorkflowPhaseState.idle => cs.onSurfaceVariant,
      _WorkflowPhaseState.active => cs.onPrimaryContainer,
      _WorkflowPhaseState.completed => cs.onSecondaryContainer,
    };

    return AnimatedContainer(
      duration: FerrisMotion.medium,
      curve: FerrisMotion.emphasized,
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 10),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(16),
        color: background,
      ),
      child: Text(
        label,
        textAlign: TextAlign.center,
        maxLines: 2,
        overflow: TextOverflow.ellipsis,
        style: t.textTheme.labelMedium?.copyWith(
          color: foreground,
          fontWeight: FontWeight.w700,
        ),
      ),
    );
  }
}

class _StatusMetaPill extends StatelessWidget {
  const _StatusMetaPill({
    required this.label,
    required this.value,
    required this.color,
  });

  final String label;
  final String value;
  final Color color;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final brightness = ThemeData.estimateBrightnessForColor(color);
    final foreground =
        brightness == Brightness.dark ? Colors.white : Colors.black87;

    return Container(
      constraints: const BoxConstraints(minWidth: 96),
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(18),
        color: color,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            label,
            style: t.textTheme.labelSmall?.copyWith(
              color: foreground.withValues(alpha: 0.78),
              fontWeight: FontWeight.w700,
            ),
          ),
          const SizedBox(height: 3),
          Text(
            value,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: t.textTheme.labelLarge?.copyWith(
              color: foreground,
              fontWeight: FontWeight.w800,
            ),
          ),
        ],
      ),
    );
  }
}
