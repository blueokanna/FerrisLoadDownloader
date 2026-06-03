use anyhow::Result;
use url::Url;

use crate::api::downloader::CandidateCollector;

mod bilibili;
mod common;
mod generic;
mod youtube;

pub(crate) use common::{extract_page_title, SiteWarning};

#[derive(Clone, Copy, Debug)]
enum SiteAdapterKind {
    YouTube,
    Bilibili,
}

pub(crate) fn extractor_name_for_host(host: Option<&str>) -> String {
    match adapter_for_host(host) {
        Some(SiteAdapterKind::YouTube) => "youtube".to_string(),
        Some(SiteAdapterKind::Bilibili) => "bilibili".to_string(),
        None => host.unwrap_or("generic").to_string(),
    }
}

pub(crate) fn inspect_page_candidates(
    page_url: &Url,
    html: &str,
    collector: &mut CandidateCollector,
    warnings: &mut Vec<SiteWarning>,
) -> Result<()> {
    match adapter_for_host(page_url.domain()) {
        Some(SiteAdapterKind::YouTube) => {
            youtube::extract_youtube_candidates(page_url, html, collector, warnings)?;
        }
        Some(SiteAdapterKind::Bilibili) => {
            bilibili::extract_bilibili_candidates(page_url, html, collector, warnings)?;
        }
        None => {}
    }

    generic::extract_generic_candidates(page_url, html, collector)?;
    Ok(())
}

fn adapter_for_host(host: Option<&str>) -> Option<SiteAdapterKind> {
    match host {
        Some(domain) if domain.contains("youtube.com") || domain.contains("youtu.be") => {
            Some(SiteAdapterKind::YouTube)
        }
        Some(domain) if domain.contains("bilibili.com") || domain.contains("b23.tv") => {
            Some(SiteAdapterKind::Bilibili)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{extractor_name_for_host, inspect_page_candidates, SiteWarning};
    use crate::api::downloader::{CandidateCollector, MediaCandidate};
    use url::Url;

    fn inspect_fixture(url: &str, html: &str) -> (Vec<MediaCandidate>, Vec<String>) {
        let page_url = Url::parse(url).expect("fixture url should parse");
        let mut collector = CandidateCollector::new(
            url,
            "Fixture default title",
            &extractor_name_for_host(page_url.domain()),
        );
        let mut warnings = Vec::<SiteWarning>::new();
        inspect_page_candidates(&page_url, html, &mut collector, &mut warnings)
            .expect("fixture inspection should succeed");
        (
            collector.finish(),
            warnings.into_iter().map(SiteWarning::into_display).collect(),
        )
    }

    #[test]
    fn youtube_inline_fixture_extracts_streams_and_audio_pairing() {
        let html = include_str!("fixtures/youtube_inline.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.youtube.com/watch?v=fixture-inline", html);

        assert!(warnings.is_empty());
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "youtube"
                && candidate.protocol == "hls"
                && candidate.media_url.contains("hls_variant.m3u8")
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "youtube"
                && candidate.audio_url.as_deref()
                    == Some("https://rr1---sn.example/audio_track.m4a")
                && candidate.height == 1080
        }));
    }

    #[test]
    fn youtube_json_parse_fixture_extracts_manifest_and_warning() {
        let html = include_str!("fixtures/youtube_json_parse.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.youtube.com/watch?v=fixture-parse", html);

        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "youtube"
                && candidate.protocol == "dash"
                && candidate.media_url.contains("dash_parse.mpd")
        }));
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("youtube-signature-protected")));
    }

    #[test]
    fn youtube_embed_fixture_extracts_embedded_player_response() {
        let html = include_str!("fixtures/youtube_embed.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.youtube.com/embed/fixture-embed", html);

        assert!(warnings.is_empty());
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "youtube"
                && candidate.media_url == "https://rr2---sn.example/embed_muxed.mp4"
                && candidate.title == "Fixture Embed Video"
                && candidate.height == 480
        }));
    }

    #[test]
    fn youtube_live_fixture_extracts_legacy_live_manifest_fields() {
        let html = include_str!("fixtures/youtube_live.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.youtube.com/watch?v=fixture-live", html);

        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "youtube"
                && candidate.protocol == "hls"
                && candidate.media_url.contains("live_master.m3u8")
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "youtube"
                && candidate.protocol == "dash"
                && candidate.media_url.contains("live_master.mpd")
        }));
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("youtube-signature-protected")));
    }

    #[test]
    fn youtube_watch_age_gate_fixture_emits_auth_warning() {
        let html = include_str!("fixtures/youtube_watch_age_gate.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.youtube.com/watch?v=fixture-age-watch", html);

        assert!(candidates.is_empty());
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("[auth:youtube-age-gate]")));
    }

    #[test]
    fn youtube_live_age_gate_fixture_emits_auth_warning() {
        let html = include_str!("fixtures/youtube_live_age_gate.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.youtube.com/watch?v=fixture-age-live", html);

        assert!(candidates.is_empty());
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("[auth:youtube-age-gate]")));
    }

    #[test]
    fn youtube_members_only_fixture_emits_membership_warning() {
        let html = include_str!("fixtures/youtube_members_only.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.youtube.com/watch?v=fixture-members", html);

        assert!(candidates.is_empty());
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("[auth:youtube-membership-required]")));
    }

    #[test]
    fn youtube_short_link_replay_fixture_uses_youtu_be_dispatch() {
        let html = include_str!("fixtures/youtube_short_replay.html");
        let (candidates, warnings) = inspect_fixture("https://youtu.be/fixture-replay", html);

        assert!(warnings.is_empty());
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "youtube"
                && candidate.protocol == "hls"
                && candidate.media_url == "https://rr3---sn.example/replay_master.m3u8"
                && candidate.title == "Fixture Live Replay"
        }));
    }

    #[test]
    fn bilibili_inline_fixture_prefers_best_audio_track() {
        let html = include_str!("fixtures/bilibili_inline.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.bilibili.com/video/BV1fixture", html);

        assert!(warnings.is_empty());
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "bilibili"
                && candidate.audio_url.as_deref() == Some("https://cn.example/audio-256.m4a")
                && candidate.height == 1080
                && candidate.title == "Fixture Bili Inline"
        }));
    }

    #[test]
    fn bilibili_json_parse_fixture_supports_flac_audio_and_progressive() {
        let html = include_str!("fixtures/bilibili_json_parse.html");
        let (candidates, warnings) = inspect_fixture("https://www.bilibili.com/video/BV1parse", html);

        assert!(warnings.is_empty());
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "bilibili"
                && candidate.audio_url.as_deref() == Some("https://upos.example/audio-flac.m4a")
                && candidate.height == 2160
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "bilibili"
                && candidate.audio_url.is_none()
                && candidate.media_url == "https://upos.example/progressive-part-1.mp4"
        }));
    }

    #[test]
    fn bilibili_bangumi_fixture_builds_title_from_initial_state() {
        let html = include_str!("fixtures/bilibili_bangumi.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.bilibili.com/bangumi/play/ep1fixture", html);

        assert!(warnings.is_empty());
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "bilibili"
                && candidate.audio_url.as_deref() == Some("https://bangumi.example/audio-128.m4a")
                && candidate.title == "Fixture Bangumi Season · 第1话 · 星际启程"
        }));
    }

    #[test]
    fn bilibili_collection_fixture_builds_title_from_ugc_season_state() {
        let html = include_str!("fixtures/bilibili_collection.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.bilibili.com/video/BV1collection", html);

        assert!(warnings.is_empty());
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "bilibili"
                && candidate.audio_url.as_deref() == Some("https://collection.example/audio-128.m4a")
                && candidate.title == "Fixture Collection · Part 03"
                && candidate.height == 720
        }));
    }

    #[test]
    fn bilibili_bangumi_pagination_fixture_uses_current_episode_from_list() {
        let html = include_str!("fixtures/bilibili_bangumi_pagination.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.bilibili.com/bangumi/play/ep-pagination", html);

        assert!(warnings.is_empty());
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "bilibili"
                && candidate.audio_url.as_deref() == Some("https://bangumi-page.example/audio.m4a")
                && candidate.title == "Fixture Paginated Season · 第2话 · 月落"
        }));
    }

    #[test]
    fn bilibili_multi_p_fixture_uses_current_page_part() {
        let html = include_str!("fixtures/bilibili_multi_p.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.bilibili.com/video/BV1multiP", html);

        assert!(warnings.is_empty());
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "bilibili"
                && candidate.audio_url.as_deref() == Some("https://multi-p.example/audio.m4a")
                && candidate.title == "Fixture Multi-P Collection · Part B"
                && candidate.height == 720
        }));
    }

    #[test]
    fn bilibili_members_only_fixture_emits_membership_warning() {
        let html = include_str!("fixtures/bilibili_members_only.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.bilibili.com/bangumi/play/ep-members", html);

        assert!(candidates.is_empty());
        assert!(warnings
            .iter()
            .any(|warning| warning.contains("[auth:bilibili-membership-required]")));
    }

    #[test]
    fn bilibili_short_link_fixture_uses_b23_dispatch() {
        let html = include_str!("fixtures/bilibili_short_link.html");
        let (candidates, warnings) = inspect_fixture("https://b23.tv/fixture-short", html);

        assert!(warnings.is_empty());
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "bilibili"
                && candidate.audio_url.as_deref() == Some("https://b23.example/audio-128.m4a")
                && candidate.title == "Fixture Short Link Video"
        }));
    }

    #[test]
    fn bilibili_multi_audio_fixture_exposes_multiple_audio_variants() {
        let html = include_str!("fixtures/bilibili_multi_audio.html");
        let (candidates, warnings) =
            inspect_fixture("https://www.bilibili.com/video/BV1multiAudio", html);

        assert!(warnings.is_empty());
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "bilibili"
                && candidate.audio_url.as_deref()
                    == Some("https://multi-audio.example/audio-zh.m4a")
                && candidate.quality_label == "1080p · Mandarin"
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "bilibili"
                && candidate.audio_url.as_deref()
                    == Some("https://multi-audio.example/audio-ja.m4a")
                && candidate.quality_label == "1080p · Japanese"
        }));
    }

    #[test]
    fn generic_fixture_extracts_meta_source_and_data_manifest_urls() {
        let html = include_str!("fixtures/generic_embed.html");
        let (candidates, warnings) = inspect_fixture("https://example.com/posts/fixture", html);

        assert!(warnings.is_empty());
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "generic"
                && candidate.media_url == "https://cdn.example/stream/master.m3u8"
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "generic"
                && candidate.media_url == "https://example.com/media/trailer.mp4"
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate.extractor == "generic"
                && candidate.media_url == "https://cdn.example/dash/manifest.mpd"
        }));
    }
}