//! MPEG-TS demuxer for the native HLS → MP4 stream-copy remuxer.
//!
//! Supports what real HLS feeds use in practice: one H.264 (stream type
//! 0x1B) video elementary stream, optionally one AAC-in-ADTS (stream type
//! 0x0F) audio stream. Unlike a naive "one PES = one access unit" splitter it
//! rebuilds **picture boundaries** (access unit delimiters or slices with
//! `first_mb_in_slice == 0`) and **display order** (H.264 picture order
//! count), so B-frame streams are remuxed with a correct `ctts` table exactly
//! like `ffmpeg -c copy`. The video timeline is continuous across HLS segment
//! PTS resets because frames are counted, not timed.
//!
//! Anything outside the supported subset (HEVC, interlaced fields, H.264
//! scaling lists / pic_order_cnt_type 1, non-4:2:0 chroma, …) makes the
//! parser return an error so the caller falls back to the platform
//! transcoder. It never guesses.

use std::io::{ErrorKind, Read};

use anyhow::{Context, Result, bail};
use log::{debug, info, warn};

use super::bits::BitReader;

const TS_PACKET_SIZE: usize = 188;
const VIDEO_BUFFER_LIMIT: usize = 32 * 1024 * 1024;
/// Upper bound on pictures held for display-order reordering (a few seconds).
const GOP_PICTURE_LIMIT: usize = 600;

/// H.264 decoder configuration extracted from SPS/PPS.
#[derive(Debug, Clone)]
pub struct AvcConfig {
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    pub profile: u8,
    pub compat: u8,
    pub level: u8,
    pub width: u32,
    pub height: u32,
    /// Frame interval in 90 kHz ticks signalled in the SPS VUI.
    pub frame_interval_ticks: Option<u32>,
}

/// AAC decoder configuration extracted from the first ADTS frame.
#[derive(Debug, Clone)]
pub struct AacConfig {
    pub asc: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Receives the demuxed elementary stream.
pub trait SampleSink {
    fn set_video_config(&mut self, config: AvcConfig) -> Result<()>;
    fn set_audio_config(&mut self, config: AacConfig) -> Result<()>;
    fn video_sample(&mut self, data: &[u8], dts: i64, cts_offset: i32, key: bool) -> Result<()>;
    fn audio_sample(&mut self, data: &[u8]) -> Result<()>;
}

/// True when `head` looks like a 188-byte MPEG-TS stream.
pub fn is_mpeg_ts(head: &[u8]) -> bool {
    if head.len() < TS_PACKET_SIZE + 1 {
        return head.first() == Some(&0x47);
    }
    [0usize, TS_PACKET_SIZE, TS_PACKET_SIZE * 2]
        .iter()
        .all(|offset| head.get(*offset) == Some(&0x47))
}

/// Demux `reader` (a merged HLS TS) and feed `sink`.
pub fn parse_ts<R: Read>(reader: R, sink: &mut dyn SampleSink) -> Result<()> {
    let mut parser = TsParser::new(sink);
    let mut buffer = [0u8; TS_PACKET_SIZE];
    let mut reader = reader;
    loop {
        match reader.read(&mut buffer[..1]) {
            Ok(0) => break,
            Ok(1) => {}
            Ok(_) => unreachable!(),
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("reading MPEG-TS input"),
        }
        reader
            .read_exact(&mut buffer[1..])
            .context("reading MPEG-TS input")?;
        parser.packet(&buffer)?;
    }
    parser.finish()
}

/// H.264 parameter sets and slice-header fields needed to rebuild the access
/// unit structure and the display order.
#[derive(Debug, Clone, Default)]
struct H264Params {
    log2_max_frame_num: u32,
    poc_type: u32,
    log2_max_poc_lsb: u32,
    separate_colour_plane: bool,
    bottom_field_poc_present: bool,
    prev_poc_lsb: i64,
    poc_wrap: i64,
}
struct SliceInfo {
    first_mb: u32,
    poc: Option<i64>,
    is_idr: bool,
}

struct PendingPicture {
    nals: Vec<Vec<u8>>,
    poc: i64,
    key: bool,
}

struct TsParser<'s> {
    sink: &'s mut dyn SampleSink,
    pmt_pid: Option<u16>,
    video_pid: Option<u16>,
    audio_pid: Option<u16>,
    unsupported_video_type: Option<u8>,
    unsupported_audio_type: Option<u8>,
    last_cc: std::collections::HashMap<u16, u8>,

    pat_buffer: Vec<u8>,
    pmt_buffer: Vec<u8>,

    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    params: H264Params,
    video_config_sent: bool,

    video_buffer: Vec<u8>,
    video_prefix: Vec<Vec<u8>>,
    video_picture: Vec<Vec<u8>>,
    video_has_picture: bool,
    video_picture_poc: i64,
    video_picture_fallback_poc: i64,
    video_picture_key: bool,
    gop: Vec<PendingPicture>,
    video_flushed: i64,
    video_pictures_seen: i64,
    video_pes_anchor: Option<(i64, i64)>,
    video_frame_interval: i64,
    video_interval_from_vui: bool,
    video_interval_estimated: bool,

    audio_buffer: Vec<u8>,
    audio_next_pts: Option<i64>,
    audio_tb: Timebase,
    audio_frame_ticks: Option<i64>,
    audio_config_sent: bool,

    video_samples: u64,
    audio_frames: u64,
}

/// 33-bit PTS/DTS re-baser used for the audio track.
struct Timebase {
    offset: i64,
    last: Option<i64>,
}

impl Timebase {
    fn new() -> Self {
        Self {
            offset: 0,
            last: None,
        }
    }

    fn map(&mut self, ticks: i64) -> i64 {
        if let Some(last) = self.last
            && ticks + self.offset < last - 45_000
        {
            self.offset = last - ticks;
        }
        let mapped = ticks + self.offset;
        self.last = Some(match self.last {
            Some(last) => last.max(mapped),
            None => mapped,
        });
        mapped
    }
}

impl<'s> TsParser<'s> {
    fn new(sink: &'s mut dyn SampleSink) -> Self {
        Self {
            sink,
            pmt_pid: None,
            video_pid: None,
            audio_pid: None,
            unsupported_video_type: None,
            unsupported_audio_type: None,
            last_cc: std::collections::HashMap::new(),
            pat_buffer: Vec::new(),
            pmt_buffer: Vec::new(),
            sps: None,
            pps: None,
            params: H264Params {
                log2_max_frame_num: 4,
                poc_type: 2,
                log2_max_poc_lsb: 4,
                ..H264Params::default()
            },
            video_config_sent: false,
            video_buffer: Vec::new(),
            video_prefix: Vec::new(),
            video_picture: Vec::new(),
            video_has_picture: false,
            video_picture_poc: 0,
            video_picture_fallback_poc: 0,
            video_picture_key: false,
            gop: Vec::new(),
            video_flushed: 0,
            video_pictures_seen: 0,
            video_pes_anchor: None,
            video_frame_interval: 3000,
            video_interval_from_vui: false,
            video_interval_estimated: false,
            audio_buffer: Vec::new(),
            audio_next_pts: None,
            audio_tb: Timebase::new(),
            audio_frame_ticks: None,
            audio_config_sent: false,
            video_samples: 0,
            audio_frames: 0,
        }
    }

    fn packet(&mut self, packet: &[u8]) -> Result<()> {
        if packet.len() != TS_PACKET_SIZE || packet[0] != 0x47 {
            bail!("lost MPEG-TS sync");
        }
        if packet[1] & 0x80 != 0 {
            return Ok(()); // transport error indicator
        }
        let pid = ((u16::from(packet[1]) & 0x1F) << 8) | u16::from(packet[2]);
        let afc = (packet[3] >> 4) & 0x3;
        let has_payload = afc & 0x1 != 0;
        let has_adaptation = afc & 0x2 != 0;
        if !has_payload {
            return Ok(());
        }
        if packet[3] & 0x80 != 0 {
            bail!("scrambled MPEG-TS packets are not supported");
        }

        let mut offset = 4usize;
        if has_adaptation {
            let length = packet[offset] as usize;
            offset += 1 + length;
            if offset >= TS_PACKET_SIZE {
                return Ok(());
            }
        }

        let cc = packet[3] & 0x0F;
        if let Some(previous) = self.last_cc.insert(pid, cc) {
            if cc == previous {
                return Ok(()); // exact duplicate
            }
            if cc != (previous + 1) & 0x0F {
                debug!("TS continuity gap on PID {pid}: {previous} -> {cc}");
            }
        }

        let payload = &packet[offset..];
        let pusi = packet[1] & 0x40 != 0;

        if pid == 0 {
            return self.psi(payload, pusi, true);
        }
        if Some(pid) == self.pmt_pid {
            return self.psi(payload, pusi, false);
        }
        if Some(pid) == self.video_pid {
            return self.video_pes(payload, pusi);
        }
        if Some(pid) == self.audio_pid {
            return self.audio_pes(payload, pusi);
        }
        Ok(())
    }

    // ───────────────────────────── PSI ─────────────────────────────

    fn psi(&mut self, payload: &[u8], pusi: bool, is_pat: bool) -> Result<()> {
        let buffer = if is_pat {
            &mut self.pat_buffer
        } else {
            &mut self.pmt_buffer
        };
        if pusi {
            if payload.is_empty() {
                return Ok(());
            }
            let pointer = payload[0] as usize;
            let start = 1 + pointer;
            if start > payload.len() {
                return Ok(());
            }
            buffer.clear();
            buffer.extend_from_slice(&payload[start..]);
        } else {
            buffer.extend_from_slice(payload);
        }
        let taken = std::mem::take(buffer);
        let result = self.consume_sections(&taken, is_pat);
        let restore = if is_pat {
            &mut self.pat_buffer
        } else {
            &mut self.pmt_buffer
        };
        *restore = taken;
        result
    }

    fn consume_sections(&mut self, buffer: &[u8], is_pat: bool) -> Result<()> {
        if buffer.len() < 3 {
            return Ok(());
        }
        let expected_table = if is_pat { 0x00 } else { 0x02 };
        if buffer[0] != expected_table {
            return Ok(());
        }
        let section_length = ((usize::from(buffer[1]) & 0x0F) << 8) | usize::from(buffer[2]);
        let total = 3 + section_length;
        if buffer.len() < total {
            return Ok(());
        }
        if is_pat {
            self.parse_pat(&buffer[..total])
        } else {
            self.parse_pmt(&buffer[..total])
        }
    }

    fn parse_pat(&mut self, section: &[u8]) -> Result<()> {
        let mut index = 8usize;
        while index + 4 <= section.len().saturating_sub(4) {
            let program = (u16::from(section[index]) << 8) | u16::from(section[index + 1]);
            let pid = ((u16::from(section[index + 2]) & 0x1F) << 8) | u16::from(section[index + 3]);
            if program != 0 {
                if self.pmt_pid != Some(pid) {
                    info!("MPEG-TS: PMT PID = {pid}");
                    self.pmt_pid = Some(pid);
                }
                return Ok(());
            }
            index += 4;
        }
        Ok(())
    }

    fn parse_pmt(&mut self, section: &[u8]) -> Result<()> {
        if section.len() < 12 {
            return Ok(());
        }
        let program_info_length =
            ((usize::from(section[10]) & 0x0F) << 8) | usize::from(section[11]);
        let mut index = 12 + program_info_length;
        while index + 5 <= section.len().saturating_sub(4) {
            let stream_type = section[index];
            let pid = ((u16::from(section[index + 1]) & 0x1F) << 8) | u16::from(section[index + 2]);
            let info_length =
                ((usize::from(section[index + 3]) & 0x0F) << 8) | usize::from(section[index + 4]);
            index += 5 + info_length;
            match stream_type {
                0x1B if self.video_pid != Some(pid) => {
                    info!("MPEG-TS: AVC video PID = {pid}");
                    self.video_pid = Some(pid);
                }
                0x0F if self.audio_pid != Some(pid) => {
                    info!("MPEG-TS: AAC audio PID = {pid}");
                    self.audio_pid = Some(pid);
                }
                // Audio we cannot copy losslessly into an MP4: AC-3,
                // Enhanced AC-3, MPEG-1/2 audio, LATM AAC. Dropping them
                // silently would produce a mute video, so record the type and
                // let the caller fall back to the platform transcoder.
                0x81 | 0x87 | 0x03 | 0x04 | 0x11 => {
                    if self.unsupported_audio_type.is_none() {
                        warn!("MPEG-TS: unsupported audio stream type 0x{stream_type:02X}");
                    }
                    self.unsupported_audio_type = Some(stream_type);
                }
                0x06 => {
                    if self.unsupported_audio_type.is_none() {
                        warn!("MPEG-TS: unsupported private stream 0x06 (subtitles or AC-3)");
                    }
                    self.unsupported_audio_type = Some(stream_type);
                }
                0x24 | 0x27 => self.unsupported_video_type = Some(stream_type),
                _ => {}
            }
        }
        Ok(())
    }

    // ───────────────────────────── PES ─────────────────────────────

    fn pes_payload<'p>(&mut self, payload: &'p [u8]) -> Result<&'p [u8]> {
        if payload.len() < 9 || payload[0] != 0 || payload[1] != 0 || payload[2] != 1 {
            bail!("invalid PES header");
        }
        let header_data_length = payload[8] as usize;
        let start = 9 + header_data_length;
        if start > payload.len() {
            bail!("PES header extends past the packet");
        }
        Ok(&payload[start..])
    }

    fn video_pes(&mut self, payload: &[u8], pusi: bool) -> Result<()> {
        let (data, pts, dts) = if pusi {
            self.pes_info(payload)?
        } else {
            (payload, None, None)
        };
        let _ = pts;
        if let Some(dts) = dts {
            self.calibrate_video_interval(dts);
        }
        if data.is_empty() {
            return Ok(());
        }
        self.video_buffer.extend_from_slice(data);
        if self.video_buffer.len() > VIDEO_BUFFER_LIMIT {
            bail!("video access unit exceeds the size limit");
        }
        self.process_video_buffer()
    }

    /// Derive the frame interval from PES decode timestamps when the SPS VUI
    /// does not carry timing information (some muxers omit it). PTS/DTS are
    /// only used for this estimate; the output timeline itself is built from
    /// the frame count, which keeps it continuous across segment resets.
    fn calibrate_video_interval(&mut self, dts: i64) {
        if !self.video_interval_from_vui
            && let Some((previous_dts, previous_count)) = self.video_pes_anchor
        {
            let frames = self.video_pictures_seen - previous_count;
            if frames > 0 {
                let delta = dts - previous_dts;
                if delta > 0 {
                    let estimate = delta / frames;
                    if (300..=20_000).contains(&estimate) {
                        if self.video_interval_estimated {
                            self.video_frame_interval = (self.video_frame_interval + estimate) / 2;
                        } else {
                            self.video_frame_interval = estimate;
                            self.video_interval_estimated = true;
                        }
                    }
                }
            }
        }
        self.video_pes_anchor = Some((dts, self.video_pictures_seen));
    }

    /// Parse the fixed PES header, returning `(payload, pts, dts)`.
    fn pes_info<'p>(&mut self, payload: &'p [u8]) -> Result<(&'p [u8], Option<i64>, Option<i64>)> {
        if payload.len() < 9 || payload[0] != 0 || payload[1] != 0 || payload[2] != 1 {
            bail!("invalid PES header");
        }
        let stream_id = payload[3];
        let header_data_length = payload[8] as usize;
        let mut pts = None;
        let mut dts = None;
        if matches!(stream_id, 0xBD | 0xC0..=0xDF | 0xE0..=0xEF) {
            let flags = payload[7] >> 6;
            if flags & 0x2 != 0 && payload.len() >= 14 {
                pts = Some(read_pts(&payload[9..14])?);
                if flags & 0x1 != 0 && payload.len() >= 19 {
                    dts = Some(read_pts(&payload[14..19])?);
                }
            }
        }
        let start = 9 + header_data_length;
        if start > payload.len() {
            bail!("PES header extends past the packet");
        }
        Ok((&payload[start..], pts, dts))
    }

    /// Split every complete NAL unit out of `video_buffer`. The trailing NAL
    /// may still be incomplete (its terminating start code has not arrived
    /// yet); it stays buffered until the next PES packet supplies more bytes.
    fn process_video_buffer(&mut self) -> Result<()> {
        loop {
            let Some((code_start, _, nal_start)) = find_start_code(&self.video_buffer, 0) else {
                self.video_buffer.clear();
                return Ok(());
            };
            if code_start > 0 {
                self.video_buffer.drain(..code_start);
            }
            let nal_start = nal_start - code_start;
            match find_start_code(&self.video_buffer, nal_start) {
                Some((next_start, _, _)) => {
                    let nal = self.video_buffer[nal_start..next_start].to_vec();
                    self.video_buffer.drain(..next_start);
                    self.process_nal(nal)?;
                }
                None => return Ok(()),
            }
        }
    }

    fn process_nal(&mut self, nal: Vec<u8>) -> Result<()> {
        // The NAL is already bounded by start-code delimiters; preserve every payload byte.
        if nal.is_empty() {
            return Ok(());
        }
        match nal[0] & 0x1F {
            7 => {
                let mut params = parse_sps(&nal)?;
                // Keep the PPS-derived flag until the next PPS arrives.
                params.bottom_field_poc_present = self.params.bottom_field_poc_present;
                self.params = params;
                self.sps = Some(nal);
            }
            8 => {
                self.params.bottom_field_poc_present = parse_pps(&nal)?;
                self.pps = Some(nal);
            }
            9 => self.close_picture()?, // access unit delimiter
            6 => self.video_prefix.push(nal),
            1..=5 => {
                let info = parse_slice(&nal, &self.params)?;
                if info.first_mb == 0 {
                    self.close_picture()?;
                }
                if !self.video_has_picture {
                    self.open_picture()?;
                }
                if let Some(raw_poc) = info.poc {
                    self.video_picture_poc = resolve_poc(raw_poc, &mut self.params);
                }
                if info.is_idr {
                    self.video_picture_key = true;
                }
                self.video_picture.push(nal);
            }
            _ => {}
        }
        Ok(())
    }

    fn open_picture(&mut self) -> Result<()> {
        self.video_has_picture = true;
        self.video_picture.clear();
        self.video_picture_key = false;
        let prefix = std::mem::take(&mut self.video_prefix);
        self.video_picture.extend(prefix);
        // Fallback ordering: arrival order (correct for streams without
        // reordering and still monotonic for streams we cannot reorder).
        self.video_picture_fallback_poc = self.gop.len() as i64;
        self.video_picture_poc = self.video_picture_fallback_poc;
        Ok(())
    }

    fn close_picture(&mut self) -> Result<()> {
        if !self.video_has_picture {
            self.video_prefix.clear();
            return Ok(());
        }
        self.video_has_picture = false;
        let nals = std::mem::take(&mut self.video_picture);
        if nals.is_empty() {
            return Ok(());
        }
        let key = self.video_picture_key;
        // Display-order reordering happens per GOP (IDR-delimited).
        if key && !self.gop.is_empty() {
            self.flush_gop()?;
        }
        self.gop.push(PendingPicture {
            nals,
            poc: self.video_picture_poc,
            key,
        });
        self.video_pictures_seen += 1;
        if self.gop.len() >= GOP_PICTURE_LIMIT {
            self.flush_gop()?;
        }
        Ok(())
    }

    /// Emit the buffered GOP in display order: decode timestamps stay
    /// uniform, the presentation timestamps come from the picture order
    /// count, and a constant is added so every `ctts` offset is non-negative
    /// (exactly what an `ffmpeg -c copy` MP4 contains).
    fn flush_gop(&mut self) -> Result<()> {
        if self.gop.is_empty() {
            return Ok(());
        }
        // Resolve the decoder configuration (and the VUI frame interval)
        // BEFORE deriving any timestamp from it.
        if !self.video_config_sent {
            let (Some(sps), Some(pps)) = (self.sps.as_ref(), self.pps.as_ref()) else {
                bail!("H.264 SPS/PPS were not present before the first slice");
            };
            let config = parse_avc_config(sps, pps)?;
            if let Some(interval_ticks) = config.frame_interval_ticks
                && interval_ticks > 0
            {
                self.video_frame_interval = i64::from(interval_ticks);
                self.video_interval_from_vui = true;
            }
            info!(
                "MPEG-TS: AVC {}x{}, profile {}, level {}, frame interval {} ticks{}",
                config.width,
                config.height,
                config.profile,
                config.level,
                self.video_frame_interval,
                if self.video_interval_from_vui {
                    " (VUI)"
                } else if self.video_interval_estimated {
                    " (measured from PES timestamps)"
                } else {
                    " (default 30fps)"
                }
            );
            self.sink.set_video_config(config)?;
            self.video_config_sent = true;
        }
        let count = self.gop.len();
        let interval = self.video_frame_interval;
        let mut order: Vec<usize> = (0..count).collect();
        order.sort_by_key(|index| self.gop[*index].poc);
        let mut rank = vec![0usize; count];
        for (position, index) in order.iter().enumerate() {
            rank[*index] = position;
        }
        let max_lag = (0..count)
            .map(|index| index.saturating_sub(rank[index]))
            .max()
            .unwrap_or(0);

        for (index, picture) in self.gop.iter().enumerate() {
            let dts = (self.video_flushed + index as i64) * interval;
            let cts_frames = (rank[index] + max_lag) as i64 - index as i64;
            let cts = i32::try_from(cts_frames * interval).unwrap_or(i32::MAX);
            let mut sample = Vec::new();
            for nal in &picture.nals {
                if nal.len() < 2 {
                    continue;
                }
                let length = u32::try_from(nal.len()).context("NAL unit too large")?;
                sample.extend_from_slice(&length.to_be_bytes());
                sample.extend_from_slice(nal);
            }
            if sample.is_empty() {
                continue;
            }
            self.sink.video_sample(&sample, dts, cts, picture.key)?;
            self.video_samples += 1;
        }
        self.video_flushed += count as i64;
        self.gop.clear();
        Ok(())
    }

    fn audio_pes(&mut self, payload: &[u8], pusi: bool) -> Result<()> {
        if pusi {
            if payload.len() >= 9 && payload[0] == 0 && payload[1] == 0 && payload[2] == 1 {
                let flags = payload[7] >> 6;
                if flags & 0x2 != 0 && payload.len() >= 14 {
                    let pts = read_pts(&payload[9..14])?;
                    let mapped = self.audio_tb.map(pts);
                    match self.audio_next_pts {
                        None => self.audio_next_pts = Some(mapped),
                        Some(next) => {
                            if (mapped - next).abs() > 3 * 90_000 {
                                self.audio_next_pts = Some(mapped);
                            }
                        }
                    }
                }
                let data = self.pes_payload(payload)?;
                self.audio_buffer.extend_from_slice(data);
            } else {
                self.audio_buffer.extend_from_slice(payload);
            }
        } else {
            self.audio_buffer.extend_from_slice(payload);
        }
        self.drain_audio()
    }

    fn drain_audio(&mut self) -> Result<()> {
        loop {
            if self.audio_buffer.len() < 7 {
                return Ok(());
            }
            match parse_adts_header(&self.audio_buffer) {
                None => {
                    let drop = match self.audio_buffer.iter().skip(1).position(|b| *b == 0xFF) {
                        Some(position) => position + 1,
                        None => self.audio_buffer.len().saturating_sub(1),
                    };
                    self.audio_buffer.drain(..drop.max(1));
                }
                Some(header) => {
                    if self.audio_buffer.len() < header.frame_length {
                        return Ok(());
                    }
                    let frame =
                        self.audio_buffer[header.header_length..header.frame_length].to_vec();
                    if !self.audio_config_sent {
                        let config = AacConfig {
                            asc: header.asc.to_vec(),
                            sample_rate: header.sample_rate,
                            channels: header.channels,
                        };
                        self.audio_frame_ticks =
                            Some(1024 * 90_000 / i64::from(header.sample_rate.max(1)));
                        info!(
                            "MPEG-TS: AAC {} Hz, {} channel(s)",
                            config.sample_rate, config.channels
                        );
                        self.sink.set_audio_config(config)?;
                        self.audio_config_sent = true;
                    }
                    self.sink.audio_sample(&frame)?;
                    self.audio_frames += 1;
                    if let Some(next) = self.audio_next_pts.as_mut() {
                        *next += self.audio_frame_ticks.unwrap_or(2089);
                    }
                    self.audio_buffer.drain(..header.frame_length);
                }
            }
        }
    }

    fn finish(&mut self) -> Result<()> {
        if let Some((_code_start, _, nal_start)) = find_start_code(&self.video_buffer, 0) {
            let nal = self.video_buffer[nal_start..].to_vec();
            self.video_buffer.clear();
            self.process_nal(nal)?;
        }
        self.close_picture()?;
        self.flush_gop()?;
        if self.video_samples == 0 {
            if let Some(stream_type) = self.unsupported_video_type {
                bail!("unsupported video codec (stream type 0x{stream_type:02X}) in MPEG-TS");
            }
            bail!("no H.264 video found in MPEG-TS");
        }
        if !self.audio_config_sent && self.audio_frames == 0 {
            if let Some(stream_type) = self.unsupported_audio_type {
                bail!("unsupported audio codec (stream type 0x{stream_type:02X}) in MPEG-TS");
            }
            if self.audio_pid.is_some() {
                bail!("MPEG-TS declares an audio stream but no AAC frames were parsed");
            }
            warn!("MPEG-TS: no AAC audio found; muxing a video-only MP4");
        }
        debug!(
            "MPEG-TS: {} video samples, {} audio frames",
            self.video_samples, self.audio_frames
        );
        Ok(())
    }
}

// ─────────────────────────── H.264 parsing ───────────────────────────

fn parse_sps(nal: &[u8]) -> Result<H264Params> {
    if nal.len() < 4 {
        bail!("SPS is too short");
    }
    let rbsp = remove_emulation_prevention(&nal[1..]);
    let mut reader = BitReader::new(&rbsp);
    let profile_idc = reader.read_bits(8)?;
    let _constraints = reader.read_bits(8)?;
    let _level_idc = reader.read_bits(8)?;
    let _sps_id = reader.read_ue()?;
    let mut chroma_format_idc = 1u32;
    let mut separate_colour_plane = false;
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma_format_idc = reader.read_ue()?;
        if chroma_format_idc == 3 {
            separate_colour_plane = reader.read_bit()? == 1;
        }
        let _bit_depth_luma = reader.read_ue()?;
        let _bit_depth_chroma = reader.read_ue()?;
        let _qpprime = reader.read_bit()?;
        if reader.read_bit()? == 1 {
            bail!("H.264 scaling lists are not supported by the native remuxer");
        }
    }
    if chroma_format_idc != 1 {
        bail!("only 4:2:0 H.264 is supported by the native remuxer");
    }
    let log2_max_frame_num = reader.read_ue()? + 4;
    let poc_type = reader.read_ue()?;
    let mut log2_max_poc_lsb = 4;
    match poc_type {
        0 => {
            log2_max_poc_lsb = reader.read_ue()? + 4;
        }
        1 => bail!("H.264 pic_order_cnt_type 1 is not supported by the native remuxer"),
        _ => {}
    }
    let _max_num_ref_frames = reader.read_ue()?;
    let _gaps = reader.read_bit()?;
    let _width_in_mbs = reader.read_ue()?;
    let _height_in_map_units = reader.read_ue()?;
    let frame_mbs_only = reader.read_bit()? == 1;
    if !frame_mbs_only {
        bail!("interlaced H.264 is not supported by the native remuxer");
    }
    let _direct_8x8 = reader.read_bit()?;
    Ok(H264Params {
        log2_max_frame_num,
        poc_type,
        log2_max_poc_lsb,
        separate_colour_plane,
        ..H264Params::default()
    })
}

/// Minimal PPS parse: `bottom_field_pic_order_in_frame_present_flag`.
fn parse_pps(nal: &[u8]) -> Result<bool> {
    if nal.len() < 2 {
        bail!("PPS is too short");
    }
    let rbsp = remove_emulation_prevention(&nal[1..]);
    let mut reader = BitReader::new(&rbsp);
    let _pps_id = reader.read_ue()?;
    let _sps_id = reader.read_ue()?;
    let _entropy_coding_mode = reader.read_bit()?;
    Ok(reader.read_bit()? == 1)
}

fn parse_slice(nal: &[u8], params: &H264Params) -> Result<SliceInfo> {
    if nal.len() < 2 {
        bail!("truncated slice NAL unit");
    }
    let nal_type = nal[0] & 0x1F;
    let rbsp = remove_emulation_prevention(&nal[1..]);
    let mut reader = BitReader::new(&rbsp);
    let first_mb = reader.read_ue()?;
    let _slice_type = reader.read_ue()?; // not needed: POC drives reordering
    let _pps_id = reader.read_ue()?;
    if params.separate_colour_plane {
        let _colour_plane_id = reader.read_bits(2)?;
    }
    let _frame_num = reader.read_bits(params.log2_max_frame_num.min(16))?;
    // frame_mbs_only == false is rejected while parsing the SPS, so no field
    // flags follow.
    let is_idr = nal_type == 5;
    if is_idr {
        let _idr_pic_id = reader.read_ue()?;
    }
    let poc = match params.poc_type {
        0 => {
            let lsb = i64::from(reader.read_bits(params.log2_max_poc_lsb.min(16))?);
            Some(lsb)
        }
        2 => None, // display order == decode order
        other => bail!("unsupported H.264 pic_order_cnt_type {other}"),
    };
    Ok(SliceInfo {
        first_mb,
        poc,
        is_idr,
    })
}

/// Resolve the picture order count against the previous sample so the values
/// are monotonic across wraparounds (pic_order_cnt_type 0).
fn resolve_poc(raw_lsb: i64, params: &mut H264Params) -> i64 {
    let max_lsb = 1i64 << params.log2_max_poc_lsb.min(16);
    if raw_lsb < params.prev_poc_lsb && (params.prev_poc_lsb - raw_lsb) > max_lsb / 2 {
        params.poc_wrap += max_lsb;
    }
    params.prev_poc_lsb = raw_lsb;
    params.poc_wrap + raw_lsb
}

fn parse_avc_config(sps: &[u8], pps: &[u8]) -> Result<AvcConfig> {
    let rbsp = remove_emulation_prevention(&sps[1..]);
    let (width, height, frame_interval_ticks) = parse_sps_dimensions(&rbsp)?;
    Ok(AvcConfig {
        sps: sps.to_vec(),
        pps: pps.to_vec(),
        profile: sps[1],
        compat: sps[2],
        level: sps[3],
        width,
        height,
        frame_interval_ticks,
    })
}

fn parse_sps_dimensions(rbsp: &[u8]) -> Result<(u32, u32, Option<u32>)> {
    let mut reader = BitReader::new(rbsp);
    let profile_idc = reader.read_bits(8)?;
    let _constraints = reader.read_bits(8)?;
    let _level_idc = reader.read_bits(8)?;
    let _sps_id = reader.read_ue()?;
    let mut chroma_format_idc = 1u32;
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma_format_idc = reader.read_ue()?;
        if chroma_format_idc == 3 {
            let _separate_colour_plane = reader.read_bit()?;
        }
        let _bit_depth_luma = reader.read_ue()?;
        let _bit_depth_chroma = reader.read_ue()?;
        let _qpprime = reader.read_bit()?;
        if reader.read_bit()? == 1 {
            bail!("H.264 scaling lists are not supported by the native remuxer");
        }
    }
    if chroma_format_idc != 1 {
        bail!("only 4:2:0 H.264 is supported by the native remuxer");
    }
    let _log2_max_frame_num = reader.read_ue()?;
    let poc_type = reader.read_ue()?;
    match poc_type {
        0 => {
            let _ = reader.read_ue()?;
        }
        1 => {
            let _ = reader.read_bit()?;
            let _ = reader.read_se()?;
            let _ = reader.read_se()?;
            let cycle = reader.read_ue()?;
            for _ in 0..cycle.min(256) {
                let _ = reader.read_se()?;
            }
        }
        _ => {}
    }
    let _max_num_ref_frames = reader.read_ue()?;
    let _gaps = reader.read_bit()?;
    let width_in_mbs = reader.read_ue()? + 1;
    let height_in_map_units = reader.read_ue()? + 1;
    let frame_mbs_only = reader.read_bit()?;
    if frame_mbs_only == 0 {
        let _mb_adaptive = reader.read_bit()?;
    }
    let _direct_8x8 = reader.read_bit()?;
    let (mut crop_left, mut crop_right, mut crop_top, mut crop_bottom) = (0u32, 0u32, 0u32, 0u32);
    if reader.read_bit()? == 1 {
        crop_left = reader.read_ue()?;
        crop_right = reader.read_ue()?;
        crop_top = reader.read_ue()?;
        crop_bottom = reader.read_ue()?;
    }
    let crop_unit_x = 2u32;
    let crop_unit_y = if frame_mbs_only == 1 { 2u32 } else { 4u32 };
    let width = width_in_mbs * 16 - (crop_left + crop_right) * crop_unit_x;
    let height =
        (2 - frame_mbs_only) * height_in_map_units * 16 - (crop_top + crop_bottom) * crop_unit_y;
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        bail!("implausible H.264 frame size {width}x{height}");
    }

    let mut frame_interval_ticks = None;
    if reader.read_bit()? == 1 {
        if reader.read_bit()? == 1 {
            let aspect_ratio_idc = reader.read_bits(8)?;
            if aspect_ratio_idc == 255 {
                let _ = reader.read_bits(16)?;
                let _ = reader.read_bits(16)?;
            }
        }
        if reader.read_bit()? == 1 {
            let _ = reader.read_bit()?;
        }
        if reader.read_bit()? == 1 {
            let _ = reader.read_bits(3)?;
            if reader.read_bit()? == 1 {
                let _ = reader.read_bits(24)?;
            }
        }
        if reader.read_bit()? == 1 {
            let _ = reader.read_ue()?;
            let _ = reader.read_ue()?;
        }
        if reader.read_bit()? == 1 {
            let num_units_in_tick = reader.read_bits(32)?;
            let time_scale = reader.read_bits(32)?;
            let _fixed_frame_rate = reader.read_bit()?;
            if num_units_in_tick > 0 && time_scale > 0 {
                let interval = 2u64 * u64::from(num_units_in_tick) * 90_000 / u64::from(time_scale);
                if interval > 0 {
                    frame_interval_ticks = Some(u32::try_from(interval).unwrap_or(u32::MAX));
                }
            }
        }
    }
    Ok((width, height, frame_interval_ticks))
}

// ─────────────────────────── ADTS parsing ───────────────────────────

struct AdtsHeader {
    header_length: usize,
    frame_length: usize,
    sample_rate: u32,
    channels: u16,
    asc: [u8; 2],
}

const AAC_SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

fn parse_adts_header(buffer: &[u8]) -> Option<AdtsHeader> {
    if buffer.len() < 7 || buffer[0] != 0xFF || (buffer[1] & 0xF6) != 0xF0 {
        return None;
    }
    let protection_absent = buffer[1] & 0x01 == 1;
    let profile = (buffer[2] >> 6) & 0x03;
    let sample_rate_index = usize::from((buffer[2] >> 2) & 0x0F);
    let channel_config = ((buffer[2] & 0x01) << 2) | (buffer[3] >> 6);
    let frame_length = ((usize::from(buffer[3]) & 0x03) << 11)
        | (usize::from(buffer[4]) << 3)
        | (usize::from(buffer[5]) >> 5);
    let header_length = if protection_absent { 7 } else { 9 };
    if sample_rate_index >= AAC_SAMPLE_RATES.len() || frame_length <= header_length {
        return None;
    }
    let object_type = profile + 1;
    let sample_rate = AAC_SAMPLE_RATES[sample_rate_index];
    let asc = [
        (object_type << 3) | (sample_rate_index as u8 >> 1),
        ((sample_rate_index as u8 & 0x01) << 7) | (channel_config << 3),
    ];
    Some(AdtsHeader {
        header_length,
        frame_length,
        sample_rate,
        channels: u16::from(channel_config),
        asc,
    })
}

fn read_pts(bytes: &[u8]) -> Result<i64> {
    if bytes.len() < 5 {
        bail!("truncated PTS field");
    }
    let value = (i64::from(bytes[0] >> 1) & 0x07) << 30
        | (i64::from((u16::from(bytes[1]) << 8 | u16::from(bytes[2])) >> 1) << 15)
        | i64::from(u16::from(bytes[3]) << 8 | u16::from(bytes[4])) >> 1;
    Ok(value)
}

/// Returns `(start_code_position, start_code_length, nal_start)`.
fn find_start_code(data: &[u8], from: usize) -> Option<(usize, usize, usize)> {
    if data.len() < 3 || from + 3 > data.len() {
        return None;
    }
    let mut index = from;
    while index + 2 < data.len() {
        if data[index] == 0 && data[index + 1] == 0 && data[index + 2] == 1 {
            let four = index > from && data[index - 1] == 0;
            let start = if four { index - 1 } else { index };
            return Some((start, if four { 4 } else { 3 }, index + 3));
        }
        index += 1;
    }
    None
}

fn remove_emulation_prevention(data: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len());
    let mut index = 0usize;
    while index < data.len() {
        if index + 2 < data.len()
            && data[index] == 0
            && data[index + 1] == 0
            && data[index + 2] == 3
        {
            output.push(0);
            output.push(0);
            index += 3;
            continue;
        }
        output.push(data[index]);
        index += 1;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_mpeg_ts() {
        let mut buffer = vec![0u8; TS_PACKET_SIZE * 3 + 1];
        for offset in [0usize, TS_PACKET_SIZE, TS_PACKET_SIZE * 2] {
            buffer[offset] = 0x47;
        }
        assert!(is_mpeg_ts(&buffer));
        buffer[TS_PACKET_SIZE] = 0x00;
        assert!(!is_mpeg_ts(&buffer));
    }

    #[test]
    fn parses_adts_header() {
        let frame_length = 100usize;
        let mut frame = vec![0u8; frame_length];
        frame[0] = 0xFF;
        frame[1] = 0xF1; // MPEG-4, layer 0, protection absent
        frame[2] = 0b0101_0000; // profile=LC, sf_index=4 (44100), channel cfg high bit=0
        frame[3] = 0b1000_0000; // channel config low bits = 2, frame length high bits = 0
        frame[4] = ((frame_length >> 3) & 0xFF) as u8;
        frame[5] = (((frame_length & 0x07) << 5) & 0xE0) as u8;
        let header = parse_adts_header(&frame).expect("valid ADTS header");
        assert_eq!(header.header_length, 7);
        assert_eq!(header.frame_length, frame_length);
        assert_eq!(header.sample_rate, 44100);
        assert_eq!(header.channels, 2);
        assert_eq!(header.asc, [0x12, 0x10]);
    }

    #[test]
    fn finds_start_codes() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x67, 0x00, 0x00, 0x01, 0x68];
        let first = find_start_code(&data, 0).unwrap();
        assert_eq!(first, (0, 4, 4));
        let second = find_start_code(&data, 5).unwrap();
        assert_eq!(second, (5, 3, 8));
    }

    #[test]
    fn removes_emulation_prevention_bytes() {
        let input = [0x00, 0x00, 0x03, 0x01, 0x00, 0x00, 0x03, 0x02];
        assert_eq!(
            remove_emulation_prevention(&input),
            vec![0x00, 0x00, 0x01, 0x00, 0x00, 0x02]
        );
    }

    #[test]
    fn resolves_poc_wraparound() {
        let mut params = H264Params {
            log2_max_poc_lsb: 8,
            ..H264Params::default()
        };
        assert_eq!(resolve_poc(250, &mut params), 250);
        // Wrap: 250 -> 4 crosses the LSB cycle.
        assert_eq!(resolve_poc(4, &mut params), 260);
        assert_eq!(resolve_poc(8, &mut params), 264);
    }
}
