//! Synchronous network helpers for the downloader.
//!
//! The `courierust` client is a blocking engine, so every network call
//! here is synchronous. These helpers mirror the small `reqwest` surface
//! the downloader previously used: per-request headers, retries, Range
//! support, and streaming to disk.
//!
//! Earlier versions carried a second, rustls-backed engine (`ureq`) as a
//! TLS fallback because `courierust`'s certificate-chain verifier only
//! accepted P-256 signatures and rejected chains with P-384 intermediates
//! (e.g. ZeroSSL / Sectigo "E46"). `courierust` 1.0.3 verifies P-384 chains
//! natively (it ships its own ECDSA/P-384 implementation plus tests for
//! that exact chain shape), so the fallback has been deleted instead of
//! pulling a second TLS stack whose latest release outgrew this crate's
//! MSRV.

use anyhow::{Context, Result, bail};
use courierust::courierust_body::Body;
use courierust::courierust_client::{Client, ClientConfig, TlsSettings as ClientTls};
use courierust::courierust_http::header::{HeaderName, HeaderValue};
use courierust::courierust_http::method::Method;
use courierust::courierust_http::request::Request;
use courierust::courierust_tls::{RootStore, TlsVersion};
use std::time::Duration;

/// Maximum body accepted for in-memory reads (playlists, keys, JSON).
pub const MAX_MEMORY_BODY: usize = 64 * 1024 * 1024;
/// Default connect timeout.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Default read timeout.
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(45);
/// Maximum redirects followed by the HTTP engine (mirrors the courierust
/// config and RFC 9110 guidance; bounded to prevent redirect loops).
const MAX_REDIRECTS: usize = 10;

/// A GET response: `(status, headers, body)`.
type GetResult = (u16, Vec<(String, String)>, Vec<u8>);

/// A synchronous HTTP client backed by `courierust` with Mozilla trust
/// roots, per-request headers, Range support and sane timeouts.
#[derive(Clone)]
pub struct SyncHttpClient {
    inner: Client,
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
                min_version: TlsVersion::Tls12,
                max_version: TlsVersion::Tls13,
                now,
            }),
            ..Default::default()
        };

        Ok(Self {
            inner: Client::with_config(config),
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
        self.get_via_courierust(url, headers, range)
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

    /// Underlying primary client (for advanced uses).
    pub fn inner(&self) -> &Client {
        &self.inner
    }
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
