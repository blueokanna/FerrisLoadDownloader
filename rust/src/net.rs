//! Synchronous network helpers for the downloader.
//!
//! The `courierust` client is a blocking engine, so every network call
//! here is synchronous. These helpers mirror the small `reqwest` surface
//! the downloader previously used: per-request headers, retries, Range
//! support, and streaming to disk.

use anyhow::{bail, Context, Result};
use courierust::courierust_body::Body;
use courierust::courierust_client::{Client, ClientConfig, TlsSettings as ClientTls};
use courierust::courierust_http::header::{HeaderName, HeaderValue};
use courierust::courierust_http::method::Method;
use courierust::courierust_http::request::Request;
use courierust::courierust_tls::RootStore;
use std::collections::HashSet;
use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

/// Maximum body accepted for in-memory reads (playlists, keys, JSON).
pub const MAX_MEMORY_BODY: usize = 64 * 1024 * 1024;
/// Default connect timeout.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Default read timeout.
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(45);
/// Maximum redirects followed by either HTTP engine (mirrors the courierust
/// config and RFC 9110 guidance; bounded to prevent redirect loops).
const MAX_REDIRECTS: usize = 10;

/// A GET response: `(status, headers, body)`.
type GetResult = (u16, Vec<(String, String)>, Vec<u8>);

/// A synchronous HTTP client.
///
/// Primary engine is `courierust` (fast, pooled). Its custom TLS verifier,
/// however, only supports P-256 ECDSA certificate signatures and wrongly
/// rejects otherwise-valid chains that contain a P-384 intermediate (e.g.
/// ZeroSSL / Sectigo "E46"). When the primary engine fails we transparently
/// retry the request with a rustls-backed `ureq` agent, which validates such
/// chains correctly. Hosts that needed the fallback are remembered so the
/// retry cost is paid only once per host.
#[derive(Clone)]
pub struct SyncHttpClient {
    inner: Client,
    fallback: ureq::Agent,
    tls_fallback_hosts: std::sync::Arc<Mutex<HashSet<String>>>,
}

impl SyncHttpClient {
    /// Build a client with Mozilla TLS roots and production timeouts.
    pub fn new() -> Result<Self> {
        Self::with_timeouts(DEFAULT_CONNECT_TIMEOUT, DEFAULT_READ_TIMEOUT)
    }

    /// Build a client with explicit timeouts.
    pub fn with_timeouts(connect_timeout: Duration, read_timeout: Duration) -> Result<Self> {
        let mut roots = RootStore::new();
        for certificate in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
            roots.add_der(certificate.as_ref().to_vec());
        }
        if roots.is_empty() {
            bail!("no TLS trust anchors could be loaded");
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let config = ClientConfig {
            http2: false,
            http3: false,
            max_connections_per_host: 4,
            connect_timeout: Some(connect_timeout),
            read_timeout: Some(read_timeout),
            handshake_timeout: Some(Duration::from_secs(10)),
            max_redirects: MAX_REDIRECTS,
            user_agent: Some("FerrisLoad/1.0".to_string()),
            max_header_list: 1 << 20,
            max_body: MAX_MEMORY_BODY,
            tls: Some(ClientTls {
                roots,
                verify: true,
                alpn: vec![b"http/1.1".to_vec()],
                now,
            }),
            ..Default::default()
        };
        // rustls-backed fallback: same Mozilla roots as the primary engine
        // (ureq's `rustls` feature bundles webpki-roots), sane timeouts, and
        // redirects handled manually below so credentials are never leaked
        // to a different origin.
        let fallback = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_connect(Some(connect_timeout))
                .timeout_global(Some(connect_timeout.saturating_add(read_timeout)))
                .max_redirects(0)
                .http_status_as_error(false)
                .build(),
        );

        Ok(Self {
            inner: Client::with_config(config),
            fallback,
            tls_fallback_hosts: std::sync::Arc::new(Mutex::new(HashSet::new())),
        })
    }

    /// Perform a GET and return the full body (bounded by
    /// `MAX_MEMORY_BODY`). Returns `(status, headers, body)`.
    pub fn get(&self, url: &str, headers: &[(String, String)]) -> Result<GetResult> {
        self.get_impl(url, headers, None)
    }

    /// Perform a GET with a Range header and return the body.
    pub fn get_range(
        &self,
        url: &str,
        headers: &[(String, String)],
        start: u64,
        end: u64,
    ) -> Result<GetResult> {
        self.get_impl(url, headers, Some((start, end)))
    }

    fn get_impl(
        &self,
        url: &str,
        headers: &[(String, String)],
        range: Option<(u64, u64)>,
    ) -> Result<GetResult> {
        ensure_http_url(url)?;
        let authority = url::Url::parse(url)
            .ok()
            .map(|parsed| parsed.authority().to_string());

        let needs_fallback = authority
            .as_ref()
            .map(|authority| self.tls_fallback_hosts.lock().unwrap().contains(authority))
            .unwrap_or(false);
        if needs_fallback {
            return self.get_via_ureq(url, headers, range);
        }

        match self.get_via_courierust(url, headers, range) {
            Ok(result) => Ok(result),
            Err(primary_error) => {
                // Remember the host so every later request (e.g. every Range
                // chunk of a large media file) goes straight to the fallback.
                if let Some(authority) = authority {
                    self.tls_fallback_hosts.lock().unwrap().insert(authority);
                }
                match self.get_via_ureq(url, headers, range) {
                    Ok(result) => Ok(result),
                    Err(_) => Err(primary_error),
                }
            }
        }
    }

    fn get_via_courierust(
        &self,
        url: &str,
        headers: &[(String, String)],
        range: Option<(u64, u64)>,
    ) -> Result<GetResult> {
        let mut request = Request::new(Method::GET, "/");
        for (name, value) in headers {
            let Some(header_name) = parse_header_name(name) else {
                continue;
            };
            let Some(header_value) = parse_header_value(value) else {
                continue;
            };
            request = request.header(header_name, header_value);
        }
        if let Some((start, end)) = range {
            request = request.header(
                HeaderName::from_lowercase("range"),
                HeaderValue::from_bytes(format!("bytes={}-{}", start, end).as_bytes())?,
            );
        }
        let response = self
            .inner
            .execute(url, request)
            .with_context(|| format!("HTTP GET failed: {url}"))?;
        let status = response.status.as_u16();
        let response_headers = response
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body = match response.body {
            Body::Empty => Vec::new(),
            Body::Bytes(bytes) => bytes.to_vec(),
            Body::Channel(rx) => {
                let mut out = Vec::new();
                while let Ok(chunk) = rx.recv() {
                    let chunk = chunk?;
                    if out.len().saturating_add(chunk.len()) > MAX_MEMORY_BODY {
                        bail!("response body exceeds the {} byte limit", MAX_MEMORY_BODY);
                    }
                    out.extend_from_slice(&chunk);
                }
                out
            }
        };
        Ok((status, response_headers, body))
    }

    /// Fallback HTTP engine (rustls). Handles redirects manually with the
    /// same cross-origin credential-drop rule as the primary engine, so a
    /// malicious redirect can never harvest `cookie`/`authorization`.
    fn get_via_ureq(
        &self,
        url: &str,
        headers: &[(String, String)],
        range: Option<(u64, u64)>,
    ) -> Result<GetResult> {
        let mut current_url = url.to_string();
        let mut current_headers: Vec<(String, String)> = headers.to_vec();
        for _ in 0..=MAX_REDIRECTS {
            let mut request = self.fallback.get(&current_url);
            for (name, value) in &current_headers {
                let Some(header_name) = parse_header_name(name) else {
                    continue;
                };
                let Some(header_value) = parse_header_value(value) else {
                    continue;
                };
                request = request.header(header_name.as_str(), header_value.to_str().unwrap_or(""));
            }
            if let Some((start, end)) = range {
                request = request.header("Range", format!("bytes={}-{}", start, end));
            }
            let response = request
                .call()
                .with_context(|| format!("HTTP GET failed: {url}"))?;
            let status = response.status().as_u16();

            if (300..400).contains(&status) && status != 304 {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok());
                let Some(location) = location else {
                    return finish_ureq_response(response);
                };
                let resolved = url::Url::parse(&current_url)
                    .context("Invalid redirect base URL")?
                    .join(location)
                    .context("Invalid redirect target URL")?;
                let next_url = resolved.to_string();
                if !matches!(resolved.scheme(), "http" | "https") {
                    bail!(
                        "Refusing non-HTTP(S) redirect target (scheme: {}): {}",
                        resolved.scheme(),
                        next_url
                    );
                }
                // RFC 9110 credential-leakage guidance: never forward
                // credentials to a different origin.
                let same_origin = resolved.authority()
                    == url::Url::parse(&current_url)
                        .map(|current| current.authority().to_string())
                        .unwrap_or_default();
                if !same_origin {
                    current_headers.retain(|(name, _)| {
                        !name.eq_ignore_ascii_case("cookie")
                            && !name.eq_ignore_ascii_case("authorization")
                            && !name.eq_ignore_ascii_case("proxy-authorization")
                    });
                }
                current_url = next_url;
                continue;
            }
            return finish_ureq_response(response);
        }
        bail!("too many redirects while following: {}", url)
    }

    /// Underlying primary client (for advanced uses).
    pub fn inner(&self) -> &Client {
        &self.inner
    }
}

/// Convert a completed `ureq` response into the shared `(status, headers,
/// body)` shape, enforcing the in-memory body cap.
fn finish_ureq_response(response: ureq::http::Response<ureq::Body>) -> Result<GetResult> {
    let status = response.status().as_u16();
    let response_headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect::<Vec<_>>();
    let mut body = Vec::new();
    let mut reader = response.into_body().into_reader();
    let _ = reader
        .by_ref()
        .take((MAX_MEMORY_BODY as u64) + 1)
        .read_to_end(&mut body);
    if body.len() > MAX_MEMORY_BODY {
        bail!("response body exceeds the {} byte limit", MAX_MEMORY_BODY);
    }
    Ok((status, response_headers, body))
}

/// Reject any URL whose scheme is not http or https. This is the final
/// network-layer guard (defense in depth) against SSRF / local-file access
/// even if a caller fails to validate a resolved playlist/segment URL.
fn ensure_http_url(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).with_context(|| format!("Invalid URL: {url}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!(
            "Refusing non-HTTP(S) request URL (scheme: {}): {}",
            parsed.scheme(),
            url
        );
    }
    Ok(())
}

/// Parse an HTTP header name, returning `None` for any value that does not
/// conform to HTTP token rules. Callers must DROP invalid headers rather than
/// forwarding them (fail-closed), preventing header injection.
fn parse_header_name(name: &str) -> Option<HeaderName> {
    HeaderName::from_bytes(name.trim().as_bytes()).ok()
}

/// Parse an HTTP header value, returning `None` for any value containing
/// CR/LF/control bytes (which would enable response-splitting / header
/// injection). Callers must DROP invalid headers rather than forwarding them.
fn parse_header_value(value: &str) -> Option<HeaderValue> {
    HeaderValue::from_bytes(value.as_bytes()).ok()
}

/// A simple semaphore built on `std` for bounded concurrency.
pub struct SyncSemaphore {
    permits: std::sync::Mutex<usize>,
    condvar: std::sync::Condvar,
    max: usize,
}

impl SyncSemaphore {
    pub fn new(permits: usize) -> Self {
        Self {
            permits: std::sync::Mutex::new(permits),
            condvar: std::sync::Condvar::new(),
            max: permits,
        }
    }

    pub fn acquire(&self) {
        let mut guard = self.permits.lock().unwrap();
        while *guard == 0 {
            guard = self.condvar.wait(guard).unwrap();
        }
        *guard -= 1;
    }

    pub fn release(&self) {
        let mut guard = self.permits.lock().unwrap();
        if *guard < self.max {
            *guard += 1;
            self.condvar.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semaphore_acquire_release() {
        let sem = SyncSemaphore::new(2);
        sem.acquire();
        sem.acquire();
        sem.release();
        sem.release();
    }

    #[test]
    fn header_name_rejects_injection() {
        // Valid token names pass.
        assert!(parse_header_name("User-Agent").is_some());
        assert!(parse_header_name("x-custom-header_1").is_some());
        // CR/LF / colon / space / non-graphic bytes must be rejected.
        assert!(parse_header_name("X-Evil\r\nInjected").is_none());
        assert!(parse_header_name("X-Evil:value").is_none());
        assert!(parse_header_name("Bad Header").is_none());
        assert!(parse_header_name("X-中文").is_none());
    }

    #[test]
    fn header_value_rejects_crlf() {
        // Normal values pass.
        assert!(parse_header_value("application/json").is_some());
        assert!(parse_header_value("").is_some());
        // CR / LF / NUL and other control bytes must be rejected.
        assert!(parse_header_value("text/html\r\nX-Evil: 1").is_none());
        assert!(parse_header_value("a\nb").is_none());
        assert!(parse_header_value("a\rb").is_none());
    }

    #[test]
    fn url_scheme_allowlist() {
        assert!(ensure_http_url("https://example.com/a.m3u8").is_ok());
        assert!(ensure_http_url("http://example.com/a.ts").is_ok());
        // Non-HTTP schemes are refused.
        assert!(ensure_http_url("file:///etc/passwd").is_err());
        assert!(ensure_http_url("ftp://example.com/a.ts").is_err());
        assert!(ensure_http_url("data:text/plain;base64,AAAA").is_err());
        assert!(ensure_http_url("javascript:alert(1)").is_err());
    }
}
