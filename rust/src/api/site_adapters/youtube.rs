use anyhow::{Context, Result};
use nextjson::Value;
use regex::Regex;
use url::Url;

use crate::api::downloader::CandidateCollector;

use super::common::{
    SiteWarning, extract_json_object_after_any, extract_json_string_after_any, extract_page_title,
    extract_text_runs_pointer, first_media_url, first_string_pointer, normalize_exposed_media_url,
};

pub(crate) fn extract_youtube_candidates(
    page_url: &Url,
    html: &str,
    collector: &mut CandidateCollector,
    warnings: &mut Vec<SiteWarning>,
) -> Result<()> {
    let Some(json) = extract_youtube_player_json(html) else {
        warnings.push(SiteWarning::site(
            "youtube-player-json-missing",
            "YouTube player JSON was not exposed in the current page source",
        ));
        return Ok(());
    };

    let value: Value = nextjson::from_str(&json).context("Failed to parse youtube player JSON")?;
    push_playability_warnings(&value, warnings);
    let title = first_string_pointer(
        &value,
        &[
            "/videoDetails/title",
            "/microformat/playerMicroformatRenderer/title/simpleText",
        ],
    )
    .or_else(|| {
        extract_text_runs_pointer(&value, "/microformat/playerMicroformatRenderer/title/runs")
    })
    .or_else(|| extract_page_title(html));

    let mut found_manifest = false;
    let mut found_stream = false;

    for (pointer, label, mime_type) in [
        (
            "/streamingData/hlsManifestUrl",
            "HLS",
            "application/vnd.apple.mpegurl",
        ),
        (
            "/streamingData/manifestUrl",
            "HLS",
            "application/vnd.apple.mpegurl",
        ),
        (
            "/streamingData/dashManifestUrl",
            "DASH",
            "application/dash+xml",
        ),
    ] {
        if let Some(manifest) = first_string_pointer(&value, &[pointer]) {
            found_manifest = true;
            collector.push(
                manifest,
                None,
                title.clone(),
                Some(label.to_string()),
                Some(mime_type.to_string()),
                None,
                None,
                Some("youtube"),
            );
        }
    }

    for (label, mime_type, raw) in extract_direct_manifest_urls(page_url, html)? {
        found_manifest = true;
        collector.push(
            raw,
            None,
            title.clone(),
            Some(label),
            Some(mime_type),
            None,
            None,
            Some("youtube"),
        );
    }

    let best_audio = value
        .pointer("/streamingData/adaptiveFormats")
        .and_then(Value::as_array)
        .and_then(|formats| {
            formats
                .iter()
                .filter(|item| {
                    item.get("mimeType")
                        .and_then(Value::as_str)
                        .map(|mime| mime.starts_with("audio/"))
                        .unwrap_or(false)
                })
                .filter_map(|item| {
                    let media_url = first_media_url(item)?;
                    Some((item.get("bitrate")?.as_i64().unwrap_or_default(), media_url))
                })
                .max_by_key(|entry| entry.0)
                .map(|entry| entry.1)
        });

    let mut saw_cipher_only = false;

    for path in ["/streamingData/formats", "/streamingData/adaptiveFormats"] {
        if let Some(formats) = value.pointer(path).and_then(Value::as_array) {
            for item in formats {
                let mime_type = item
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let Some(media_url) = first_media_url(item) else {
                    if item.get("signatureCipher").is_some() || item.get("cipher").is_some() {
                        saw_cipher_only = true;
                    }
                    continue;
                };
                found_stream = true;
                let is_video_only =
                    mime_type.starts_with("video/") && path.ends_with("adaptiveFormats");
                collector.push(
                    media_url,
                    if is_video_only {
                        best_audio.clone()
                    } else {
                        None
                    },
                    title.clone(),
                    item.get("qualityLabel")
                        .or_else(|| item.get("audioQuality"))
                        .or_else(|| item.get("quality"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    Some(mime_type.to_string()),
                    item.get("width")
                        .and_then(Value::as_i64)
                        .map(|value| value as i32),
                    item.get("height")
                        .and_then(Value::as_i64)
                        .map(|value| value as i32),
                    Some("youtube"),
                );
            }
        }
    }

    if saw_cipher_only {
        warnings.push(SiteWarning::media(
            "youtube-signature-protected",
            "Some YouTube streams require signature resolution and were intentionally skipped",
        ));
    }

    if !found_manifest && !found_stream {
        warnings.push(SiteWarning::site(
            "youtube-streams-missing",
            "YouTube player data was found, but no reusable media URLs were exposed",
        ));
    }

    Ok(())
}

fn push_playability_warnings(value: &Value, warnings: &mut Vec<SiteWarning>) {
    let Some(status) = value
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|status| !status.is_empty())
    else {
        return;
    };

    let reason = first_string_pointer(
        value,
        &[
            "/playabilityStatus/reason",
            "/playabilityStatus/errorScreen/playerErrorMessageRenderer/reason/simpleText",
            "/playabilityStatus/errorScreen/playerLegacyDesktopYpcOfferRenderer/itemTitle",
            "/playabilityStatus/errorScreen/playerLegacyDesktopYpcTrailerRenderer/trailerVideoTitle",
            "/playabilityStatus/messages/0",
        ],
    )
    .or_else(|| {
        extract_text_runs_pointer(
            value,
            "/playabilityStatus/errorScreen/playerErrorMessageRenderer/reason/runs",
        )
    })
    .or_else(|| {
        extract_text_runs_pointer(
            value,
            "/playabilityStatus/errorScreen/playerErrorMessageRenderer/subreason/runs",
        )
    })
    .or_else(|| {
        extract_text_runs_pointer(
            value,
            "/playabilityStatus/errorScreen/playerLegacyDesktopYpcOfferRenderer/offers/0/runs",
        )
    })
    .unwrap_or_else(|| status.to_string());

    let lower_reason = reason.to_ascii_lowercase();
    let access_code = if lower_reason.contains("members-only")
        || lower_reason.contains("member-only")
        || lower_reason.contains("join this channel")
        || lower_reason.contains("membership")
    {
        Some("youtube-membership-required")
    } else if matches!(
        status,
        "LOGIN_REQUIRED" | "AGE_CHECK_REQUIRED" | "CONTENT_CHECK_REQUIRED"
    ) || ((status == "UNPLAYABLE" || status == "ERROR") && lower_reason.contains("age"))
    {
        Some("youtube-age-gate")
    } else {
        None
    };

    if let Some(code) = access_code {
        warnings.push(SiteWarning::auth(code, reason));
    }
}

fn extract_youtube_player_json(html: &str) -> Option<String> {
    extract_json_object_after_any(
        html,
        &[
            "ytInitialPlayerResponse = ",
            "ytInitialPlayerResponse=",
            "var ytInitialPlayerResponse = ",
            "var ytInitialPlayerResponse=",
            "window[\"ytInitialPlayerResponse\"] = ",
            "window[\"ytInitialPlayerResponse\"]=",
            "window['ytInitialPlayerResponse'] = ",
            "window['ytInitialPlayerResponse']=",
        ],
    )
    .or_else(|| {
        extract_json_string_after_any(
            html,
            &[
                "ytInitialPlayerResponse = JSON.parse(",
                "ytInitialPlayerResponse=JSON.parse(",
                "var ytInitialPlayerResponse = JSON.parse(",
                "var ytInitialPlayerResponse=JSON.parse(",
                "window[\"ytInitialPlayerResponse\"] = JSON.parse(",
                "window['ytInitialPlayerResponse'] = JSON.parse(",
                "\"player_response\":",
                "\"playerResponse\":",
                "\"embedded_player_response\":",
                "\"serialized_player_response\":",
                "\"ytInitialPlayerResponse\":",
                "'player_response':",
                "'playerResponse':",
                "'embedded_player_response':",
                "'serialized_player_response':",
            ],
        )
    })
}

fn extract_direct_manifest_urls(
    page_url: &Url,
    html: &str,
) -> Result<Vec<(String, String, String)>> {
    let mut manifests = Vec::new();

    for (field, label, mime_type) in [
        ("hlsManifestUrl", "HLS", "application/vnd.apple.mpegurl"),
        ("manifestUrl", "HLS", "application/vnd.apple.mpegurl"),
        ("dashManifestUrl", "DASH", "application/dash+xml"),
        ("hlsvp", "HLS", "application/vnd.apple.mpegurl"),
        ("dashmpd", "DASH", "application/dash+xml"),
    ] {
        let pattern = format!(r#"\"{}\"\s*:\s*\"([^\"]+)\""#, regex::escape(field));
        let regex = Regex::new(&pattern)?;
        for capture in regex.captures_iter(html) {
            let Some(raw) = capture.get(1).map(|value| value.as_str()) else {
                continue;
            };
            let Some(resolved) = normalize_exposed_media_url(page_url, raw) else {
                continue;
            };
            manifests.push((label.to_string(), mime_type.to_string(), resolved));
        }
    }

    Ok(manifests)
}
