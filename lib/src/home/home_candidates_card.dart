import 'package:flutter/material.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/home/home_widgets.dart';
import 'package:m3u8_downloader/src/rust/api/downloader.dart';

class HomeCandidatesCard extends StatelessWidget {
  const HomeCandidatesCard({
    super.key,
    required this.inspection,
    required this.selectedCandidate,
    required this.running,
    required this.analyzing,
    required this.selectionRevision,
    required this.onCandidateSelected,
    required this.onOpenAuthBrowser,
  });

  final MediaInspectionResult? inspection;
  final MediaCandidate? selectedCandidate;
  final bool running;
  final bool analyzing;
  final int selectionRevision;
  final ValueChanged<MediaCandidate> onCandidateSelected;
  final VoidCallback onOpenAuthBrowser;

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;

    return SectionCard(
      title: l.text('analysis_results'),
      subtitle: inspection == null
          ? l.text('no_candidates')
          : '${inspection!.pageTitle} · ${inspection!.candidates.length}',
      icon: Icons.video_collection_outlined,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (inspection?.warnings.isNotEmpty ?? false)
            Padding(
              padding: const EdgeInsets.only(bottom: 14),
              child: Column(
                children: [
                  for (final warning in inspection!.warnings) ...[
                    InspectionWarningTile(warning: warning),
                    const SizedBox(height: 10),
                  ],
                ],
              ),
            ),
          if (inspection?.authRequired ?? false)
            Padding(
              padding: const EdgeInsets.only(bottom: 14),
              child: AnimatedContainer(
                duration: const Duration(milliseconds: 280),
                curve: Curves.easeOutCubic,
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
                      inspection!.challengeReason,
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
                      onPressed:
                          running || analyzing ? null : onOpenAuthBrowser,
                      icon: const Icon(Icons.open_in_browser_rounded),
                      label: Text(l.text('auth_browser_open')),
                    ),
                  ],
                ),
              ),
            ),
          AnimatedSwitcher(
            duration: FerrisMotion.slow,
            switchInCurve: FerrisMotion.decelerate,
            switchOutCurve: FerrisMotion.accelerate,
            transitionBuilder: ferrisFadeScaleTransition,
            child: selectedCandidate == null
                ? const SizedBox.shrink()
                : Container(
                    key: ValueKey(
                      'selection-$selectionRevision-${selectedCandidate!.id}',
                    ),
                    width: double.infinity,
                    margin: const EdgeInsets.only(bottom: 14),
                    padding: const EdgeInsets.all(16),
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(22),
                      gradient: LinearGradient(
                        begin: Alignment.topLeft,
                        end: Alignment.bottomRight,
                        colors: [
                          cs.primaryContainer,
                          cs.tertiaryContainer,
                        ],
                      ),
                    ),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          l.text('current_selection'),
                          style: t.textTheme.labelLarge?.copyWith(
                            color: cs.onPrimaryContainer,
                            fontWeight: FontWeight.w800,
                            letterSpacing: 0.2,
                          ),
                        ),
                        const SizedBox(height: 10),
                        Text(
                          selectedCandidate!.title,
                          style: t.textTheme.titleMedium?.copyWith(
                            color: cs.onPrimaryContainer,
                            fontWeight: FontWeight.w800,
                          ),
                        ),
                        const SizedBox(height: 10),
                        Wrap(
                          spacing: 8,
                          runSpacing: 8,
                          children: [
                            StatusBadge(
                              label: selectedCandidate!.extractor,
                              color: cs.onPrimaryContainer,
                            ),
                            if (selectedCandidate!.qualityLabel.isNotEmpty)
                              StatusBadge(
                                label: selectedCandidate!.qualityLabel,
                                color: cs.onPrimaryContainer,
                              ),
                            if (selectedCandidate!.audioUrl != null)
                              StatusBadge(
                                label: l.text('separate_audio'),
                                color: cs.onPrimaryContainer,
                              ),
                          ],
                        ),
                      ],
                    ),
                  ),
          ),
          AnimatedSwitcher(
            duration: FerrisMotion.slow,
            switchInCurve: FerrisMotion.decelerate,
            switchOutCurve: FerrisMotion.accelerate,
            transitionBuilder: ferrisFadeScaleTransition,
            child: inspection == null || inspection!.candidates.isEmpty
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
                        'candidate-list-${inspection!.candidates.length}'),
                    children: [
                      for (final entry in inspection!.candidates.indexed)
                        Padding(
                          padding: const EdgeInsets.only(bottom: 10),
                          child: RevealMotion(
                            key: ValueKey(entry.$2.id),
                            delay: Duration(milliseconds: 28 * entry.$1),
                            child: CandidateTile(
                              candidate: entry.$2,
                              selected: selectedCandidate?.id == entry.$2.id,
                              onTap: () => onCandidateSelected(entry.$2),
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
}
