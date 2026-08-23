import 'package:flutter/material.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/app/app_settings.dart';
import 'package:m3u8_downloader/src/app/app_theme.dart';
import 'package:m3u8_downloader/src/app/platform_utils.dart';
import 'package:m3u8_downloader/src/app/runtime_capabilities.dart';
import 'package:m3u8_downloader/src/home/auth_context_editor.dart';
import 'package:m3u8_downloader/src/home/home_widgets.dart';
import 'package:m3u8_downloader/src/rust/api/downloader.dart';

class HomeSettingsSheet extends StatefulWidget {
  const HomeSettingsSheet({
    super.key,
    required this.settings,
    required this.onSettingsChanged,
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
    this.inspection,
    this.enabled = true,
  });

  final AppSettings settings;
  final ValueChanged<AppSettings> onSettingsChanged;
  final Future<void> Function() onOpenBrowser;
  final VoidCallback onClearContext;
  final Listenable authContextListenable;
  final List<String> Function() authContextBadges;
  final bool Function() hasAuthContextOverrides;
  final TextEditingController userAgentController;
  final TextEditingController refererController;
  final TextEditingController originController;
  final TextEditingController cookieController;
  final TextEditingController headersController;
  final MediaInspectionResult? inspection;
  final bool enabled;

  @override
  State<HomeSettingsSheet> createState() => _HomeSettingsSheetState();
}

class _HomeSettingsSheetState extends State<HomeSettingsSheet> {
  late AppSettings _draft = widget.settings;
  late final Future<PlatformCapabilitySnapshot> _capabilities =
      RuntimeCapabilityProbe.inspect();
  late final TextEditingController _apiUrlCtrl =
      TextEditingController(text: widget.settings.apiBaseUrl);
  late final TextEditingController _apiTokenCtrl =
      TextEditingController(text: widget.settings.apiToken);

  @override
  void dispose() {
    _apiUrlCtrl.dispose();
    _apiTokenCtrl.dispose();
    super.dispose();
  }

  void _updateDraft(AppSettings next) {
    setState(() => _draft = next);
    widget.onSettingsChanged(next);
  }

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final t = Theme.of(context);
    final cs = t.colorScheme;
    final mode = _draft.themeMode;
    final profile = _draft.themeProfile;
    final locale = _draft.locale;

    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 4, 20, 32),
      child: SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            RevealMotion(
              delay: const Duration(milliseconds: 30),
              child: Padding(
                padding: const EdgeInsets.only(left: 2),
                child: Text(
                  l.text('appearance'),
                  style: t.textTheme.titleLarge?.copyWith(
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
            ),
            const SizedBox(height: 18),
            RevealMotion(
              delay: const Duration(milliseconds: 80),
              child: DisplayPreviewCard(
                profile: profile,
                mode: mode,
                localeLabel: l.localeLabel(locale),
              ),
            ),
            const SizedBox(height: 18),
            RevealMotion(
              delay: const Duration(milliseconds: 110),
              child: RuntimeCapabilitiesPanel(capabilities: _capabilities),
            ),
            if (FerrisPlatform.isWeb) ...[
              const SizedBox(height: 18),
              RevealMotion(
                delay: const Duration(milliseconds: 125),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Padding(
                      padding: const EdgeInsets.only(left: 2),
                      child: Text(
                        l.text('api_base_url'),
                        style: t.textTheme.labelLarge,
                      ),
                    ),
                    const SizedBox(height: 10),
                    TextField(
                      controller: _apiUrlCtrl,
                      enabled: widget.enabled,
                      keyboardType: TextInputType.url,
                      autocorrect: false,
                      decoration: InputDecoration(
                        hintText: 'http://localhost:3000',
                        helperText: l.text('api_base_url_hint'),
                        border: OutlineInputBorder(
                          borderRadius: FerrisShapes.of(context).md,
                        ),
                      ),
                      onChanged: (value) {
                        final trimmed = value.trim();
                        if (trimmed.isNotEmpty &&
                            trimmed != widget.settings.apiBaseUrl) {
                          _updateDraft(_draft.copyWith(apiBaseUrl: trimmed));
                        }
                      },
                    ),
                    const SizedBox(height: 14),
                    TextField(
                      controller: _apiTokenCtrl,
                      enabled: widget.enabled,
                      autocorrect: false,
                      obscureText: true,
                      decoration: InputDecoration(
                        hintText: 'Bearer token',
                        helperText: l.text('api_token_hint'),
                        border: OutlineInputBorder(
                          borderRadius: FerrisShapes.of(context).md,
                        ),
                      ),
                      onChanged: (value) {
                        final trimmed = value.trim();
                        if (trimmed != widget.settings.apiToken) {
                          _updateDraft(_draft.copyWith(apiToken: trimmed));
                        }
                      },
                    ),
                  ],
                ),
              ),
            ],
            const SizedBox(height: 18),
            RevealMotion(
              delay: const Duration(milliseconds: 140),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Padding(
                    padding: const EdgeInsets.only(left: 2),
                    child: Text(l.text('mode'), style: t.textTheme.labelLarge),
                  ),
                  const SizedBox(height: 10),
                  AnimatedContainer(
                    duration: const Duration(milliseconds: 260),
                    curve: Curves.easeOutCubic,
                    padding: const EdgeInsets.all(12),
                    decoration: BoxDecoration(
                      borderRadius: FerrisShapes.of(context).md,
                      color: cs.surfaceContainerHigh.withValues(alpha: 0.52),
                    ),
                    child: SegmentedButton<ThemeMode>(
                      selected: {mode},
                      onSelectionChanged: widget.enabled
                          ? (selection) {
                              _updateDraft(
                                _draft.copyWith(themeMode: selection.first),
                              );
                            }
                          : null,
                      segments: [
                        ButtonSegment(
                          value: ThemeMode.system,
                          label: Text(l.text('system')),
                          icon: const Icon(Icons.brightness_auto),
                        ),
                        ButtonSegment(
                          value: ThemeMode.light,
                          label: Text(l.text('light')),
                          icon: const Icon(Icons.light_mode),
                        ),
                        ButtonSegment(
                          value: ThemeMode.dark,
                          label: Text(l.text('dark')),
                          icon: const Icon(Icons.dark_mode),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(height: 20),
            RevealMotion(
              delay: const Duration(milliseconds: 160),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Padding(
                    padding: const EdgeInsets.only(left: 2),
                    child: Text(l.text('theme'), style: t.textTheme.labelLarge),
                  ),
                  const SizedBox(height: 10),
                  SizedBox(
                    height: 172,
                    child: ListView.separated(
                      scrollDirection: Axis.horizontal,
                      itemCount: appThemeProfiles.length,
                      separatorBuilder: (_, __) => const SizedBox(width: 12),
                      itemBuilder: (context, index) {
                        final item = appThemeProfiles[index];
                        return ThemePaletteCard(
                          profile: item,
                          selected: profile.id == item.id,
                          onTap: widget.enabled
                              ? () {
                                  _updateDraft(
                                    _draft.copyWith(themeProfileId: item.id),
                                  );
                                }
                              : null,
                        );
                      },
                    ),
                  ),
                  const SizedBox(height: 14),
                  Row(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      for (final item in appThemeProfiles)
                        AnimatedContainer(
                          duration: const Duration(milliseconds: 220),
                          curve: Curves.easeOutCubic,
                          margin: const EdgeInsets.symmetric(horizontal: 3),
                          width: profile.id == item.id ? 20 : 6,
                          height: 6,
                          decoration: BoxDecoration(
                            borderRadius: FerrisShapes.of(context).pill,
                            color: profile.id == item.id
                                ? cs.primary
                                : cs.outlineVariant,
                          ),
                        ),
                    ],
                  ),
                ],
              ),
            ),
            const SizedBox(height: 20),
            RevealMotion(
              delay: const Duration(milliseconds: 200),
              child: AnimatedContainer(
                duration: const Duration(milliseconds: 260),
                curve: Curves.easeOutCubic,
                padding: const EdgeInsets.all(18),
                decoration: BoxDecoration(
                  borderRadius: FerrisShapes.of(context).md,
                  color: cs.surfaceContainerHigh.withValues(alpha: 0.52),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(l.text('language'), style: t.textTheme.labelLarge),
                    const SizedBox(height: 10),
                    DropdownMenu<Locale>(
                      initialSelection: locale,
                      enabled: widget.enabled,
                      width: double.infinity,
                      hintText: l.text('language'),
                      onSelected: (value) {
                        if (value == null) {
                          return;
                        }
                        _updateDraft(
                          _draft.copyWith(
                            localeTag: AppSettings.serializeLocale(value),
                          ),
                        );
                      },
                      dropdownMenuEntries: [
                        for (final item in AppLocalizations.supportedLocales)
                          DropdownMenuEntry(
                            value: item,
                            label: l.localeLabel(item),
                          ),
                      ],
                    ),
                  ],
                ),
              ),
            ),
            const SizedBox(height: 20),
            RevealMotion(
              delay: const Duration(milliseconds: 240),
              child: AuthorizationSettingsPanel(
                enabled: widget.enabled,
                autoOpenAuthBrowser: _draft.autoOpenAuthBrowser,
                onAutoOpenChanged: widget.enabled
                    ? (value) => _updateDraft(
                          _draft.copyWith(autoOpenAuthBrowser: value),
                        )
                    : null,
                onOpenBrowser: widget.onOpenBrowser,
                onClearContext: widget.onClearContext,
                authContextListenable: widget.authContextListenable,
                authContextBadges: widget.authContextBadges,
                hasAuthContextOverrides: widget.hasAuthContextOverrides,
                userAgentController: widget.userAgentController,
                refererController: widget.refererController,
                originController: widget.originController,
                cookieController: widget.cookieController,
                headersController: widget.headersController,
              ),
            ),
            if (widget.inspection?.authRequired ?? false) ...[
              const SizedBox(height: 18),
              RevealMotion(
                delay: const Duration(milliseconds: 280),
                child: InspectionWarningTile(
                  warning:
                      '[auth:challenge] ${widget.inspection!.challengeReason}',
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
