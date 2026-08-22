use nextjson::Value;
use regex::Regex;
use std::collections::HashSet;
use url::Url;

#[derive(Clone, Debug)]
pub(crate) struct SiteWarning {
    scope: &'static str,
    code: &'static str,
    message: String,
}

impl SiteWarning {
    pub(crate) fn auth(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            scope: "auth",
            code,
            message: message.into(),
        }
    }

    pub(crate) fn site(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            scope: "site",
            code,
            message: message.into(),
        }
    }

    pub(crate) fn media(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            scope: "media",
            code,
            message: message.into(),
        }
    }

    pub(crate) fn into_display(self) -> String {
        format!("[{}:{}] {}", self.scope, self.code, self.message)
    }

    pub(crate) fn scope(&self) -> &'static str {
        self.scope
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

pub(crate) fn extract_page_title(html: &str) -> Option<String> {
    let patterns = [
        r#"<meta[^>]+property=[\"']og:title[\"'][^>]+content=[\"']([^\"']+)[\"']"#,
        r#"<meta[^>]+name=[\"']twitter:title[\"'][^>]+content=[\"']([^\"']+)[\"']"#,
        r#"<title>([^<]+)</title>"#,
    ];

    for pattern in patterns {
        let regex = Regex::new(pattern).ok()?;
        if let Some(caps) = regex.captures(html) {
            if let Some(value) = caps.get(1) {
                let cleaned = html_unescape(value.as_str()).trim().to_string();
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
        }
    }

    None
}

pub(crate) fn html_unescape(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

pub(crate) fn normalize_exposed_media_url(page_url: &Url, raw: &str) -> Option<String> {
    let normalized = html_unescape(raw)
        .replace("\\u002F", "/")
        .replace("\\u002f", "/")
        .replace("\\u003A", ":")
        .replace("\\u003a", ":")
        .replace("\\u0026", "&")
        .replace("\\/", "/")
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();

    if normalized.is_empty() {
        return None;
    }

    let normalized = if normalized.starts_with("//") {
        format!("https:{}", normalized)
    } else {
        normalized
    };

    let resolved = page_url
        .join(&normalized)
        .map(|url| url.to_string())
        .or_else(|_| Url::parse(&normalized).map(|url| url.to_string()))
        .ok()?;

    if is_supported_media_like(&resolved) {
        Some(resolved)
    } else {
        None
    }
}

pub(crate) fn is_supported_media_like(url: &str) -> bool {
    url.contains(".m3u8")
        || url.contains("application/vnd.apple.mpegurl")
        || url.contains(".mp4")
        || url.contains(".webm")
        || url.contains(".mkv")
        || url.contains(".m4v")
        || url.contains(".mpd")
}

pub(crate) fn first_array_pointer<'a>(
    value: &'a Value,
    pointers: &[&str],
) -> Option<&'a Vec<Value>> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_array))
}

pub(crate) fn first_string_pointer(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_string)
    })
}

pub(crate) fn extract_text_runs_pointer(value: &Value, pointer: &str) -> Option<String> {
    let runs = value.pointer(pointer).and_then(Value::as_array)?;
    let text = runs
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<String>()
        .trim()
        .to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub(crate) fn first_media_url(value: &Value) -> Option<String> {
    collect_media_urls(value).into_iter().next()
}

pub(crate) fn collect_media_urls(value: &Value) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut urls = Vec::new();

    for field in [
        "url",
        "baseUrl",
        "base_url",
        "baseUrlHttps",
        "base_url_https",
        "playUrl",
        "play_url",
    ] {
        if let Some(raw) = value.get(field).and_then(Value::as_str) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                urls.push(trimmed.to_string());
            }
        }
    }

    for field in [
        "backupUrl",
        "backup_url",
        "backupPlayUrl",
        "backup_play_url",
    ] {
        if let Some(entries) = value.get(field).and_then(Value::as_array) {
            for entry in entries {
                if let Some(raw) = entry.as_str() {
                    let trimmed = raw.trim();
                    if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                        urls.push(trimmed.to_string());
                    }
                }
            }
        }
    }

    urls
}

pub(crate) fn extract_json_object_after_any(haystack: &str, markers: &[&str]) -> Option<String> {
    markers
        .iter()
        .find_map(|marker| extract_json_object_after(haystack, marker))
}

pub(crate) fn extract_json_object_after(haystack: &str, marker: &str) -> Option<String> {
    let start = haystack.find(marker)? + marker.len();
    let remainder = &haystack[start..];
    let json_start = remainder.find('{')?;
    let bytes = remainder.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut end_index = None;

    for (offset, byte) in bytes.iter().enumerate().skip(json_start) {
        let ch = *byte as char;
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end_index = Some(offset + 1);
                    break;
                }
            }
            _ => {}
        }
    }

    end_index.map(|end| remainder[json_start..end].to_string())
}

pub(crate) fn extract_json_string_after_any(haystack: &str, markers: &[&str]) -> Option<String> {
    markers
        .iter()
        .find_map(|marker| extract_json_string_after(haystack, marker))
}

pub(crate) fn extract_json_string_after(haystack: &str, marker: &str) -> Option<String> {
    let start = haystack.find(marker)? + marker.len();
    let remainder = &haystack[start..];
    let skip = remainder.find(|ch: char| !ch.is_whitespace())?;
    let remainder = &remainder[skip..];
    let quote = remainder.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }

    let mut escaped = false;
    let mut collected = String::new();
    for ch in remainder[1..].chars() {
        if escaped {
            collected.push('\\');
            collected.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            break;
        }
        collected.push(ch);
    }

    let decoded = decode_js_string_literal(&collected)?;
    let trimmed = decoded.trim();
    if trimmed.starts_with('{') {
        Some(trimmed.to_string())
    } else {
        None
    }
}

pub(crate) fn decode_js_string_literal(value: &str) -> Option<String> {
    let mut result = String::new();
    let mut chars = value.chars();

    while let Some(ch) = chars.next() {
        if ch != '\\' {
            result.push(ch);
            continue;
        }

        let escaped = chars.next()?;
        match escaped {
            '\\' => result.push('\\'),
            '"' => result.push('"'),
            '\'' => result.push('\''),
            '/' => result.push('/'),
            'b' => result.push('\u{0008}'),
            'f' => result.push('\u{000C}'),
            'n' => result.push('\n'),
            'r' => result.push('\r'),
            't' => result.push('\t'),
            'u' => {
                let code = chars.by_ref().take(4).collect::<String>();
                if code.len() != 4 {
                    return None;
                }
                let value = u16::from_str_radix(&code, 16).ok()?;
                result.push(char::from_u32(value as u32)?);
            }
            other => result.push(other),
        }
    }

    Some(html_unescape(&result))
}
