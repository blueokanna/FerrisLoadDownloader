import 'package:flutter/material.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/app/app_theme.dart';
import 'package:m3u8_downloader/src/home/home_widgets.dart';

class AuthorizationSettingsPanel extends StatelessWidget {
  const AuthorizationSettingsPanel({
    super.key,
    required this.enabled,
    required this.autoOpenAuthBrowser,
    required this.onAutoOpenChanged,
    required this.onOpenBrowser,
    required this.onClearContext,
    required this.authContextListenable,
    required this.authContextBadges,
    required this.hasAuthContextOverrides,
    required this.userAgentController,
    required this.refererController,
    required this.originController,
    required this.cookieController,
    required this.headersController,
  });

  final bool enabled;
  final bool autoOpenAuthBrowser;
  final ValueChanged<bool>? onAutoOpenChanged;
  final Future<void> Function()? onOpenBrowser;
  final VoidCallback? onClearContext;
  final Listenable authContextListenable;
  final List<String> Function() authContextBadges;
  final bool Function() hasAuthContextOverrides;
  final TextEditingController userAgentController;
  final TextEditingController refererController;
  final TextEditingController originController;
  final TextEditingController cookieController;
  final TextEditingController headersController;

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;

    return AnimatedContainer(
      duration: const Duration(milliseconds: 280),
      curve: Curves.easeOutCubic,
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        borderRadius: FerrisShapes.of(context).md,
        color: cs.surfaceContainerHigh.withValues(alpha: 0.52),
        border: Border.all(color: cs.outlineVariant.withValues(alpha: 0.22)),
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
                child: Icon(Icons.verified_user_outlined, color: cs.secondary),
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
          const SizedBox(height: 16),
          AnimatedContainer(
            duration: FerrisMotion.medium,
            curve: FerrisMotion.emphasized,
            padding: const EdgeInsets.all(14),
            decoration: BoxDecoration(
              borderRadius: FerrisShapes.of(context).md,
              color: cs.surfaceContainer,
              border: Border.all(
                color: cs.outlineVariant.withValues(alpha: 0.18),
              ),
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        l.text('auth_auto_open'),
                        style: t.textTheme.titleSmall?.copyWith(
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                      const SizedBox(height: 6),
                      Text(
                        l.text('auth_auto_open_hint'),
                        style: t.textTheme.bodySmall?.copyWith(
                          color: cs.onSurfaceVariant,
                          height: 1.35,
                        ),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 12),
                Switch.adaptive(
                  value: autoOpenAuthBrowser,
                  onChanged: enabled ? onAutoOpenChanged : null,
                ),
              ],
            ),
          ),
          const SizedBox(height: 14),
          AnimatedBuilder(
            animation: authContextListenable,
            builder: (context, _) {
              final badges = authContextBadges();
              if (badges.isEmpty) {
                return const SizedBox.shrink();
              }
              return Padding(
                padding: const EdgeInsets.only(bottom: 14),
                child: Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    for (final badge in badges) MiniChip(label: badge),
                  ],
                ),
              );
            },
          ),
          LayoutBuilder(
            builder: (context, constraints) {
              final compact = constraints.maxWidth < 440;
              final browserButton = FilledButton.tonalIcon(
                onPressed: enabled && onOpenBrowser != null
                    ? () async => onOpenBrowser!()
                    : null,
                icon: const Icon(Icons.open_in_browser_rounded),
                label: Text(l.text('auth_browser_open')),
              );
              final clearButton = OutlinedButton.icon(
                onPressed: enabled && hasAuthContextOverrides()
                    ? onClearContext
                    : null,
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
                  const SizedBox(width: 14),
                  Expanded(child: clearButton),
                ],
              );
            },
          ),
          const SizedBox(height: 16),
          AnimatedContainer(
            duration: FerrisMotion.medium,
            curve: FerrisMotion.emphasized,
            padding: const EdgeInsets.all(14),
            decoration: BoxDecoration(
              borderRadius: FerrisShapes.of(context).md,
              color: cs.surface.withValues(alpha: 0.76),
              border: Border.all(
                color: cs.outlineVariant.withValues(alpha: 0.16),
              ),
            ),
            child: AuthContextEditor(
              enabled: enabled,
              userAgentController: userAgentController,
              refererController: refererController,
              originController: originController,
              cookieController: cookieController,
              headersController: headersController,
            ),
          ),
          const SizedBox(height: 14),
          Text(
            l.text('auth_browser_hint_inline'),
            style: t.textTheme.bodySmall?.copyWith(
              color: cs.onSurfaceVariant,
              height: 1.35,
            ),
          ),
        ],
      ),
    );
  }
}

class AuthContextEditor extends StatelessWidget {
  const AuthContextEditor({
    super.key,
    required this.enabled,
    required this.userAgentController,
    required this.refererController,
    required this.originController,
    required this.cookieController,
    required this.headersController,
  });

  final bool enabled;
  final TextEditingController userAgentController;
  final TextEditingController refererController;
  final TextEditingController originController;
  final TextEditingController cookieController;
  final TextEditingController headersController;

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);

    return Column(
      children: [
        TextFormField(
          controller: userAgentController,
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
                    controller: refererController,
                    enabled: enabled,
                    decoration: const InputDecoration(
                      labelText: 'Referer',
                      prefixIcon: Icon(Icons.link_rounded),
                    ),
                  ),
                  const SizedBox(height: 12),
                  TextFormField(
                    controller: originController,
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
                    controller: refererController,
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
                    controller: originController,
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
          controller: cookieController,
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
          controller: headersController,
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
}
