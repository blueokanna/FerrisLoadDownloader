import 'package:flutter/material.dart';
import 'package:flutter_inappwebview/flutter_inappwebview.dart';
import 'package:m3u8_downloader/src/app/app_localizations.dart';
import 'package:m3u8_downloader/src/rust/api/downloader.dart';

class AuthSessionBundle {
  const AuthSessionBundle({
    required this.url,
    required this.userAgent,
    required this.referer,
    required this.origin,
    required this.cookie,
  });

  final String url;
  final String userAgent;
  final String referer;
  final String origin;
  final String cookie;
}

class AuthBrowserPage extends StatefulWidget {
  const AuthBrowserPage({
    super.key,
    required this.initialUrl,
    required this.seedContext,
  });

  final String initialUrl;
  final RequestContext seedContext;

  @override
  State<AuthBrowserPage> createState() => _AuthBrowserPageState();
}

class _AuthBrowserPageState extends State<AuthBrowserPage> {
  late final TextEditingController _addressCtrl;
  InAppWebViewController? _controller;
  String _currentUrl = '';
  bool _loading = true;
  bool _canGoBack = false;
  bool _canGoForward = false;
  double _progress = 0;

  @override
  void initState() {
    super.initState();
    _addressCtrl = TextEditingController(text: widget.initialUrl);
    _currentUrl = widget.initialUrl;
  }

  @override
  void dispose() {
    _addressCtrl.dispose();
    super.dispose();
  }

  Future<void> _syncNavigationState() async {
    final controller = _controller;
    if (controller == null) return;
    final currentUrl = (await controller.getUrl())?.toString() ?? _currentUrl;
    final canGoBack = await controller.canGoBack();
    final canGoForward = await controller.canGoForward();
    if (!mounted) return;
    setState(() {
      _currentUrl = currentUrl;
      _addressCtrl.text = currentUrl;
      _canGoBack = canGoBack;
      _canGoForward = canGoForward;
    });
  }

  Future<void> _openTypedUrl() async {
    final controller = _controller;
    if (controller == null) return;
    final text = _addressCtrl.text.trim();
    if (text.isEmpty) return;
    final normalized = text.startsWith('http://') || text.startsWith('https://')
        ? text
        : 'https://$text';
    await controller.loadUrl(urlRequest: URLRequest(url: WebUri(normalized)));
  }

  Future<void> _importSession() async {
    final l = AppLocalizations.of(context);
    final controller = _controller;
    final activeUrl = _currentUrl.isNotEmpty ? _currentUrl : widget.initialUrl;
    if (controller == null || activeUrl.isEmpty) {
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text(l.text('auth_browser_no_session'))),
      );
      return;
    }

    final cookieManager = CookieManager.instance();
    final cookiesByName = <String, Cookie>{};
    for (final candidateUrl in {widget.initialUrl, activeUrl}) {
      if (candidateUrl.isEmpty) continue;
      final uri = Uri.tryParse(candidateUrl);
      if (uri == null || !uri.hasScheme) continue;
      final cookies = await cookieManager.getCookies(url: WebUri(candidateUrl));
      for (final cookie in cookies) {
        cookiesByName[cookie.name] = cookie;
      }
    }

    final cookieHeader = cookiesByName.values
        .map((cookie) => '${cookie.name}=${cookie.value}')
        .join('; ');
    final rawUserAgent = await controller.evaluateJavascript(
      source: 'window.navigator.userAgent',
    );
    final userAgent = _normalizeJsValue(rawUserAgent);
    final refererUrl = (await controller.getUrl())?.toString() ?? activeUrl;
    final origin = _originFromUrl(refererUrl);

    if (!mounted) return;
    if (cookieHeader.isEmpty && userAgent.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text(l.text('auth_browser_no_session'))),
      );
      return;
    }

    Navigator.of(context).pop(
      AuthSessionBundle(
        url: refererUrl,
        userAgent: userAgent,
        referer: refererUrl,
        origin: origin,
        cookie: cookieHeader,
      ),
    );
  }

  String _normalizeJsValue(dynamic value) {
    if (value == null) return '';
    final text = value.toString().trim();
    if (text.length >= 2 && text.startsWith('"') && text.endsWith('"')) {
      return text.substring(1, text.length - 1);
    }
    return text;
  }

  String _originFromUrl(String value) {
    final uri = Uri.tryParse(value);
    if (uri == null || !uri.hasScheme || uri.host.isEmpty) return '';
    return uri.origin;
  }

  @override
  Widget build(BuildContext context) {
    final l = AppLocalizations.of(context);
    final theme = Theme.of(context);
    final colorScheme = theme.colorScheme;

    return Scaffold(
      appBar: AppBar(
        title: Text(l.text('auth_browser_title')),
        bottom: PreferredSize(
          preferredSize: const Size.fromHeight(3),
          child: _loading
              ? LinearProgressIndicator(value: _progress > 0 && _progress < 1 ? _progress : null)
              : const SizedBox(height: 3),
        ),
      ),
      body: Column(
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 8),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  l.text('auth_browser_hint'),
                  style: theme.textTheme.bodyMedium?.copyWith(
                    color: colorScheme.onSurfaceVariant,
                  ),
                ),
                const SizedBox(height: 12),
                Row(
                  children: [
                    Expanded(
                      child: TextField(
                        controller: _addressCtrl,
                        keyboardType: TextInputType.url,
                        textInputAction: TextInputAction.go,
                        onSubmitted: (_) => _openTypedUrl(),
                        decoration: InputDecoration(
                          labelText: l.text('auth_browser_address'),
                          prefixIcon: const Icon(Icons.language_rounded),
                        ),
                      ),
                    ),
                    const SizedBox(width: 8),
                    FilledButton.tonalIcon(
                      onPressed: _openTypedUrl,
                      icon: const Icon(Icons.arrow_forward_rounded),
                      label: Text(l.text('auth_browser_go')),
                    ),
                  ],
                ),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
            child: Row(
              children: [
                IconButton(
                  onPressed: _canGoBack ? () => _controller?.goBack() : null,
                  icon: const Icon(Icons.arrow_back_rounded),
                ),
                IconButton(
                  onPressed: _canGoForward ? () => _controller?.goForward() : null,
                  icon: const Icon(Icons.arrow_forward_rounded),
                ),
                IconButton(
                  onPressed: () => _controller?.reload(),
                  icon: const Icon(Icons.refresh_rounded),
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    _currentUrl,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: colorScheme.onSurfaceVariant,
                    ),
                  ),
                ),
              ],
            ),
          ),
          Expanded(
            child: InAppWebView(
              initialUrlRequest: URLRequest(url: WebUri(widget.initialUrl)),
              initialSettings: InAppWebViewSettings(
                javaScriptEnabled: true,
                allowsBackForwardNavigationGestures: true,
                thirdPartyCookiesEnabled: true,
                userAgent: widget.seedContext.userAgent.isEmpty
                    ? null
                    : widget.seedContext.userAgent,
              ),
              onWebViewCreated: (controller) {
                _controller = controller;
              },
              onLoadStart: (controller, url) {
                if (!mounted) return;
                setState(() {
                  _loading = true;
                  _currentUrl = url?.toString() ?? _currentUrl;
                });
              },
              onLoadStop: (controller, url) async {
                if (!mounted) return;
                setState(() {
                  _loading = false;
                  _currentUrl = url?.toString() ?? _currentUrl;
                  _progress = 1;
                });
                await _syncNavigationState();
              },
              onProgressChanged: (controller, progress) {
                if (!mounted) return;
                setState(() {
                  _progress = progress / 100;
                  _loading = progress < 100;
                });
              },
              onUpdateVisitedHistory: (controller, url, _) async {
                if (!mounted) return;
                setState(() {
                  _currentUrl = url?.toString() ?? _currentUrl;
                });
                await _syncNavigationState();
              },
            ),
          ),
          SafeArea(
            top: false,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 10, 16, 16),
              child: Row(
                children: [
                  Expanded(
                    child: OutlinedButton(
                      onPressed: () => Navigator.of(context).maybePop(),
                      child: Text(MaterialLocalizations.of(context).cancelButtonLabel),
                    ),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: FilledButton.icon(
                      onPressed: _importSession,
                      icon: const Icon(Icons.verified_user_rounded),
                      label: Text(l.text('auth_browser_import')),
                    ),
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