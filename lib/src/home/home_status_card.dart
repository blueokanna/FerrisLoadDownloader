import 'package:flutter/material.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/app/app_theme.dart';
import 'package:m3u8_downloader/src/home/home_page_controller.dart';
import 'package:m3u8_downloader/src/home/home_widgets.dart';

class HomeStatusCard extends StatelessWidget {
  const HomeStatusCard({
    super.key,
    required this.status,
    required this.error,
    required this.resultPath,
    required this.progress,
    required this.running,
    required this.analyzing,
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

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;
    final stageLabel = _stageLabel(l);

    return SectionCard(
      title: l.text('status'),
      subtitle: stageLabel,
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
              borderRadius: FerrisShapes.of(context).md,
              color: cs.surfaceContainerHigh,
              border: Border.all(
                color: cs.outlineVariant.withValues(alpha: 0.55),
              ),
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
                const SizedBox(width: 12),
                Expanded(
                  child: AnimatedSwitcher(
                    duration: FerrisMotion.medium,
                    switchInCurve: FerrisMotion.decelerate,
                    switchOutCurve: FerrisMotion.accelerate,
                    transitionBuilder: ferrisFadeScaleTransition,
                    child: status != null
                        ? Text(
                            status!,
                            key: ValueKey('status-msg-$status'),
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: t.textTheme.bodyMedium?.copyWith(
                              fontWeight: FontWeight.w600,
                            ),
                          )
                        : Text(
                            stageLabel,
                            key: const ValueKey('status-stage'),
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: t.textTheme.bodyMedium?.copyWith(
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(height: 16),
          AnimatedSwitcher(
            duration: FerrisMotion.medium,
            switchInCurve: FerrisMotion.decelerate,
            switchOutCurve: FerrisMotion.accelerate,
            transitionBuilder: ferrisFadeScaleTransition,
            child: recoveryMessage == null
                ? const SizedBox.shrink()
                : Container(
                    key: ValueKey(
                      'recovery-$recoveryRevision-$recoveryMessage',
                    ),
                    width: double.infinity,
                    margin: const EdgeInsets.only(top: 14),
                    padding: const EdgeInsets.all(16),
                    decoration: BoxDecoration(
                      borderRadius: FerrisShapes.of(context).md,
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
          SmoothLinearProgressIndicator(
            minHeight: 8,
            value: (running || analyzing) && progress <= 0 ? null : progress,
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
                      borderRadius: FerrisShapes.of(context).md,
                      color: cs.errorContainer,
                      border: Border.all(
                        color: cs.error.withValues(alpha: 0.28),
                      ),
                    ),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Container(
                              width: 36,
                              height: 36,
                              decoration: BoxDecoration(
                                shape: BoxShape.circle,
                                color: cs.error.withValues(alpha: 0.14),
                              ),
                              child: Icon(
                                Icons.error_outline_rounded,
                                size: 20,
                                color: cs.onErrorContainer,
                              ),
                            ),
                            const SizedBox(width: 12),
                            Expanded(
                              child: Text(
                                error!,
                                style: t.textTheme.bodyMedium?.copyWith(
                                  color: cs.onErrorContainer,
                                  height: 1.4,
                                ),
                              ),
                            ),
                          ],
                        ),
                        if (onRetry != null) ...[
                          const SizedBox(height: 14),
                          Align(
                            alignment: Alignment.centerLeft,
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
                      borderRadius: FerrisShapes.of(context).md,
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
}
