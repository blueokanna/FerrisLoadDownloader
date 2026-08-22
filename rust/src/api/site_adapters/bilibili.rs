use anyhow::{Context, Result};
use nextjson::Value;
use url::Url;

use crate::api::downloader::CandidateCollector;

use super::common::{
    extract_json_object_after_any, extract_json_string_after_any, extract_page_title,
    first_array_pointer, first_media_url, first_string_pointer, SiteWarning,
};

pub(crate) fn extract_bilibili_candidates(
    _page_url: &Url,
    html: &str,
    collector: &mut CandidateCollector,
    warnings: &mut Vec<SiteWarning>,
) -> Result<()> {
    let Some(json) = extract_bilibili_playinfo_json(html) else {
        if let Some(access_warning) = extract_bilibili_access_warning(html) {
            warnings.push(access_warning);
        } else {
            warnings.push(SiteWarning::site(
                "bilibili-playinfo-missing",
                "Bilibili playinfo JSON was not exposed in the current page source",
            ));
        }
        return Ok(());
    };

    let value: Value =
        nextjson::from_str(&json).context("Failed to parse bilibili playinfo JSON")?;
    let title = extract_bilibili_title(html);
    let mut found_progressive = false;
    let mut found_dash = false;
    let audio_tracks = collect_bilibili_audio_tracks(&value);

    for pointer in [
        "/data/durl",
        "/result/durl",
        "/result/video_info/durl",
        "/data/playurl/durl",
    ] {
        if let Some(durl_list) = value.pointer(pointer).and_then(Value::as_array) {
            found_progressive = true;
            for (index, item) in durl_list.iter().enumerate() {
                if let Some(media_url) = first_media_url(item) {
                    collector.push(
                        media_url,
                        None,
                        title.clone(),
                        Some(format!("Part {}", index + 1)),
                        Some("video/mp4".to_string()),
                        None,
                        None,
                        Some("bilibili"),
                    );
                }
            }
        }
    }

    let best_audio = audio_tracks.first().map(|track| track.url.clone());

    for pointer in [
        "/data/dash/video",
        "/result/dash/video",
        "/result/video_info/dash/video",
        "/data/playurl/dash/video",
    ] {
        if let Some(videos) = value.pointer(pointer).and_then(Value::as_array) {
            found_dash = true;
            for video in videos {
                let Some(media_url) = first_media_url(video) else {
                    continue;
                };

                let base_quality = video
                    .get("height")
                    .and_then(Value::as_i64)
                    .map(|height| format!("{}p", height));

                if audio_tracks.len() > 1 {
                    for audio_track in &audio_tracks {
                        collector.push(
                            media_url.clone(),
                            Some(audio_track.url.clone()),
                            title.clone(),
                            Some(compose_bilibili_quality_label(
                                base_quality.as_deref(),
                                audio_track.label.as_deref(),
                            )),
                            video
                                .get("mimeType")
                                .or_else(|| video.get("mime_type"))
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            video
                                .get("width")
                                .and_then(Value::as_i64)
                                .map(|value| value as i32),
                            video
                                .get("height")
                                .and_then(Value::as_i64)
                                .map(|value| value as i32),
                            Some("bilibili"),
                        );
                    }
                    continue;
                }

                collector.push(
                    media_url,
                    best_audio.clone(),
                    title.clone(),
                    base_quality,
                    video
                        .get("mimeType")
                        .or_else(|| video.get("mime_type"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    video
                        .get("width")
                        .and_then(Value::as_i64)
                        .map(|value| value as i32),
                    video
                        .get("height")
                        .and_then(Value::as_i64)
                        .map(|value| value as i32),
                    Some("bilibili"),
                );
            }
        }
    }

    if found_dash && audio_tracks.is_empty() {
        warnings.push(SiteWarning::media(
            "bilibili-audio-missing",
            "Bilibili exposed DASH video streams without a reusable audio track",
        ));
    }

    if !found_progressive && !found_dash {
        if let Some(access_warning) = extract_bilibili_access_warning(html) {
            warnings.push(access_warning);
        } else {
            warnings.push(SiteWarning::site(
                "bilibili-streams-missing",
                "Bilibili page JSON was found, but no playable stream URLs were exposed",
            ));
        }
    }

    Ok(())
}

fn extract_bilibili_title(html: &str) -> Option<String> {
    extract_page_title(html).or_else(|| {
        let value = extract_bilibili_initial_state_value(html)?;

        let season_title = first_string_pointer(
            &value,
            &[
                "/mediaInfo/title",
                "/ugcSeason/title",
                "/seasonTitle",
                "/roomInfo/title",
            ],
        );
        let episode_from_list = extract_bilibili_episode_from_lists(&value);
        let page_part = extract_bilibili_current_page_part(&value);
        let episode_title =
            first_string_pointer(&value, &["/epInfo/title", "/videoData/title", "/h1Title"])
                .or(episode_from_list)
                .or(page_part);
        let episode_subtitle = first_string_pointer(
            &value,
            &[
                "/epInfo/long_title",
                "/epInfo/share_copy",
                "/videoData/subtitle",
            ],
        );

        compose_bilibili_title(season_title, episode_title, episode_subtitle)
    })
}

fn extract_bilibili_access_warning(html: &str) -> Option<SiteWarning> {
    let value = extract_bilibili_initial_state_value(html)?;
    let membership_badge = first_string_pointer(
        &value,
        &[
            "/epInfo/badgeInfo/text",
            "/epInfo/badge",
            "/mediaInfo/badgeInfo/text",
            "/mediaInfo/badge",
        ],
    );

    let requires_premium = membership_badge
        .as_deref()
        .map(looks_like_bilibili_premium_access)
        .unwrap_or(false)
        || first_i64_pointer(&value, &["/epInfo/payMark", "/mediaInfo/payment/price"])
            .map(|value| value > 0)
            .unwrap_or(false);

    if !requires_premium {
        return None;
    }

    let detail = membership_badge.unwrap_or_else(|| "member or paid access".to_string());
    Some(SiteWarning::auth(
        "bilibili-membership-required",
        format!("Bilibili page requires {} access", detail),
    ))
}

fn extract_bilibili_initial_state_value(html: &str) -> Option<Value> {
    let json = extract_bilibili_initial_state_json(html)?;
    nextjson::from_str(&json).ok()
}

#[derive(Clone)]
struct BilibiliAudioTrack {
    url: String,
    label: Option<String>,
    bandwidth: i64,
}

fn collect_bilibili_audio_tracks(value: &Value) -> Vec<BilibiliAudioTrack> {
    let mut tracks = Vec::new();

    for pointer in [
        "/data/dash/flac/audio",
        "/result/dash/flac/audio",
        "/result/video_info/dash/flac/audio",
        "/data/playurl/dash/flac/audio",
        "/data/dash/dolby/audio",
        "/result/dash/dolby/audio",
        "/result/video_info/dash/dolby/audio",
        "/data/playurl/dash/dolby/audio",
        "/data/dash/audio",
        "/result/dash/audio",
        "/result/video_info/dash/audio",
        "/data/playurl/dash/audio",
    ] {
        let Some(audios) = value.pointer(pointer).and_then(Value::as_array) else {
            continue;
        };

        for audio in audios {
            let Some(media_url) = first_media_url(audio) else {
                continue;
            };
            if tracks
                .iter()
                .any(|track: &BilibiliAudioTrack| track.url == media_url)
            {
                continue;
            }

            tracks.push(BilibiliAudioTrack {
                url: media_url,
                label: first_string_pointer(audio, &["/lang_text", "/lang", "/codecs"]).or_else(
                    || {
                        audio
                            .get("id")
                            .and_then(Value::as_i64)
                            .map(|id| format!("Audio {}", id))
                    },
                ),
                bandwidth: audio
                    .get("bandwidth")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            });
        }
    }

    tracks.sort_by_key(|track| std::cmp::Reverse(track.bandwidth));
    tracks
}

fn extract_bilibili_episode_from_lists(value: &Value) -> Option<String> {
    let current_id = first_i64_pointer(
        value,
        &[
            "/epInfo/id",
            "/epInfo/ep_id",
            "/epInfo/episode_id",
            "/epId",
            "/currentEpId",
        ],
    )?;

    for pointer in ["/epList", "/sections/0/episodes", "/sections/0/epList"] {
        let Some(entries) = value.pointer(pointer).and_then(Value::as_array) else {
            continue;
        };

        for entry in entries {
            let Some(entry_id) = entry
                .get("id")
                .or_else(|| entry.get("ep_id"))
                .or_else(|| entry.get("episode_id"))
                .and_then(Value::as_i64)
            else {
                continue;
            };

            if entry_id != current_id {
                continue;
            }

            let title = first_string_pointer(entry, &["/title", "/show_title"]);
            let subtitle = first_string_pointer(entry, &["/long_title", "/share_copy"]);
            return compose_bilibili_title(None, title, subtitle);
        }
    }

    None
}

fn extract_bilibili_current_page_part(value: &Value) -> Option<String> {
    let current_page =
        first_i64_pointer(value, &["/videoData/p", "/p", "/page/p", "/pageData/page"])?;
    let pages = first_array_pointer(value, &["/videoData/pages", "/pages"])?;

    for page in pages {
        let Some(page_number) = page
            .get("page")
            .or_else(|| page.get("p"))
            .or_else(|| page.get("pageNumber"))
            .and_then(Value::as_i64)
        else {
            continue;
        };

        if page_number != current_page {
            continue;
        }

        if let Some(part) = first_string_pointer(page, &["/part", "/title", "/page_title"]) {
            return Some(part);
        }
    }

    None
}

fn extract_bilibili_playinfo_json(html: &str) -> Option<String> {
    extract_json_object_after_any(
        html,
        &[
            "__playinfo__=",
            "__playinfo__ = ",
            "window.__playinfo__=",
            "window.__playinfo__ = ",
            "self.__playinfo__=",
            "self.__playinfo__ = ",
        ],
    )
    .or_else(|| {
        extract_json_string_after_any(
            html,
            &[
                "window.__playinfo__ = JSON.parse(",
                "window.__playinfo__=JSON.parse(",
                "self.__playinfo__ = JSON.parse(",
                "self.__playinfo__=JSON.parse(",
            ],
        )
    })
}

fn extract_bilibili_initial_state_json(html: &str) -> Option<String> {
    extract_json_object_after_any(
        html,
        &[
            "__INITIAL_STATE__=",
            "__INITIAL_STATE__ = ",
            "window.__INITIAL_STATE__=",
            "window.__INITIAL_STATE__ = ",
            "self.__INITIAL_STATE__=",
            "self.__INITIAL_STATE__ = ",
        ],
    )
    .or_else(|| {
        extract_json_string_after_any(
            html,
            &[
                "window.__INITIAL_STATE__ = JSON.parse(",
                "window.__INITIAL_STATE__=JSON.parse(",
                "self.__INITIAL_STATE__ = JSON.parse(",
                "self.__INITIAL_STATE__=JSON.parse(",
            ],
        )
    })
}

fn compose_bilibili_title(
    season_title: Option<String>,
    episode_title: Option<String>,
    episode_subtitle: Option<String>,
) -> Option<String> {
    let mut segments = Vec::new();

    push_unique_segment(&mut segments, season_title.as_deref());
    push_unique_segment(&mut segments, episode_title.as_deref());
    push_unique_segment(&mut segments, episode_subtitle.as_deref());

    if segments.is_empty() {
        None
    } else {
        Some(segments.join(" · "))
    }
}

fn compose_bilibili_quality_label(base_quality: Option<&str>, audio_label: Option<&str>) -> String {
    match (base_quality, audio_label) {
        (Some(base), Some(audio)) if !audio.is_empty() => format!("{} · {}", base, audio),
        (Some(base), _) => base.to_string(),
        (None, Some(audio)) if !audio.is_empty() => audio.to_string(),
        _ => "Auto".to_string(),
    }
}

fn first_i64_pointer(value: &Value, pointers: &[&str]) -> Option<i64> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_i64))
}

fn looks_like_bilibili_premium_access(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.contains("会员")
        || value.contains("大会员")
        || value.contains("付费")
        || lower.contains("vip")
        || lower.contains("premium")
}

fn push_unique_segment(segments: &mut Vec<String>, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };

    if segments
        .iter()
        .any(|segment| segment == value || segment.contains(value) || value.contains(segment))
    {
        return;
    }

    segments.push(value.to_string());
}
