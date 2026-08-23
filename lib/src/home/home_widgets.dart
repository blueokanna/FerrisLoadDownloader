import 'package:flutter/material.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/app/app_theme.dart';
import 'package:m3u8_downloader/src/app/runtime_capabilities.dart';
import 'package:m3u8_downloader/src/rust/api/downloader.dart';

class FerrisMotion {
  static const Duration fast = Duration(milliseconds: 200);
  static const Duration medium = Duration(milliseconds: 300);
  static const Duration slow = Duration(milliseconds: 500);
  static const Curve linear = Curves.linear;
  static const Curve standard = Cubic(0.2, 0, 0, 1);
  static const Curve standardDecelerate = Cubic(0, 0, 0, 1);
  static const Curve standardAccelerate = Cubic(0.3, 0, 1, 1);
  static const Curve emphasized = Curves.easeInOutCubicEmphasized;
  static const Curve emphasizedDecelerate = Cubic(0.05, 0.7, 0.1, 1);
  static const Curve emphasizedAccelerate = Cubic(0.3, 0, 0.8, 0.15);
  static const Curve decelerate = emphasizedDecelerate;
  static const Curve accelerate = emphasizedAccelerate;
}

Widget ferrisFadeScaleTransition(Widget child, Animation<double> animation) {
  final curved = CurvedAnimation(
    parent: animation,
    curve: FerrisMotion.standardDecelerate,
    reverseCurve: FerrisMotion.standardAccelerate,
  );
  return FadeTransition(
    opacity: curved,
    child: ScaleTransition(
      scale: Tween<double>(begin: 0.985, end: 1).animate(curved),
      child: child,
    ),
  );
}

Widget ferrisSharedAxisTransition(Widget child, Animation<double> animation) {
  final fade = CurvedAnimation(
    parent: animation,
    curve: FerrisMotion.standardDecelerate,
    reverseCurve: FerrisMotion.standardAccelerate,
  );
  final slide = Tween<Offset>(
    begin: const Offset(0, 0.045),
    end: Offset.zero,
  ).animate(
    CurvedAnimation(parent: animation, curve: FerrisMotion.emphasized),
  );
  return FadeTransition(
    opacity: fade,
    child: SlideTransition(position: slide, child: child),
  );
}

class SectionCard extends StatelessWidget {
  const SectionCard({
    super.key,
    required this.title,
    required this.subtitle,
    required this.icon,
    required this.child,
    this.trailing,
  });

  final String title;
  final String subtitle;
  final IconData icon;
  final Widget child;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final cs = t.colorScheme;
    final shapes = FerrisShapes.of(context);
    final spacing = FerrisSpacing.of(context);
    return AnimatedContainer(
      duration: FerrisMotion.medium,
      curve: FerrisMotion.emphasized,
      decoration: BoxDecoration(
        borderRadius: shapes.card,
        color: cs.surfaceContainerLow,
        border: Border.all(color: cs.outlineVariant.withValues(alpha: 0.55)),
      ),
      child: Material(
        color: Colors.transparent,
        borderRadius: shapes.card,
        clipBehavior: Clip.antiAlias,
        child: Padding(
          padding: EdgeInsets.fromLTRB(
            spacing.lg,
            spacing.lg,
            spacing.lg,
            spacing.xl,
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Container(
                    width: 40,
                    height: 40,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: cs.primaryContainer,
                    ),
                    child: Icon(icon, color: cs.onPrimaryContainer),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          title,
                          style: t.textTheme.titleMedium?.copyWith(
                            fontWeight: FontWeight.w800,
                          ),
                        ),
                        const SizedBox(height: 2),
                        Text(
                          subtitle,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: t.textTheme.bodySmall?.copyWith(
                            color: cs.onSurfaceVariant,
                          ),
                        ),
                      ],
                    ),
                  ),
                  if (trailing != null) ...[
                    const SizedBox(width: 8),
                    trailing!,
                  ],
                ],
              ),
              SizedBox(height: spacing.lg),
              child,
            ],
          ),
        ),
      ),
    );
  }
}

class CandidateTile extends StatelessWidget {
  const CandidateTile({
    super.key,
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
    return AnimatedSlide(
      duration: FerrisMotion.medium,
      curve: FerrisMotion.emphasized,
      offset: selected ? Offset.zero : const Offset(0, 0.018),
      child: AnimatedScale(
        duration: FerrisMotion.medium,
        curve: FerrisMotion.emphasized,
        scale: selected ? 1 : 0.986,
        child: InkWell(
          borderRadius: FerrisShapes.of(context).md,
          onTap: onTap,
          child: AnimatedContainer(
            duration: FerrisMotion.medium,
            curve: FerrisMotion.emphasized,
            padding: const EdgeInsets.all(16),
            decoration: BoxDecoration(
              borderRadius: FerrisShapes.of(context).md,
              border: Border.all(
                color: selected
                    ? cs.primary
                    : cs.outlineVariant.withValues(alpha: 0.62),
                width: selected ? 2 : 1,
              ),
              color: selected ? cs.secondaryContainer : cs.surfaceContainerHigh,
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
                      duration: FerrisMotion.fast,
                      transitionBuilder: ferrisFadeScaleTransition,
                      child: selected
                          ? Icon(
                              Icons.check_circle_rounded,
                              key: const ValueKey('selected-check'),
                              color: cs.primary,
                            )
                          : const SizedBox(
                              key: ValueKey('selected-empty'),
                              width: 24,
                            ),
                    ),
                  ],
                ),
                const SizedBox(height: 8),
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    MiniChip(label: candidate.protocol.toUpperCase()),
                    if (candidate.primary) MiniChip(label: l.text('core')),
                    MiniChip(label: '${l.text('score')} ${candidate.score}'),
                    MiniChip(label: candidate.qualityLabel),
                    MiniChip(label: candidate.container.toUpperCase()),
                    if (candidate.width > 0 && candidate.height > 0)
                      MiniChip(label: '${candidate.width}x${candidate.height}'),
                    if (candidate.durationSeconds > 0)
                      MiniChip(label: '${candidate.durationSeconds.round()}s'),
                    if (candidate.segmentCount > 0)
                      MiniChip(
                        label:
                            '${candidate.segmentCount} ${l.text('segments')}',
                      ),
                    if (candidate.audioUrl != null) const MiniChip(label: 'AV'),
                    if (candidate.requiresFfmpeg)
                      const MiniChip(label: 'FFmpeg'),
                  ],
                ),
                const SizedBox(height: 10),
                if (candidate.reason.isNotEmpty) ...[
                  Text(
                    candidate.reason,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: t.textTheme.bodySmall?.copyWith(
                      color: cs.primary,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                  const SizedBox(height: 8),
                ],
                Text(
                  candidate.mediaUrl,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: t.textTheme.bodySmall?.copyWith(
                    color: cs.onSurfaceVariant,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class RevealMotion extends StatefulWidget {
  const RevealMotion({
    super.key,
    required this.child,
    this.delay = Duration.zero,
    this.offset = const Offset(0, 0.04),
  });

  final Widget child;
  final Duration delay;
  final Offset offset;

  @override
  State<RevealMotion> createState() => _RevealMotionState();
}

class _RevealMotionState extends State<RevealMotion>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;
  late Animation<double> _opacity;
  late Animation<double> _scale;
  late Animation<Offset> _slide;
  int _ticket = 0;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(vsync: this, duration: FerrisMotion.slow);
    _configureAnimations();
    _schedule();
  }

  void _configureAnimations() {
    final curve = CurvedAnimation(
      parent: _controller,
      curve: FerrisMotion.emphasizedDecelerate,
      reverseCurve: FerrisMotion.emphasizedAccelerate,
    );
    _opacity = CurvedAnimation(
      parent: _controller,
      curve: const Interval(0, 0.72, curve: Curves.easeOut),
    );
    _scale = Tween<double>(begin: 0.985, end: 1).animate(curve);
    _slide = Tween<Offset>(
      begin: widget.offset,
      end: Offset.zero,
    ).animate(curve);
  }

  void _schedule() {
    final ticket = ++_ticket;
    Future<void>.delayed(widget.delay, () {
      if (!mounted || ticket != _ticket) {
        return;
      }
      _controller.forward(from: 0);
    });
  }

  @override
  void didUpdateWidget(covariant RevealMotion oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.delay != widget.delay || oldWidget.offset != widget.offset) {
      _configureAnimations();
      _controller.value = 0;
      _schedule();
    }
  }

  @override
  void dispose() {
    _ticket++;
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return FadeTransition(
      opacity: _opacity,
      child: SlideTransition(
        position: _slide,
        child: ScaleTransition(scale: _scale, child: widget.child),
      ),
    );
  }
}

class MiniChip extends StatelessWidget {
  const MiniChip({super.key, required this.label, this.icon});

  final String label;
  final IconData? icon;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final cs = t.colorScheme;
    final shapes = FerrisShapes.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
      decoration: BoxDecoration(
        borderRadius: shapes.pill,
        color: cs.surfaceContainerHighest,
        border: Border.all(color: cs.outlineVariant.withValues(alpha: 0.28)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (icon != null) ...[
            Icon(icon, size: 14, color: cs.onSurfaceVariant),
            const SizedBox(width: 5),
          ],
          Text(
            label,
            style: t.textTheme.labelSmall?.copyWith(fontWeight: FontWeight.w700),
          ),
        ],
      ),
    );
  }
}

class RuntimeCapabilitiesPanel extends StatelessWidget {
  const RuntimeCapabilitiesPanel({
    super.key,
    required this.capabilities,
  });

  final Future<PlatformCapabilitySnapshot> capabilities;

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;

    final shapes = FerrisShapes.of(context);
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        borderRadius: shapes.md,
        color: cs.surfaceContainerHigh.withValues(alpha: 0.52),
        border: Border.all(color: cs.outlineVariant.withValues(alpha: 0.5)),
      ),
      child: FutureBuilder<PlatformCapabilitySnapshot>(
        future: capabilities,
        builder: (context, snapshot) {
          final report = snapshot.data;
          return AnimatedSize(
            duration: FerrisMotion.medium,
            curve: FerrisMotion.emphasized,
            alignment: Alignment.topCenter,
            child: AnimatedSwitcher(
              duration: FerrisMotion.medium,
              switchInCurve: FerrisMotion.emphasizedDecelerate,
              switchOutCurve: FerrisMotion.emphasizedAccelerate,
              transitionBuilder: ferrisSharedAxisTransition,
              child: report == null
                  ? Row(
                      key: const ValueKey('capabilities-loading'),
                      children: [
                        SizedBox(
                          width: 20,
                          height: 20,
                          child: CircularProgressIndicator(
                            strokeWidth: 2.4,
                            color: cs.primary,
                          ),
                        ),
                        const SizedBox(width: 12),
                        Expanded(
                          child: Text(
                            l.text('probing_capabilities'),
                            style: t.textTheme.bodyMedium,
                          ),
                        ),
                      ],
                    )
                  : _RuntimeCapabilityContent(
                      key: const ValueKey('capabilities-ready'),
                      report: report,
                    ),
            ),
          );
        },
      ),
    );
  }
}

class _RuntimeCapabilityContent extends StatelessWidget {
  const _RuntimeCapabilityContent({
    super.key,
    required this.report,
  });

  final PlatformCapabilitySnapshot report;

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;
    final encoders = report.videoEncoders.take(4).toList();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Icon(Icons.memory_rounded, color: cs.primary),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    l.text('runtime_capabilities'),
                    style: t.textTheme.titleSmall?.copyWith(
                      fontWeight: FontWeight.w800,
                    ),
                  ),
                  Text(
                    '${report.platform.toUpperCase()} · ${report.transcoderBackend}',
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: t.textTheme.bodySmall?.copyWith(
                      color: cs.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
        const SizedBox(height: 14),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            _CapabilityIndicator(
              label: 'FFmpeg',
              available: report.ffmpegAvailable,
            ),
            _CapabilityIndicator(
              label: 'yt-dlp',
              available: report.ytdlpAvailable,
            ),
            _CapabilityIndicator(
              label: l.text('hardware_encoder'),
              available: report.hardwareAccelerated,
            ),
            const MiniChip(label: 'HLS · DASH'),
            const MiniChip(label: 'YouTube · Bilibili'),
          ],
        ),
        if (encoders.isNotEmpty) ...[
          const SizedBox(height: 14),
          Text(
            l.text('video_encoders'),
            style: t.textTheme.labelLarge?.copyWith(
              color: cs.onSurfaceVariant,
            ),
          ),
          const SizedBox(height: 6),
          for (final encoder in encoders)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Icon(Icons.check_circle_rounded, size: 16, color: cs.primary),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(encoder, style: t.textTheme.bodySmall),
                  ),
                ],
              ),
            ),
        ],
        if (report.videoDecoders.isNotEmpty) ...[
          const SizedBox(height: 10),
          Text(
            '${l.text('video_decoders')}: ${report.videoDecoders.length}',
            style: t.textTheme.bodySmall?.copyWith(
              color: cs.onSurfaceVariant,
              fontWeight: FontWeight.w700,
            ),
          ),
        ],
        for (final note in report.notes) ...[
          const SizedBox(height: 10),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Icon(
                Icons.info_outline_rounded,
                size: 16,
                color: cs.onSurfaceVariant,
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  note,
                  style: t.textTheme.bodySmall?.copyWith(
                    color: cs.onSurfaceVariant,
                  ),
                ),
              ),
            ],
          ),
        ],
      ],
    );
  }
}

class _CapabilityIndicator extends StatelessWidget {
  const _CapabilityIndicator({
    required this.label,
    required this.available,
  });

  final String label;
  final bool available;

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final cs = Theme.of(context).colorScheme;
    final background = available ? cs.primaryContainer : cs.errorContainer;
    final foreground = available ? cs.onPrimaryContainer : cs.onErrorContainer;
    return AnimatedContainer(
      duration: FerrisMotion.fast,
      curve: FerrisMotion.standard,
      padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 7),
      decoration: BoxDecoration(
        color: background,
        borderRadius: FerrisShapes.of(context).pill,
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            available ? Icons.check_rounded : Icons.close_rounded,
            size: 15,
            color: foreground,
          ),
          const SizedBox(width: 5),
          Flexible(
            child: Text(
              '$label · ${l.text(available ? 'available' : 'not_available')}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: Theme.of(context).textTheme.labelSmall?.copyWith(
                    color: foreground,
                    fontWeight: FontWeight.w800,
                  ),
            ),
          ),
        ],
      ),
    );
  }
}

class DisplayPreviewCard extends StatelessWidget {
  const DisplayPreviewCard({
    super.key,
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
      duration: FerrisMotion.slow,
      curve: FerrisMotion.emphasized,
      height: 234,
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        borderRadius: FerrisShapes.of(context).card,
        color: cs.surfaceContainer,
        border: Border.all(color: cs.outlineVariant),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  profile.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: previewTheme.textTheme.labelLarge?.copyWith(
                    color: cs.onSurface,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              AnimatedSwitcher(
                duration: FerrisMotion.fast,
                transitionBuilder: ferrisFadeScaleTransition,
                child: Icon(
                  isDark ? Icons.dark_mode_rounded : Icons.light_mode_rounded,
                  key: ValueKey(isDark),
                  color: cs.onSurface,
                ),
              ),
            ],
          ),
          const SizedBox(height: 16),
          Expanded(
            child: Container(
              width: double.infinity,
              padding: const EdgeInsets.all(16),
              decoration: BoxDecoration(
                borderRadius: FerrisShapes.of(context).md,
                color: cs.surface,
                border: Border.all(color: cs.outlineVariant),
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
                  const Spacer(),
                  Row(
                    children: [
                      PreviewMiniSwatch(color: profile.seed),
                      const SizedBox(width: 8),
                      PreviewMiniSwatch(color: profile.accent),
                      const Spacer(),
                      Icon(Icons.check_circle_rounded, color: cs.primary),
                    ],
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class ThemePaletteCard extends StatelessWidget {
  const ThemePaletteCard({
    super.key,
    required this.profile,
    required this.selected,
    this.onTap,
  });

  final AppThemeProfile profile;
  final bool selected;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final cs = t.colorScheme;
    final shapes = FerrisShapes.of(context);
    return AnimatedSlide(
      duration: FerrisMotion.medium,
      curve: FerrisMotion.emphasized,
      offset: selected ? Offset.zero : const Offset(0, 0.02),
      child: AnimatedScale(
        duration: FerrisMotion.medium,
        curve: FerrisMotion.emphasized,
        scale: selected ? 1 : 0.972,
        child: InkWell(
          borderRadius: shapes.md,
          onTap: onTap,
          child: AnimatedContainer(
            duration: FerrisMotion.medium,
            curve: FerrisMotion.emphasized,
            width: 172,
            padding: const EdgeInsets.all(14),
            decoration: BoxDecoration(
              borderRadius: shapes.md,
              color: selected ? cs.primaryContainer : cs.surfaceContainerHigh,
              border: Border.all(
                color: selected ? cs.primary : cs.outlineVariant,
                width: selected ? 2 : 1,
              ),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Container(
                  height: 58,
                  decoration: BoxDecoration(
                    borderRadius: shapes.sm,
                    color: profile.canvasLight,
                  ),
                  child: Row(
                    children: [
                      const SizedBox(width: 12),
                      ThemeDot(color: profile.seed),
                      const SizedBox(width: 8),
                      ThemeDot(color: profile.accent),
                      const Spacer(),
                      AnimatedSwitcher(
                        duration: FerrisMotion.fast,
                        transitionBuilder: ferrisFadeScaleTransition,
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
                  style: t.textTheme.labelLarge?.copyWith(
                    fontWeight: FontWeight.w800,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  profile.description,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: t.textTheme.bodySmall?.copyWith(
                    color: cs.onSurfaceVariant,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class InspectionWarningTile extends StatelessWidget {
  const InspectionWarningTile({super.key, required this.warning});

  final String warning;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final parsed = InspectionWarningData.parse(warning, t.colorScheme);
    return AnimatedContainer(
      duration: FerrisMotion.medium,
      curve: FerrisMotion.emphasized,
      width: double.infinity,
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        borderRadius: FerrisShapes.of(context).md,
        color: parsed.background,
        border: Border.all(color: parsed.border),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 34,
            height: 34,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: parsed.accent.withValues(alpha: 0.14),
            ),
            child: Icon(parsed.icon, size: 18, color: parsed.accent),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    MiniChip(label: parsed.scopeLabel),
                    MiniChip(label: parsed.codeLabel),
                  ],
                ),
                const SizedBox(height: 10),
                Text(
                  parsed.message,
                  style: t.textTheme.bodySmall?.copyWith(
                    color: parsed.foreground,
                    fontWeight: FontWeight.w600,
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

class InspectionWarningData {
  const InspectionWarningData({
    required this.scopeLabel,
    required this.codeLabel,
    required this.message,
    required this.icon,
    required this.background,
    required this.border,
    required this.accent,
    required this.foreground,
  });

  final String scopeLabel;
  final String codeLabel;
  final String message;
  final IconData icon;
  final Color background;
  final Color border;
  final Color accent;
  final Color foreground;

  static InspectionWarningData parse(String raw, ColorScheme cs) {
    final match = RegExp(r'^\[([^:\]]+):([^\]]+)\]\s*(.+)$').firstMatch(raw);
    final scope = match?.group(1)?.trim().toLowerCase() ?? 'notice';
    final code = match?.group(2)?.trim() ?? 'general';
    final message = match?.group(3)?.trim() ?? raw;

    switch (scope) {
      case 'auth':
        return InspectionWarningData(
          scopeLabel: 'AUTH',
          codeLabel: code.toUpperCase(),
          message: message,
          icon: Icons.shield_outlined,
          background: cs.errorContainer.withValues(alpha: 0.72),
          border: cs.error.withValues(alpha: 0.24),
          accent: cs.error,
          foreground: cs.onErrorContainer,
        );
      case 'site':
        return InspectionWarningData(
          scopeLabel: 'SITE',
          codeLabel: code.toUpperCase(),
          message: message,
          icon: Icons.language_rounded,
          background: cs.tertiaryContainer.withValues(alpha: 0.74),
          border: cs.tertiary.withValues(alpha: 0.24),
          accent: cs.tertiary,
          foreground: cs.onTertiaryContainer,
        );
      case 'media':
        return InspectionWarningData(
          scopeLabel: 'MEDIA',
          codeLabel: code.toUpperCase(),
          message: message,
          icon: Icons.movie_creation_outlined,
          background: cs.primaryContainer.withValues(alpha: 0.7),
          border: cs.primary.withValues(alpha: 0.24),
          accent: cs.primary,
          foreground: cs.onPrimaryContainer,
        );
      default:
        return InspectionWarningData(
          scopeLabel: 'NOTICE',
          codeLabel: code.toUpperCase(),
          message: message,
          icon: Icons.info_outline_rounded,
          background: cs.surfaceContainerHigh.withValues(alpha: 0.74),
          border: cs.outlineVariant.withValues(alpha: 0.32),
          accent: cs.secondary,
          foreground: cs.onSurface,
        );
    }
  }
}

class PreviewMiniSwatch extends StatelessWidget {
  const PreviewMiniSwatch({super.key, required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 18,
      height: 18,
      decoration: BoxDecoration(shape: BoxShape.circle, color: color),
    );
  }
}

class ThemeDot extends StatelessWidget {
  const ThemeDot({super.key, required this.color});

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

class StatusBadge extends StatelessWidget {
  const StatusBadge({super.key, required this.label, required this.color});

  final String label;
  final Color color;

  @override
  Widget build(BuildContext context) {
    final shapes = FerrisShapes.of(context);
    final brightness = ThemeData.estimateBrightnessForColor(color);
    final foreground =
        brightness == Brightness.dark ? Colors.white : Colors.black87;
    return AnimatedContainer(
      duration: FerrisMotion.fast,
      curve: FerrisMotion.emphasized,
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      decoration: BoxDecoration(
        borderRadius: shapes.pill,
        color: color,
        border: Border.all(color: foreground.withValues(alpha: 0.08)),
      ),
      child: Text(
        label,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: Theme.of(context).textTheme.labelMedium?.copyWith(
              color: foreground,
              fontWeight: FontWeight.w700,
            ),
      ),
    );
  }
}

class StatusPulseDot extends StatefulWidget {
  const StatusPulseDot({
    super.key,
    required this.active,
    required this.color,
    this.size = 12,
  });

  final bool active;
  final Color color;
  final double size;

  @override
  State<StatusPulseDot> createState() => _StatusPulseDotState();
}

class SmoothLinearProgressIndicator extends StatelessWidget {
  const SmoothLinearProgressIndicator({
    super.key,
    required this.value,
    this.color,
    this.backgroundColor,
    this.minHeight = 6,
  });

  final double? value;
  final Color? color;
  final Color? backgroundColor;
  final double minHeight;

  @override
  Widget build(BuildContext context) {
    final target = value?.clamp(0.0, 1.0);
    final radius = FerrisShapes.of(context).xs;
    if (target == null) {
      return ClipRRect(
        borderRadius: radius,
        child: LinearProgressIndicator(
          minHeight: minHeight,
          color: color,
          backgroundColor: backgroundColor,
        ),
      );
    }

    return TweenAnimationBuilder<double>(
      tween: Tween<double>(end: target),
      duration: FerrisMotion.medium,
      curve: FerrisMotion.linear,
      builder: (context, animatedValue, _) => ClipRRect(
        borderRadius: radius,
        child: LinearProgressIndicator(
          minHeight: minHeight,
          value: animatedValue,
          color: color,
          backgroundColor: backgroundColor,
        ),
      ),
    );
  }
}

class _StatusPulseDotState extends State<StatusPulseDot>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1200),
  );

  @override
  void initState() {
    super.initState();
    _sync();
  }

  @override
  void didUpdateWidget(covariant StatusPulseDot oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.active != widget.active) {
      _sync();
    }
  }

  void _sync() {
    if (widget.active) {
      _controller.repeat(reverse: true);
    } else {
      _controller.stop();
      _controller.value = 0;
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _controller,
      builder: (context, _) {
        final t = widget.active ? _controller.value : 0;
        final glowScale = 1 + (t * 0.75);
        return SizedBox(
          width: widget.size * 2.4,
          height: widget.size * 2.4,
          child: Center(
            child: Stack(
              alignment: Alignment.center,
              children: [
                Transform.scale(
                  scale: glowScale,
                  child: Container(
                    width: widget.size,
                    height: widget.size,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: widget.color.withValues(
                        alpha: widget.active ? 0.16 - (t * 0.08) : 0,
                      ),
                    ),
                  ),
                ),
                Container(
                  width: widget.size,
                  height: widget.size,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: widget.color,
                    boxShadow: [
                      BoxShadow(
                        color: widget.color.withValues(
                          alpha: widget.active ? 0.34 : 0.12,
                        ),
                        blurRadius: widget.active ? 12 + (t * 6) : 6,
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

class BatteryBanner extends StatelessWidget {
  const BatteryBanner({
    super.key,
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
    final shapes = FerrisShapes.of(context);
    final spacing = FerrisSpacing.of(context);
    return AnimatedContainer(
      duration: FerrisMotion.medium,
      curve: FerrisMotion.emphasized,
      decoration: BoxDecoration(
        borderRadius: shapes.md,
        color: cs.tertiaryContainer,
      ),
      child: Padding(
        padding: EdgeInsets.all(spacing.lg),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(
                  Icons.battery_saver_rounded,
                  color: cs.onTertiaryContainer,
                ),
                const SizedBox(width: 8),
                Text(
                  title,
                  style: t.textTheme.titleSmall?.copyWith(
                    color: cs.onTertiaryContainer,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 8),
            Text(
              body,
              style: t.textTheme.bodyMedium?.copyWith(
                color: cs.onTertiaryContainer,
              ),
            ),
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
