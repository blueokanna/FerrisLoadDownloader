#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

//! Platform-independent FerrisLoad types, validation, and wire encoding.
//!
//! Network, filesystem, Flutter FFI, and hardware codec implementations belong
//! to platform adapters. This crate intentionally stays usable without `std`.

use core::fmt;
use serde::{Deserialize, Serialize};

/// Current stable version of the download-plan wire schema.
pub const DOWNLOAD_PLAN_VERSION: u16 = 1;
/// Hard upper bound used to prevent accidental unbounded task fan-out.
pub const MAX_CONCURRENCY: u16 = 256;
/// Hard upper bound used to prevent pathological retry loops.
pub const MAX_RETRIES: u8 = 32;
/// Maximum accepted bitrate in kbit/s that remains safe when converted to bps.
pub const MAX_BITRATE_KBPS: u32 = i32::MAX as u32 / 1000;
/// Maximum encoded download-plan size accepted at trust boundaries.
pub const MAX_WIRE_SIZE: u64 = 64 * 1024;
/// Maximum collection size accepted by the RustBinary decoder.
pub const MAX_WIRE_COLLECTIONS: u64 = 128;

/// A borrowed, allocation-free download plan shared by all runtime adapters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DownloadPlan<'a> {
    /// Wire schema version.
    pub version: u16,
    /// User-facing page URL used for origin and extractor context.
    pub page_url: &'a str,
    /// Primary downloadable media URL.
    pub media_url: &'a str,
    /// Optional separate audio stream URL.
    pub audio_url: Option<&'a str>,
    /// Destination path selected by the platform adapter.
    pub output_path: &'a str,
    /// Maximum number of concurrent segment downloads.
    pub concurrency: u16,
    /// Number of retry attempts after the initial request.
    pub retries: u8,
    /// Requested video bitrate in kbit/s, or zero for stream copy/default.
    pub video_bitrate_kbps: u32,
    /// Requested audio bitrate in kbit/s, or zero for stream copy/default.
    pub audio_bitrate_kbps: u32,
    /// Preserve intermediate files after completion.
    pub keep_temporary_files: bool,
}

impl DownloadPlan<'_> {
    /// Validates bounded values and fields required by every platform adapter.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.version != DOWNLOAD_PLAN_VERSION {
            return Err(ValidationError::UnsupportedVersion(self.version));
        }
        if self.page_url.trim().is_empty() {
            return Err(ValidationError::EmptyField("page_url"));
        }
        if self.media_url.trim().is_empty() {
            return Err(ValidationError::EmptyField("media_url"));
        }
        if self.output_path.trim().is_empty() {
            return Err(ValidationError::EmptyField("output_path"));
        }
        if !(1..=MAX_CONCURRENCY).contains(&self.concurrency) {
            return Err(ValidationError::ConcurrencyOutOfRange(self.concurrency));
        }
        if self.retries > MAX_RETRIES {
            return Err(ValidationError::RetriesOutOfRange(self.retries));
        }
        if self.video_bitrate_kbps > MAX_BITRATE_KBPS {
            return Err(ValidationError::BitrateOutOfRange {
                field: "video_bitrate_kbps",
                value: self.video_bitrate_kbps,
            });
        }
        if self.audio_bitrate_kbps > MAX_BITRATE_KBPS {
            return Err(ValidationError::BitrateOutOfRange {
                field: "audio_bitrate_kbps",
                value: self.audio_bitrate_kbps,
            });
        }
        Ok(())
    }
}

/// Validation failures that are stable across FFI and platform boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationError {
    /// The payload uses an unknown schema version.
    UnsupportedVersion(u16),
    /// A required string field is empty.
    EmptyField(&'static str),
    /// Segment concurrency is outside the supported range.
    ConcurrencyOutOfRange(u16),
    /// Retry count is outside the supported range.
    RetriesOutOfRange(u8),
    /// A bitrate exceeds the defensive upper bound.
    BitrateOutOfRange {
        /// Name of the invalid bitrate field.
        field: &'static str,
        /// Invalid value in kbit/s.
        value: u32,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported download plan version {version}")
            }
            Self::EmptyField(field) => write!(formatter, "{field} must not be empty"),
            Self::ConcurrencyOutOfRange(value) => write!(
                formatter,
                "concurrency {value} is outside 1..={MAX_CONCURRENCY}"
            ),
            Self::RetriesOutOfRange(value) => {
                write!(formatter, "retries {value} exceeds {MAX_RETRIES}")
            }
            Self::BitrateOutOfRange { field, value } => write!(
                formatter,
                "{field} value {value} exceeds {MAX_BITRATE_KBPS} kbit/s"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ValidationError {}

/// Failures produced while validating or decoding the stable wire format.
#[derive(Debug)]
pub enum WireError {
    /// The RustBinary payload is malformed or violates a resource limit.
    Codec(rustbinary::core::Error),
    /// The decoded plan violates a FerrisLoad invariant.
    Validation(ValidationError),
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codec(error) => write!(formatter, "invalid RustBinary frame: {error}"),
            Self::Validation(error) => error.fmt(formatter),
        }
    }
}

impl From<rustbinary::core::Error> for WireError {
    fn from(error: rustbinary::core::Error) -> Self {
        Self::Codec(error)
    }
}

impl From<ValidationError> for WireError {
    fn from(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for WireError {}

fn wire_options() -> rustbinary::core::Config {
    rustbinary::core::options()
        .with_limit(MAX_WIRE_SIZE)
        .with_collection_limit(MAX_WIRE_COLLECTIONS)
        .reject_trailing_bytes()
}

/// Returns the exact caller-buffer size required for a validated plan.
pub fn encoded_download_plan_size(plan: &DownloadPlan<'_>) -> Result<usize, WireError> {
    plan.validate()?;
    let size = wire_options().serialized_size(plan)?;
    usize::try_from(size)
        .map_err(|_| WireError::Codec(rustbinary::core::Error::IntegerOverflow { target: "usize" }))
}

/// Encodes a validated plan into a caller-owned buffer using RustBinary Core.
pub fn encode_download_plan(
    output: &mut [u8],
    plan: &DownloadPlan<'_>,
) -> Result<usize, WireError> {
    plan.validate()?;
    wire_options()
        .serialize_into_slice(output, plan)
        .map_err(Into::into)
}

/// Decodes and validates a borrowed plan without allocating.
pub fn decode_download_plan(input: &[u8]) -> Result<DownloadPlan<'_>, WireError> {
    let plan = wire_options().deserialize::<DownloadPlan<'_>>(input)?;
    plan.validate()?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan<'a>() -> DownloadPlan<'a> {
        DownloadPlan {
            version: DOWNLOAD_PLAN_VERSION,
            page_url: "https://example.com/watch/1",
            media_url: "https://cdn.example.com/master.m3u8",
            audio_url: None,
            output_path: "video.mp4",
            concurrency: 8,
            retries: 3,
            video_bitrate_kbps: 0,
            audio_bitrate_kbps: 0,
            keep_temporary_files: false,
        }
    }

    #[test]
    fn rustbinary_round_trip_is_borrowed_and_exact() {
        let source = plan();
        let required = encoded_download_plan_size(&source).unwrap();
        let mut frame = [0_u8; 512];
        let written = encode_download_plan(&mut frame, &source).unwrap();
        assert_eq!(written, required);

        let decoded = decode_download_plan(&frame[..written]).unwrap();
        assert_eq!(decoded, source);
        let start = frame.as_ptr() as usize;
        let end = start + written;
        let media = decoded.media_url.as_ptr() as usize;
        assert!((start..end).contains(&media));
    }

    #[test]
    fn validation_rejects_unbounded_runtime_values() {
        let mut invalid = plan();
        invalid.concurrency = 0;
        assert_eq!(
            invalid.validate(),
            Err(ValidationError::ConcurrencyOutOfRange(0))
        );

        invalid = plan();
        invalid.video_bitrate_kbps = MAX_BITRATE_KBPS + 1;
        assert_eq!(
            invalid.validate(),
            Err(ValidationError::BitrateOutOfRange {
                field: "video_bitrate_kbps",
                value: MAX_BITRATE_KBPS + 1,
            })
        );
    }

    #[test]
    fn decoder_rejects_trailing_bytes() {
        let source = plan();
        let mut frame = [0_u8; 512];
        let written = encode_download_plan(&mut frame, &source).unwrap();
        frame[written] = 0xff;
        assert!(matches!(
            decode_download_plan(&frame[..=written]),
            Err(WireError::Codec(rustbinary::core::Error::TrailingBytes {
                remaining: 1
            }))
        ));
    }
}
