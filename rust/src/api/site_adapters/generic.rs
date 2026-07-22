use anyhow::Result;
use regex::Regex;
use url::Url;

use crate::api::downloader::CandidateCollector;

use super::common::normalize_exposed_media_url;

fn push_generic_candidate(
    page_url: &Url,
    raw: &str,
    quality_label: &str,
    collector: &mut CandidateCollector,
) {
    if let Some(resolved) = normalize_exposed_media_url(page_url, raw) {
        collector.push(
            resolved,
            None,
            None,
            Some(quality_label.to_string()),
            None,
            None,
            None,
            Some("generic"),
        );
    }
}

pub(crate) fn extract_generic_candidates(
    page_url: &Url,
    html: &str,
    collector: &mut CandidateCollector,
) -> Result<()> {
    let absolute_url_regex = Regex::new(
        r#"https?:\\?/\\?/[^\"'<>\s]+?(?:m3u8|mp4|webm|m4v|m4a|mpd)(?:\?[^\"'<>\s]*)?"#,
    )?;
    let relative_url_regex = Regex::new(
        r#"(?:src|href|content|data-src|data-url|data-video|data-hls|data-mp4|data-play-url|data-play_url|data-manifest|data-stream|data-stream-url)\s*=\s*[\"']([^\"']+?(?:m3u8|mp4|webm|m4v|m4a|mpd)(?:\?[^\"']*)?)[\"']"#,
    )?;
    let content_url_regex = Regex::new(
        r#"\"(?:contentUrl|embedUrl|playbackUrl|streamUrl|stream_url|videoUrl|video_url|sourceUrl|source_url|playUrl|play_url|playlistUrl|playlist_url|manifestUrl|manifest_url|hlsUrl|hls_url|dashUrl|dash_url|mp4Url|mp4_url|file|src)\"\s*:\s*\"([^\"]+)\""#,
    )?;
    let meta_url_regex = Regex::new(
        r#"<meta[^>]+(?:property|name)=[\"'](?:og:video|og:video:url|og:video:secure_url|twitter:player:stream|twitter:player:stream:content_type)[\"'][^>]+content=[\"']([^\"']+)[\"']"#,
    )?;
    let source_tag_regex = Regex::new(
        r#"<source[^>]+src=[\"']([^\"']+?(?:m3u8|mp4|webm|m4v|m4a|mpd)(?:\?[^\"']*)?)[\"']"#,
    )?;

    for capture in absolute_url_regex.captures_iter(html) {
        if let Some(raw) = capture.get(0) {
            push_generic_candidate(page_url, raw.as_str(), "Detected", collector);
        }
    }

    for capture in relative_url_regex.captures_iter(html) {
        if let Some(raw) = capture.get(1) {
            push_generic_candidate(page_url, raw.as_str(), "Detected", collector);
        }
    }

    for capture in content_url_regex.captures_iter(html) {
        if let Some(raw) = capture.get(1) {
            push_generic_candidate(page_url, raw.as_str(), "Content URL", collector);
        }
    }

    for capture in meta_url_regex.captures_iter(html) {
        if let Some(raw) = capture.get(1) {
            push_generic_candidate(page_url, raw.as_str(), "Open Graph", collector);
        }
    }

    for capture in source_tag_regex.captures_iter(html) {
        if let Some(raw) = capture.get(1) {
            push_generic_candidate(page_url, raw.as_str(), "Video Source", collector);
        }
    }

    Ok(())
}
