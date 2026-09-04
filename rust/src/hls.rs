//! Self-contained HLS (HTTP Live Streaming) playlist parser (RFC 8216).
//!
//! This module replaces the third-party `m3u8-rs` crate with a
//! dependency-free, resource-bounded implementation. The public types
//! mirror the subset of the `m3u8-rs` API surface that the downloader
//! actually consumes, so call sites stay stable.
//!
//! Security properties:
//! - input size is bounded (`MAX_INPUT_BYTES`) before parsing;
//! - attribute values are validated, quoted/unquoted handling is strict;
//! - numeric fields reject overflow and malformed input;
//! - unknown tags are skipped, never interpreted;
//! - no panics on adversarial input (all parsing is fallible).

use std::collections::BTreeMap;
use std::string::{String, ToString};
use std::vec::Vec;

/// Hard upper bound on a playlist document (protects memory and CPU).
pub const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
/// Hard upper bound on a single playlist line.
const MAX_LINE_BYTES: usize = 64 * 1024;
/// Hard upper bound on the number of media segments.
const MAX_SEGMENTS: usize = 100_000;

/// A parsed HLS playlist: either a master or a media playlist.
#[derive(Debug, Clone, PartialEq)]
pub enum Playlist {
    /// Master playlist: a set of variant streams.
    MasterPlaylist(MasterPlaylist),
    /// Media playlist: a set of media segments.
    MediaPlaylist(MediaPlaylist),
}

/// A master playlist.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MasterPlaylist {
    /// `#EXT-X-VERSION`
    pub version: Option<usize>,
    /// `#EXT-X-STREAM-INF` variants.
    pub variants: Vec<VariantStream>,
    /// `#EXT-X-MEDIA` renditions.
    pub alternatives: Vec<AlternativeMedia>,
    /// `#EXT-X-INDEPENDENT-SEGMENTS`
    pub independent_segments: bool,
}

/// A variant stream (`#EXT-X-STREAM-INF` or `#EXT-X-I-FRAME-STREAM-INF`).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct VariantStream {
    /// Whether this is an I-frame variant.
    pub is_i_frame: bool,
    /// The playlist URI.
    pub uri: String,
    /// `BANDWIDTH` in bits per second.
    pub bandwidth: u64,
    /// `AVERAGE-BANDWIDTH` if present.
    pub average_bandwidth: Option<u64>,
    /// `CODECS` if present.
    pub codecs: Option<String>,
    /// `RESOLUTION` if present.
    pub resolution: Option<Resolution>,
    /// `FRAME-RATE` if present.
    pub frame_rate: Option<f64>,
    /// `AUDIO` rendition group id if present.
    pub audio: Option<String>,
    /// `VIDEO` rendition group id if present.
    pub video: Option<String>,
    /// `SUBTITLES` rendition group id if present.
    pub subtitles: Option<String>,
}

/// A video resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    /// Width in pixels.
    pub width: u64,
    /// Height in pixels.
    pub height: u64,
}

/// An `#EXT-X-MEDIA` rendition.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct AlternativeMedia {
    /// `TYPE`
    pub media_type: AlternativeMediaType,
    /// `URI` (absent for closed captions).
    pub uri: Option<String>,
    /// `GROUP-ID`
    pub group_id: String,
    /// `LANGUAGE`
    pub language: Option<String>,
    /// `NAME`
    pub name: String,
    /// `DEFAULT=YES`
    pub default: bool,
    /// `AUTOSELECT=YES`
    pub autoselect: bool,
}

/// The `TYPE` of an `#EXT-X-MEDIA` rendition.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AlternativeMediaType {
    /// Audio rendition.
    Audio,
    /// Video rendition.
    #[default]
    Video,
    /// Subtitles rendition.
    Subtitles,
    /// Closed captions rendition.
    ClosedCaptions,
    /// Any other type.
    Other(String),
}

/// A media playlist.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MediaPlaylist {
    /// `#EXT-X-VERSION`
    pub version: Option<usize>,
    /// `#EXT-X-TARGETDURATION`
    pub target_duration: u64,
    /// `#EXT-X-MEDIA-SEQUENCE`
    pub media_sequence: u64,
    /// The media segments, in playlist order.
    pub segments: Vec<MediaSegment>,
    /// `#EXT-X-ENDLIST`
    pub end_list: bool,
    /// `#EXT-X-PLAYLIST-TYPE`
    pub playlist_type: Option<MediaPlaylistType>,
    /// `#EXT-X-I-FRAMES-ONLY`
    pub i_frames_only: bool,
    /// `#EXT-X-INDEPENDENT-SEGMENTS`
    pub independent_segments: bool,
}

/// `#EXT-X-PLAYLIST-TYPE` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaPlaylistType {
    /// `EVENT`
    Event,
    /// `VOD`
    Vod,
    /// Any other value.
    Other(String),
}

/// A media segment.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MediaSegment {
    /// The segment URI.
    pub uri: String,
    /// `#EXTINF` duration in seconds.
    pub duration: f32,
    /// `#EXTINF` title, if any.
    pub title: Option<String>,
    /// `#EXT-X-BYTERANGE`
    pub byte_range: Option<ByteRange>,
    /// `#EXT-X-DISCONTINUITY`
    pub discontinuity: bool,
    /// `#EXT-X-KEY` active for this segment.
    pub key: Option<Key>,
    /// `#EXT-X-MAP` active for this segment.
    pub map: Option<Map>,
}

/// `#EXT-X-KEY` encryption method.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum KeyMethod {
    /// `NONE`
    #[default]
    None,
    /// `AES-128`
    AES128,
    /// `SAMPLE-AES`
    SampleAES,
    /// Any other method.
    Other(String),
}

/// `#EXT-X-KEY` attributes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Key {
    /// `METHOD`
    pub method: KeyMethod,
    /// `URI`
    pub uri: Option<String>,
    /// `IV` (hex, may start with `0x`).
    pub iv: Option<String>,
    /// `KEYFORMAT`
    pub keyformat: Option<String>,
}

/// `#EXT-X-MAP` attributes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Map {
    /// `URI`
    pub uri: String,
    /// `BYTERANGE`
    pub byte_range: Option<ByteRange>,
}

/// `#EXT-X-BYTERANGE` (length[@offset]).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    /// Length in bytes.
    pub length: u64,
    /// Start offset in bytes; `None` means "follow the previous range".
    pub offset: Option<u64>,
}

/// A playlist parse error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistError {
    message: String,
}

impl core::fmt::Display for PlaylistError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PlaylistError {}

fn playlist_error(message: impl Into<String>) -> PlaylistError {
    PlaylistError {
        message: message.into(),
    }
}

/// Parse a playlist from bytes. Returns `(version, playlist)`.
pub fn parse_playlist(input: &[u8]) -> Result<(&str, Playlist), PlaylistError> {
    let input = input.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(input); // UTF-8 BOM
    if input.len() > MAX_INPUT_BYTES {
        return Err(playlist_error(format!(
            "playlist exceeds the {} byte input limit",
            MAX_INPUT_BYTES
        )));
    }
    let text =
        core::str::from_utf8(input).map_err(|_| playlist_error("playlist is not valid UTF-8"))?;

    // Master playlists carry `#EXT-X-STREAM-INF` / `#EXT-X-I-FRAME-STREAM-INF`
    // (or `#EXT-X-MEDIA`); media playlists carry `#EXTINF` segments. A
    // playlist without a single segment and without variants is empty but
    // still valid to parse. Peek for the distinguishing tags first so the
    // correct builder is chosen even when tags are ordered unusually.
    let has_master_tag = text.contains("#EXT-X-STREAM-INF")
        || text.contains("#EXT-X-I-FRAME-STREAM-INF")
        || text.contains("#EXT-X-MEDIA:");

    if has_master_tag && !text.contains("#EXTINF") {
        let master = parse_master(text)?;
        Ok((
            version_string(master.version),
            Playlist::MasterPlaylist(master),
        ))
    } else {
        let media = parse_media(text)?;
        Ok((
            version_string(media.version),
            Playlist::MediaPlaylist(media),
        ))
    }
}

fn version_string(version: Option<usize>) -> &'static str {
    // The downloader only ever discards the version, so a compact
    // representation is fine.
    match version {
        Some(_) => "hls",
        None => "",
    }
}

struct Line {
    /// Everything after the leading `#`.
    content: String,
    /// 1-based line number for error reporting.
    number: usize,
}

fn split_lines(text: &str) -> Result<Vec<Line>, PlaylistError> {
    let mut lines = Vec::new();
    for (index, raw) in text.split('\n').enumerate() {
        let line_number = index + 1;
        if raw.len() > MAX_LINE_BYTES {
            return Err(playlist_error(format!(
                "playlist line {line_number} exceeds the {} byte limit",
                MAX_LINE_BYTES
            )));
        }
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            lines.push(Line {
                content: line.strip_prefix('#').unwrap_or(line).to_string(),
                number: line_number,
            });
        } else {
            lines.push(Line {
                content: line.to_string(),
                number: line_number,
            });
        }
    }
    Ok(lines)
}

fn parse_master(text: &str) -> Result<MasterPlaylist, PlaylistError> {
    let lines = split_lines(text)?;
    let mut playlist = MasterPlaylist::default();
    let mut pending_variant: Option<VariantStream> = None;

    for line in lines {
        if let Some(rest) = line.content.strip_prefix("EXT-X-STREAM-INF:") {
            if pending_variant.is_some() {
                return Err(playlist_error(format!(
                    "line {}: EXT-X-STREAM-INF without a URI line",
                    line.number
                )));
            }
            let attrs = parse_attributes(rest)?;
            pending_variant = Some(VariantStream {
                is_i_frame: false,
                uri: String::new(),
                bandwidth: parse_required_u64(&attrs, "BANDWIDTH", line.number)?,
                average_bandwidth: parse_optional_u64(&attrs, "AVERAGE-BANDWIDTH", line.number)?,
                codecs: parse_optional_quoted(&attrs, "CODECS"),
                resolution: parse_optional_resolution(&attrs, line.number)?,
                frame_rate: parse_optional_f64(&attrs, "FRAME-RATE", line.number)?,
                audio: parse_optional_quoted(&attrs, "AUDIO"),
                video: parse_optional_quoted(&attrs, "VIDEO"),
                subtitles: parse_optional_quoted(&attrs, "SUBTITLES"),
            });
            continue;
        }
        if line.content.starts_with("EXT-X-I-FRAME-STREAM-INF:") {
            let rest = &line.content["EXT-X-I-FRAME-STREAM-INF:".len()..];
            let attrs = parse_attributes(rest)?;
            let uri = parse_required_quoted(&attrs, "URI", line.number)?.to_string();
            playlist.variants.push(VariantStream {
                is_i_frame: true,
                uri,
                bandwidth: parse_required_u64(&attrs, "BANDWIDTH", line.number)?,
                average_bandwidth: parse_optional_u64(&attrs, "AVERAGE-BANDWIDTH", line.number)?,
                codecs: parse_optional_quoted(&attrs, "CODECS"),
                resolution: parse_optional_resolution(&attrs, line.number)?,
                frame_rate: parse_optional_f64(&attrs, "FRAME-RATE", line.number)?,
                audio: parse_optional_quoted(&attrs, "AUDIO"),
                video: parse_optional_quoted(&attrs, "VIDEO"),
                subtitles: parse_optional_quoted(&attrs, "SUBTITLES"),
            });
            continue;
        }
        if let Some(rest) = line.content.strip_prefix("EXT-X-MEDIA:") {
            let attrs = parse_attributes(rest)?;
            playlist
                .alternatives
                .push(parse_alternative_media(&attrs, line.number)?);
            continue;
        }
        if line.content == "EXT-X-INDEPENDENT-SEGMENTS" {
            playlist.independent_segments = true;
            continue;
        }
        if let Some(rest) = line.content.strip_prefix("EXT-X-VERSION:") {
            playlist.version = Some(parse_usize(rest, line.number)?);
            continue;
        }
        // The file header is a tag but not an `EXT-X-...` directive.
        if line.content == "EXTM3U" {
            continue;
        }
        // A bare URI line closes a pending variant.
        if !line.content.starts_with("EXT-") {
            if let Some(mut variant) = pending_variant.take() {
                variant.uri = line.content.clone();
                playlist.variants.push(variant);
            }
            continue;
        }
        // Unknown tag: skip.
    }

    if pending_variant.is_some() {
        return Err(playlist_error(
            "master playlist ended with an EXT-X-STREAM-INF without a URI line",
        ));
    }
    Ok(playlist)
}

fn parse_media(text: &str) -> Result<MediaPlaylist, PlaylistError> {
    let lines = split_lines(text)?;
    let mut playlist = MediaPlaylist::default();
    // Current in-progress segment; pushed onto `segments` when its URI line
    // arrives (or at end of input).
    let mut pending_segment: Option<MediaSegment> = None;
    let mut current_key: Option<Key> = None;
    let mut current_map: Option<Map> = None;
    // Tags that legally appear between a segment URI line and the next
    // EXTINF (RFC 8216 §4.3.2): BYTERANGE and DISCONTINUITY describe the
    // *next* segment, so they are buffered until that segment opens.
    let mut pending_byte_range: Option<ByteRange> = None;
    let mut pending_discontinuity = false;

    for line in lines {
        if let Some(rest) = line.content.strip_prefix("EXTINF:") {
            let (duration, title) = parse_extinf(rest, line.number)?;
            if pending_segment.is_some() {
                return Err(playlist_error(format!(
                    "line {}: EXTINF without a URI line",
                    line.number
                )));
            }
            pending_segment = Some(MediaSegment {
                uri: String::new(),
                duration,
                title,
                byte_range: pending_byte_range.take(),
                discontinuity: std::mem::take(&mut pending_discontinuity),
                key: current_key.clone(),
                map: current_map.clone(),
            });
            continue;
        }
        if let Some(rest) = line.content.strip_prefix("EXT-X-BYTERANGE:") {
            let byte_range = parse_byte_range(rest, line.number)?;
            match pending_segment.as_mut() {
                Some(segment) => segment.byte_range = Some(byte_range),
                None => pending_byte_range = Some(byte_range),
            }
            continue;
        }
        if line.content == "EXT-X-DISCONTINUITY" {
            match pending_segment.as_mut() {
                Some(segment) => segment.discontinuity = true,
                None => pending_discontinuity = true,
            }
            continue;
        }
        if let Some(rest) = line.content.strip_prefix("EXT-X-KEY:") {
            let attrs = parse_attributes(rest)?;
            current_key = Some(parse_key(&attrs, line.number)?);
            if let Some(segment) = pending_segment.as_mut() {
                segment.key = current_key.clone();
            }
            continue;
        }
        if let Some(rest) = line.content.strip_prefix("EXT-X-MAP:") {
            let attrs = parse_attributes(rest)?;
            current_map = Some(parse_map(&attrs, line.number)?);
            if let Some(segment) = pending_segment.as_mut() {
                segment.map = current_map.clone();
            }
            continue;
        }
        if line.content == "EXT-X-ENDLIST" {
            playlist.end_list = true;
            continue;
        }
        if line.content == "EXT-X-I-FRAMES-ONLY" {
            playlist.i_frames_only = true;
            continue;
        }
        if line.content == "EXT-X-INDEPENDENT-SEGMENTS" {
            playlist.independent_segments = true;
            continue;
        }
        if let Some(rest) = line.content.strip_prefix("EXT-X-MEDIA-SEQUENCE:") {
            playlist.media_sequence = parse_u64(rest, line.number)?;
            continue;
        }
        if let Some(rest) = line.content.strip_prefix("EXT-X-TARGETDURATION:") {
            playlist.target_duration = parse_u64(rest, line.number)?;
            continue;
        }
        if let Some(rest) = line.content.strip_prefix("EXT-X-VERSION:") {
            playlist.version = Some(parse_usize(rest, line.number)?);
            continue;
        }
        if let Some(rest) = line.content.strip_prefix("EXT-X-PLAYLIST-TYPE:") {
            playlist.playlist_type = Some(match rest.trim() {
                "EVENT" => MediaPlaylistType::Event,
                "VOD" => MediaPlaylistType::Vod,
                other => MediaPlaylistType::Other(other.to_string()),
            });
            continue;
        }
        // The file header is a tag but not an `EXT-X-...` directive.
        if line.content == "EXTM3U" {
            continue;
        }
        // A bare URI line closes the pending segment.
        if !line.content.starts_with("EXT-") {
            let mut segment = pending_segment.take().ok_or_else(|| {
                playlist_error(format!(
                    "line {}: URI line without a preceding EXTINF",
                    line.number
                ))
            })?;
            segment.uri = line.content.clone();
            if playlist.segments.len() >= MAX_SEGMENTS {
                return Err(playlist_error(format!(
                    "playlist exceeds the {} segment limit",
                    MAX_SEGMENTS
                )));
            }
            playlist.segments.push(segment);
            continue;
        }
        // Unknown tag: skip.
    }

    if pending_segment.is_some() {
        return Err(playlist_error(
            "media playlist ended with an EXTINF without a URI line",
        ));
    }
    Ok(playlist)
}

fn parse_extinf(rest: &str, line: usize) -> Result<(f32, Option<String>), PlaylistError> {
    let (duration_text, title) = match rest.split_once(',') {
        Some((duration, title)) => (duration.trim(), Some(title.to_string())),
        None => (rest.trim(), None),
    };
    let duration = duration_text.parse::<f32>().map_err(|_| {
        playlist_error(format!(
            "line {line}: invalid EXTINF duration {duration_text:?}"
        ))
    })?;
    if !duration.is_finite() || duration < 0.0 {
        return Err(playlist_error(format!(
            "line {line}: EXTINF duration must be finite and non-negative"
        )));
    }
    Ok((duration, title))
}

fn parse_byte_range(rest: &str, line: usize) -> Result<ByteRange, PlaylistError> {
    let (length_text, offset_text) = match rest.split_once('@') {
        Some((length, offset)) => (length.trim(), Some(offset.trim())),
        None => (rest.trim(), None),
    };
    let length = length_text.parse::<u64>().map_err(|_| {
        playlist_error(format!(
            "line {line}: invalid byte range length {length_text:?}"
        ))
    })?;
    let offset = match offset_text {
        Some(offset) if !offset.is_empty() => Some(offset.parse::<u64>().map_err(|_| {
            playlist_error(format!("line {line}: invalid byte range offset {offset:?}"))
        })?),
        _ => None,
    };
    Ok(ByteRange { length, offset })
}

fn parse_key(attrs: &BTreeMap<String, String>, line: usize) -> Result<Key, PlaylistError> {
    let method = match attrs.get("METHOD").map(String::as_str) {
        Some("NONE") => KeyMethod::None,
        Some("AES-128") => KeyMethod::AES128,
        Some("SAMPLE-AES") => KeyMethod::SampleAES,
        Some(other) => KeyMethod::Other(other.to_string()),
        None => {
            return Err(playlist_error(format!(
                "line {line}: EXT-X-KEY missing METHOD"
            )));
        }
    };
    let key = Key {
        method,
        uri: attrs.get("URI").cloned(),
        iv: attrs.get("IV").cloned(),
        keyformat: attrs.get("KEYFORMAT").cloned(),
    };
    if key.method != KeyMethod::None && key.iv.is_none() {
        // IV is technically optional (defaults to media sequence), so this
        // is not an error; the caller decides.
    }
    Ok(key)
}

fn parse_map(attrs: &BTreeMap<String, String>, line: usize) -> Result<Map, PlaylistError> {
    let uri = attrs
        .get("URI")
        .cloned()
        .ok_or_else(|| playlist_error(format!("line {line}: EXT-X-MAP missing URI")))?;
    let byte_range = match attrs.get("BYTERANGE") {
        Some(raw) => Some(parse_byte_range(raw, line)?),
        None => None,
    };
    Ok(Map { uri, byte_range })
}

fn parse_alternative_media(
    attrs: &BTreeMap<String, String>,
    line: usize,
) -> Result<AlternativeMedia, PlaylistError> {
    let media_type = match attrs.get("TYPE").map(String::as_str) {
        Some("AUDIO") => AlternativeMediaType::Audio,
        Some("VIDEO") => AlternativeMediaType::Video,
        Some("SUBTITLES") => AlternativeMediaType::Subtitles,
        Some("CLOSED-CAPTIONS") => AlternativeMediaType::ClosedCaptions,
        Some(other) => AlternativeMediaType::Other(other.to_string()),
        None => {
            return Err(playlist_error(format!(
                "line {line}: EXT-X-MEDIA missing TYPE"
            )));
        }
    };
    let group_id = attrs
        .get("GROUP-ID")
        .cloned()
        .ok_or_else(|| playlist_error(format!("line {line}: EXT-X-MEDIA missing GROUP-ID")))?;
    let name = attrs
        .get("NAME")
        .cloned()
        .ok_or_else(|| playlist_error(format!("line {line}: EXT-X-MEDIA missing NAME")))?;
    Ok(AlternativeMedia {
        media_type,
        uri: attrs.get("URI").cloned(),
        group_id,
        language: attrs.get("LANGUAGE").cloned(),
        name,
        default: is_yes(attrs.get("DEFAULT").map(String::as_str)),
        autoselect: is_yes(attrs.get("AUTOSELECT").map(String::as_str)),
    })
}

fn is_yes(value: Option<&str>) -> bool {
    matches!(value, Some("YES"))
}

fn parse_attributes(rest: &str) -> Result<BTreeMap<String, String>, PlaylistError> {
    let mut attrs = BTreeMap::new();
    let bytes = rest.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        // Skip whitespace and commas.
        while index < bytes.len() && (bytes[index] == b',' || bytes[index].is_ascii_whitespace()) {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        // Attribute name.
        let name_start = index;
        while index < bytes.len() && bytes[index] != b'=' && bytes[index] != b',' {
            index += 1;
        }
        let name = &rest[name_start..index];
        if name.is_empty() {
            return Err(playlist_error("empty attribute name in tag"));
        }
        // Expect `=`.
        if index >= bytes.len() || bytes[index] != b'=' {
            return Err(playlist_error(format!("attribute {name:?} is missing '='")));
        }
        index += 1;
        // Skip whitespace after `=`.
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let value = if index < bytes.len() && bytes[index] == b'"' {
            // Quoted value.
            index += 1;
            let value_start = index;
            while index < bytes.len() && bytes[index] != b'"' {
                index += 1;
            }
            if index >= bytes.len() {
                return Err(playlist_error(format!(
                    "attribute {name:?} has an unterminated quoted value"
                )));
            }
            let value = &rest[value_start..index];
            index += 1; // closing quote
            value.to_string()
        } else {
            // Unquoted value (until comma).
            let value_start = index;
            while index < bytes.len() && bytes[index] != b',' {
                index += 1;
            }
            let value = &rest[value_start..index];
            value.trim().to_string()
        };
        if attrs.insert(name.to_string(), value).is_some() {
            return Err(playlist_error(format!(
                "duplicate attribute {name:?} in tag"
            )));
        }
        // Consume the trailing comma (loop start also tolerates it).
    }
    Ok(attrs)
}

fn parse_required_u64(
    attrs: &BTreeMap<String, String>,
    name: &str,
    line: usize,
) -> Result<u64, PlaylistError> {
    attrs
        .get(name)
        .ok_or_else(|| playlist_error(format!("line {line}: missing {name} attribute")))
        .and_then(|value| {
            value
                .parse::<u64>()
                .map_err(|_| playlist_error(format!("line {line}: invalid {name} value {value:?}")))
        })
}

fn parse_optional_u64(
    attrs: &BTreeMap<String, String>,
    name: &str,
    line: usize,
) -> Result<Option<u64>, PlaylistError> {
    match attrs.get(name) {
        Some(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| playlist_error(format!("line {line}: invalid {name} value {value:?}"))),
        None => Ok(None),
    }
}

fn parse_optional_f64(
    attrs: &BTreeMap<String, String>,
    name: &str,
    line: usize,
) -> Result<Option<f64>, PlaylistError> {
    match attrs.get(name) {
        Some(value) => {
            let parsed = value.parse::<f64>().map_err(|_| {
                playlist_error(format!("line {line}: invalid {name} value {value:?}"))
            })?;
            if !parsed.is_finite() || parsed < 0.0 {
                return Err(playlist_error(format!(
                    "line {line}: {name} must be finite and non-negative"
                )));
            }
            Ok(Some(parsed))
        }
        None => Ok(None),
    }
}

fn parse_optional_resolution(
    attrs: &BTreeMap<String, String>,
    line: usize,
) -> Result<Option<Resolution>, PlaylistError> {
    match attrs.get("RESOLUTION") {
        Some(value) => {
            let (width, height) = value.split_once('x').ok_or_else(|| {
                playlist_error(format!("line {line}: invalid RESOLUTION value {value:?}"))
            })?;
            let width = width
                .trim()
                .parse::<u64>()
                .map_err(|_| playlist_error(format!("line {line}: invalid RESOLUTION width")))?;
            let height = height
                .trim()
                .parse::<u64>()
                .map_err(|_| playlist_error(format!("line {line}: invalid RESOLUTION height")))?;
            Ok(Some(Resolution { width, height }))
        }
        None => Ok(None),
    }
}

fn parse_required_quoted<'a>(
    attrs: &'a BTreeMap<String, String>,
    name: &str,
    line: usize,
) -> Result<&'a String, PlaylistError> {
    attrs
        .get(name)
        .ok_or_else(|| playlist_error(format!("line {line}: missing {name} attribute")))
}

fn parse_optional_quoted(attrs: &BTreeMap<String, String>, name: &str) -> Option<String> {
    attrs.get(name).cloned()
}

fn parse_u64(rest: &str, line: usize) -> Result<u64, PlaylistError> {
    let text = rest.trim();
    text.parse::<u64>()
        .map_err(|_| playlist_error(format!("line {line}: invalid integer {text:?}")))
}

fn parse_usize(rest: &str, line: usize) -> Result<usize, PlaylistError> {
    let text = rest.trim();
    text.parse::<usize>()
        .map_err(|_| playlist_error(format!("line {line}: invalid integer {text:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_master_playlist() {
        let source = br#"#EXTM3U
#EXT-X-VERSION:7
#EXT-X-INDEPENDENT-SEGMENTS
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="main-audio",NAME="English",AUTOSELECT=YES,LANGUAGE="en",URI="en.m3u8"
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="main-audio",NAME="Original",DEFAULT=YES,AUTOSELECT=YES,LANGUAGE="ja",URI="original.m3u8"
#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=9000000,RESOLUTION=3840x2160,URI="iframe.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=854x480,AUDIO="main-audio"
480p.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1920x1080,AUDIO="main-audio"
1080p.m3u8
"#;
        let (_, playlist) = parse_playlist(source).expect("master playlist should parse");
        let Playlist::MasterPlaylist(master) = playlist else {
            panic!("expected a master playlist");
        };
        assert_eq!(master.variants.len(), 3);
        assert!(master.variants[0].is_i_frame);
        assert_eq!(master.variants[1].uri, "480p.m3u8");
        assert_eq!(master.variants[1].bandwidth, 800000);
        assert_eq!(
            master.variants[2].resolution,
            Some(Resolution {
                width: 1920,
                height: 1080
            })
        );
        assert_eq!(master.variants[2].audio.as_deref(), Some("main-audio"));
        assert_eq!(master.alternatives.len(), 2);
        let ja = master
            .alternatives
            .iter()
            .find(|a| a.language.as_deref() == Some("ja"));
        assert!(ja.is_some());
        assert!(ja.unwrap().default);
        assert!(master.independent_segments);
    }

    #[test]
    fn parses_media_playlist_with_encryption() {
        let source = br#"#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:10
#EXT-X-MEDIA-SEQUENCE:42
#EXT-X-KEY:METHOD=AES-128,URI="https://cdn.example/key",IV=0x00000000000000000000000000000000
#EXTINF:9.009,title
seg1.ts
#EXT-X-BYTERANGE:1000@500
#EXTINF:8.5
seg2.ts
#EXT-X-ENDLIST
"#;
        let (_, playlist) = parse_playlist(source).expect("media playlist should parse");
        let Playlist::MediaPlaylist(media) = playlist else {
            panic!("expected a media playlist");
        };
        assert_eq!(media.media_sequence, 42);
        assert_eq!(media.target_duration, 10);
        assert_eq!(media.segments.len(), 2);
        assert!(media.end_list);
        assert_eq!(media.segments[0].duration, 9.009);
        assert_eq!(media.segments[0].title.as_deref(), Some("title"));
        assert_eq!(media.segments[0].uri, "seg1.ts");
        let key = media.segments[0].key.as_ref().expect("key present");
        assert_eq!(key.method, KeyMethod::AES128);
        assert_eq!(key.uri.as_deref(), Some("https://cdn.example/key"));
        assert_eq!(
            media.segments[1].byte_range,
            Some(ByteRange {
                length: 1000,
                offset: Some(500)
            })
        );
    }

    #[test]
    fn rejects_unterminated_segment() {
        let source = b"#EXTM3U\n#EXTINF:5.0\n";
        assert!(parse_playlist(source).is_err());
    }

    #[test]
    fn rejects_overflow_input() {
        let mut source = vec![b'#'; MAX_INPUT_BYTES + 1];
        source.extend_from_slice(b"EXTM3U\n");
        assert!(parse_playlist(&source).is_err());
    }

    #[test]
    fn rejects_non_utf8() {
        assert!(parse_playlist(&[0xff, 0xfe, 0x00, 0x01]).is_err());
    }
}
