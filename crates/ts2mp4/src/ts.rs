//! MPEG-TS demuxer for the usual HLS payload: H.264 video and AAC (ADTS) audio; other audio is named, not read.

use crate::codec;
use alloc::{string::String, vec::Vec};

/// One segment's media. Timestamps are in 90 kHz units.
pub struct Media {
    pub video: Option<Video>,
    pub audio: Option<Audio>,
    /// The names of the audio codecs the PMT lists that this player cannot play (AC-3, E-AC-3, MPEG audio): those
    /// tracks are left out.
    pub skipped: Vec<&'static str>,
}

pub struct Video {
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub samples: Vec<Sample>,
}

pub struct Audio {
    /// AudioSpecificConfig: object type, sample rate index, channels.
    pub config: [u8; 2],
    pub rate: u32,
    pub channels: u8,
    pub samples: Vec<Sample>,
}

pub struct Sample {
    pub pts: i64,
    pub dts: i64,
    pub key: bool,
    /// Video: length-prefixed NAL units. Audio: one raw AAC frame.
    pub data: Vec<u8>,
}

impl Media {
    /// The earliest timestamp.
    pub fn start(&self) -> Option<i64> {
        let video = self.video.as_ref().map(|v| v.samples[0].dts);
        let audio = self.audio.as_ref().map(|a| a.samples[0].pts);
        video.into_iter().chain(audio).min()
    }

    /// MIME type for Media Source, e.g. `video/mp4; codecs="avc1.64001f,mp4a.40.2"`.
    pub fn mime(&self) -> String {
        let mut codecs = Vec::new();
        if let Some(v) = &self.video {
            codecs.push(codec::avc1(&v.sps[1..4]));
        }
        if let Some(object) = self.audio.as_ref().and_then(|a| codec::aac_object(&a.config)) {
            codecs.push(codec::mp4a(object));
        }
        codec::mime(&codecs)
    }

    /// The codec setup: what `init` reads, the SPS and PPS (the picture size comes from the SPS) and the audio
    /// config (the rate and channels come from it), with their lengths so that no two setups give the same bytes.
    /// Segments with the same setup get the same init segment, so a player can keep it and build another only
    /// when it changes.
    pub fn setup(&self) -> Vec<u8> {
        let (sps, pps) = self.video.as_ref().map_or((&[][..], &[][..]), |v| (&v.sps, &v.pps));
        let config = self.audio.as_ref().map_or(&[][..], |a| &a.config);
        [&(sps.len() as u16).to_be_bytes(), &(pps.len() as u16).to_be_bytes(), sps, pps, config].concat()
    }
}

pub fn demux(ts: &[u8]) -> Media {
    demux_near(ts, None)
}

/// Like `demux`, with the segment's timestamps taken past as many wraps of the clock as put them nearest `near`:
/// they go back to 0 every 26.5 h, and a live stream that runs longer goes on from there. Within the segment,
/// each timestamp is unwrapped against the one before it, so samples keep their order across a wrap.
pub fn demux_near(ts: &[u8], near: Option<i64>) -> Media {
    let (video_pes, audio_pes, skipped) = split(ts);
    let video = video(&video_pes, near);
    // Against the video rather than `near`: a segment's two tracks are on the same side of a wrap.
    let near = video.as_ref().and_then(|v| v.samples.first()).map(|s| s.dts).or(near);
    Media { video, audio: audio(&audio_pes, near), skipped }
}

/// PTS and DTS are 33 bits at 90 kHz: they wrap every 2^33 ticks.
const WRAP: i64 = 1 << 33;

/// The timestamp `t` plus as many wraps as put it nearest `near`, before or after it; as it is with nothing to be near.
fn unwrap(t: i64, near: Option<i64>) -> i64 {
    near.map_or(t, |near| t + (near - t + WRAP / 2).div_euclid(WRAP) * WRAP)
}

/// TS packets -> the PES packets of the first H.264 and the first AAC stream, and the audio codecs left out.
fn split(ts: &[u8]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>, Vec<&'static str>) {
    let (mut pmt, mut pids) = (None, Streams::default());
    let (mut video, mut audio) = (Vec::new(), Vec::new());
    for p in ts.as_chunks::<188>().0.iter().filter(|p| p[0] == 0x47) {
        let pid = u16::from_be_bytes([p[1] & 0x1f, p[2]]);
        let start = p[1] & 0x40 != 0;
        let skip = if p[3] & 0x20 != 0 { 5 + usize::from(p[4]) } else { 4 }; // adaptation field
        let Some(payload) = p.get(skip..).filter(|_| p[3] & 0x10 != 0) else { continue };
        if pid == 0 && start {
            pmt = pat(payload).or(pmt);
        } else if Some(pid) == pmt && start {
            pids = streams(payload).unwrap_or(pids);
        } else if let Some(list) = match pid {
            _ if Some(pid) == pids.video => Some(&mut video),
            _ if Some(pid) == pids.audio => Some(&mut audio),
            _ => None,
        } {
            if start {
                list.push(Vec::new());
            }
            if let Some(pes) = list.last_mut() {
                pes.extend_from_slice(payload);
            }
        }
    }
    (video, audio, pids.skipped)
}

/// A PSI section (PAT or PMT) from table_id up to, not including, its CRC.
fn section(payload: &[u8]) -> Option<&[u8]> {
    let s = payload.get(1 + usize::from(*payload.first()?)..)?;
    let len = usize::from(*s.get(1)? & 0x0f) << 8 | usize::from(*s.get(2)?);
    s.get(..(3 + len).checked_sub(4)?)
}

/// PAT -> PID of the first program's PMT.
fn pat(payload: &[u8]) -> Option<u16> {
    let programs = section(payload)?.get(8..)?.as_chunks::<4>().0;
    programs.iter().find(|e| e[..2] != [0, 0]).map(|e| u16::from_be_bytes([e[2] & 0x1f, e[3]]))
}

/// What a PMT lists: the PIDs of the H.264 and AAC streams, and the audio codecs this player cannot play.
#[derive(Default)]
struct Streams {
    video: Option<u16>,
    audio: Option<u16>,
    skipped: Vec<&'static str>,
}

/// PMT -> its streams: H.264 is type 0x1b, AAC 0x0f.
fn streams(payload: &[u8]) -> Option<Streams> {
    let s = section(payload)?;
    let mut i = 12 + (usize::from(*s.get(10)? & 0x0f) << 8 | usize::from(*s.get(11)?));
    let mut found = Streams::default();
    while let Some(e) = s.get(i..i + 5) {
        let pid = Some(u16::from_be_bytes([e[1] & 0x1f, e[2]]));
        let end = i + 5 + (usize::from(e[3] & 0x0f) << 8 | usize::from(e[4]));
        match e[0] {
            0x1b => found.video = found.video.or(pid),
            0x0f => found.audio = found.audio.or(pid),
            kind => {
                let name = match kind {
                    0x03 | 0x04 => Some("MPEG audio"),
                    0x81 => Some("AC-3"),
                    0x87 => Some("E-AC-3"),
                    0x06 => s.get(i + 5..end).and_then(dolby),
                    _ => None,
                };
                if let Some(name) = name.filter(|n| !found.skipped.contains(n)) {
                    found.skipped.push(name);
                }
            }
        }
        i = end;
    }
    Some(found)
}

/// The descriptors of a PES private data stream (type 0x06) -> "AC-3" or "E-AC-3" when they say it is one.
fn dolby(mut descriptors: &[u8]) -> Option<&'static str> {
    while let [tag, len, rest @ ..] = descriptors {
        let (body, next) = rest.split_at_checked(usize::from(*len))?;
        match (tag, body) {
            (0x6a, _) | (0x05, b"AC-3") => return Some("AC-3"),
            (0x7a, _) | (0x05, b"EAC3") => return Some("E-AC-3"),
            _ => descriptors = next,
        }
    }
    None
}

/// (pts, dts) of a PES packet.
type Times = (i64, i64);

/// PES packet -> its times when present, and the payload.
fn pes(data: &[u8]) -> Option<(Option<Times>, &[u8])> {
    if data.get(..3)? != [0, 0, 1] {
        return None;
    }
    let flags = *data.get(7)? >> 6;
    let payload = data.get(9 + usize::from(*data.get(8)?)..)?;
    let pts = if flags & 2 != 0 { Some(timestamp(data.get(9..14)?)) } else { None };
    let dts = if flags == 3 { Some(timestamp(data.get(14..19)?)) } else { pts };
    Some((pts.zip(dts), payload))
}

fn timestamp(b: &[u8]) -> i64 {
    i64::from((b[0] >> 1) & 7) << 30
        | i64::from(b[1]) << 22
        | i64::from(b[2] >> 1) << 15
        | i64::from(b[3]) << 7
        | i64::from(b[4] >> 1)
}

/// PES packets -> video frames, one per PES (the norm in HLS), the first one timed at the wrap nearest `near`.
fn video(list: &[Vec<u8>], near: Option<i64>) -> Option<Video> {
    let (mut sps, mut pps, mut samples) = (Vec::new(), Vec::new(), Vec::<Sample>::new());
    for data in list {
        let Some((times, payload)) = pes(data) else { continue };
        let (mut key, mut frame) = (false, Vec::new());
        for nal in nal_units(payload) {
            match nal[0] & 0x1f {
                7 if sps.is_empty() => sps = nal.to_vec(),
                8 if pps.is_empty() => pps = nal.to_vec(),
                7..=9 => {} // parameter sets go in the init segment; drop access unit delimiters
                kind => {
                    key |= kind == 5;
                    frame.extend((nal.len() as u32).to_be_bytes());
                    frame.extend_from_slice(nal);
                }
            }
        }
        match (times, samples.last_mut()) {
            (Some((pts, dts)), _) => {
                let dts = unwrap(dts, samples.last().map(|s| s.dts).or(near));
                samples.push(Sample { pts: unwrap(pts, Some(dts)), dts, key, data: frame });
            }
            (None, Some(last)) => last.data.extend(frame), // a frame split over two PES
            (None, None) => {}
        }
    }
    if sps.len() < 4 || pps.is_empty() || samples.is_empty() {
        return None;
    }
    let (width, height) = picture_size(&sps);
    Some(Video { sps, pps, width, height, samples })
}

/// Splits an Annex B byte stream on its `00 00 01` start codes.
fn nal_units(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let starts: Vec<usize> = data.windows(3).enumerate().filter(|(_, w)| *w == [0, 0, 1]).map(|(i, _)| i + 3).collect();
    (0..starts.len()).filter_map(move |k| {
        let end = starts.get(k + 1).map_or(data.len(), |next| next - 3);
        let nal = data.get(starts[k]..end)?;
        let len = nal.iter().rposition(|&b| b != 0)? + 1; // trailing zeros belong to the next start code
        Some(&nal[..len])
    })
}

const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

/// PES packets -> AAC frames, timed from the first PTS at the wrap nearest `near` (frames may straddle PES packets).
fn audio(list: &[Vec<u8>], near: Option<i64>) -> Option<Audio> {
    let (mut stream, mut start) = (Vec::new(), None);
    for (times, payload) in list.iter().filter_map(|data| pes(data)) {
        start = start.or(times.map(|(pts, _)| pts));
        stream.extend_from_slice(payload);
    }
    let start = unwrap(start?, near);
    let (mut i, mut audio) = (0, None::<Audio>);
    // An ADTS sync word: twelve set bits, then the ID bit, then the two layer bits, always 00.
    let sync = |h: &[u8]| matches!(h, [0xff, b, ..] if b & 0xf6 == 0xf0);
    while let Some(h) = stream.get(i..i + 7) {
        if !sync(h) {
            i += 1; // resync on the next ADTS header
            continue;
        }
        let (object, freq, channels) = ((h[2] >> 6) + 1, (h[2] >> 2) & 15, (h[2] & 1) << 2 | h[3] >> 6);
        let config = [object << 3 | freq >> 1, (freq & 1) << 7 | channels << 3];
        let len = usize::from(h[3] & 3) << 11 | usize::from(h[4]) << 3 | usize::from(h[5] >> 5);
        let header = if h[1] & 1 == 1 { 7 } else { 9 };
        // A reserved rate, a config other than the first frame's, or a length that is shorter than the header or does
        // not end where the next header starts (or the stream ends) means a corrupt byte passed for a sync word: resync
        // past it rather than drop the whole track, mix two configs in it, or lock in the corrupt one as the track's.
        let rate = RATES.get(usize::from(freq)).filter(|_| audio.as_ref().is_none_or(|a| a.config == config));
        let frame = stream.get(i + header..i + len).filter(|_| stream.get(i + len..).is_some_and(|rest| rest.is_empty() || sync(rest)));
        let (Some(&rate), Some(frame)) = (rate, frame) else {
            i += 1;
            continue;
        };
        let a = audio.get_or_insert_with(|| Audio { config, rate, channels, samples: Vec::new() });
        let pts = start + a.samples.len() as i64 * 1024 * 90_000 / i64::from(a.rate);
        a.samples.push(Sample { pts, dts: pts, key: true, data: frame.to_vec() });
        i += len;
    }
    audio
}

/// Picture size from an H.264 sequence parameter set (ITU-T H.264, 7.3.2.1.1).
fn picture_size(sps: &[u8]) -> (u32, u32) {
    // Drop emulation prevention bytes: 00 00 03 -> 00 00.
    let (mut rbsp, mut zeros) = (Vec::new(), 0);
    for &b in &sps[1..] {
        if zeros == 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        rbsp.push(b);
    }
    let mut r = Bits { data: &rbsp, pos: 0 };
    let profile = r.bits(8);
    r.bits(16); // constraint flags, level
    r.ue(); // seq_parameter_set_id
    let mut chroma = 1;
    if matches!(profile, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
        chroma = r.ue();
        if chroma == 3 {
            r.bits(1); // separate_colour_plane_flag
        }
        r.ue(); // bit_depth_luma_minus8
        r.ue(); // bit_depth_chroma_minus8
        r.bits(1); // qpprime_y_zero_transform_bypass_flag
        if r.bits(1) == 1 {
            for i in 0..if chroma == 3 { 12 } else { 8 } {
                if r.bits(1) == 1 {
                    r.skip_scaling_list(if i < 6 { 16 } else { 64 });
                }
            }
        }
    }
    r.ue(); // log2_max_frame_num_minus4
    match r.ue() {
        0 => {
            r.ue(); // log2_max_pic_order_cnt_lsb_minus4
        }
        1 => {
            r.bits(1);
            r.se();
            r.se();
            for _ in 0..r.ue().min(255) {
                r.se();
            }
        }
        _ => {}
    }
    r.ue(); // max_num_ref_frames
    r.bits(1); // gaps_in_frame_num_value_allowed_flag
    let (w, h) = (r.ue() + 1, r.ue() + 1); // in macroblocks / map units
    let frame_mbs_only = r.bits(1);
    if frame_mbs_only == 0 {
        r.bits(1); // mb_adaptive_frame_field_flag
    }
    r.bits(1); // direct_8x8_inference_flag
    let crop = if r.bits(1) == 1 { [r.ue(), r.ue(), r.ue(), r.ue()] } else { [0; 4] };
    let field = 2 - frame_mbs_only;
    let (crop_x, crop_y) = match chroma {
        0 => (1, field),
        1 => (2, 2 * field),
        2 => (2, field),
        _ => (1, field),
    };
    // Saturating throughout: a malformed SPS must not overflow (a panic in debug, a wrong size in release).
    let width = w.saturating_mul(16).saturating_sub(crop[0].saturating_add(crop[1]).saturating_mul(crop_x));
    let height = field.saturating_mul(h).saturating_mul(16).saturating_sub(crop[2].saturating_add(crop[3]).saturating_mul(crop_y));
    (width, height)
}

/// Big-endian bit reader; reads past the end yield zeros.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bits(&mut self, n: u32) -> u32 {
        (0..n).fold(0, |v, _| {
            let bit = self.data.get(self.pos / 8).map_or(0, |b| b >> (7 - self.pos % 8) & 1);
            self.pos += 1;
            v << 1 | u32::from(bit)
        })
    }

    /// Unsigned Exp-Golomb.
    fn ue(&mut self) -> u32 {
        let mut zeros = 0;
        while self.bits(1) == 0 && zeros < 31 {
            zeros += 1;
        }
        ((1u64 << zeros) - 1 + u64::from(self.bits(zeros))) as u32
    }

    /// Signed Exp-Golomb.
    fn se(&mut self) -> i64 {
        let k = i64::from(self.ue());
        if k % 2 == 1 { (k + 1) / 2 } else { -k / 2 }
    }

    fn skip_scaling_list(&mut self, size: usize) {
        let (mut last, mut next) = (8, 8);
        for _ in 0..size {
            if next != 0 {
                next = (last + self.se()).rem_euclid(256);
            }
            if next != 0 {
                last = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// An 8-byte ADTS frame (no CRC) of AAC object type `object`, rate index `freq` and `channels`, with a one-byte payload.
    fn adts(object: u8, freq: u8, channels: u8, payload: u8) -> [u8; 8] {
        [0xff, 0xf1, (object - 1) << 6 | freq << 2 | channels >> 2, (channels & 3) << 6, 1, 0x1f, 0xfc, payload]
    }

    /// One PES packet with PTS 0 carrying `frames`.
    fn packet(frames: &[[u8; 8]]) -> Vec<u8> {
        let mut packet = vec![0, 0, 1, 0xc0, 0, 0, 0x80, 0x80, 5];
        packet.extend(time(2, 0));
        packet.extend(frames.iter().flatten());
        packet
    }

    fn payloads(audio: &Audio) -> Vec<u8> {
        audio.samples.iter().flat_map(|s| s.data.iter().copied()).collect()
    }

    #[test]
    fn a_header_with_a_reserved_rate_is_skipped_and_the_other_frames_kept() {
        let audio = audio(&[packet(&[adts(2, 4, 2, 1), adts(2, 13, 2, 2), adts(2, 4, 2, 3), adts(2, 15, 2, 4), adts(2, 4, 2, 5)])], None).unwrap();
        assert_eq!((audio.rate, audio.channels), (44100, 2));
        assert_eq!(payloads(&audio), [1, 3, 5]);
    }

    #[test]
    fn a_frame_with_another_object_type_rate_or_channel_count_than_the_first_is_skipped() {
        let frames = [adts(2, 4, 2, 1), adts(1, 4, 2, 2), adts(2, 3, 2, 3), adts(2, 4, 1, 4), adts(2, 4, 2, 5)];
        let audio = audio(&[packet(&frames)], None).unwrap();
        assert_eq!((audio.config, audio.rate, audio.channels), ([0x12, 0x10], 44100, 2));
        assert_eq!(payloads(&audio), [1, 5]);
    }

    /// `frame` with its length field set to `len`.
    fn sized(mut frame: [u8; 8], len: u16) -> [u8; 8] {
        frame[3] = frame[3] & 0xfc | (len >> 11) as u8;
        frame[4] = (len >> 3) as u8;
        frame[5] = (len << 5) as u8 | 0x1f;
        frame
    }

    #[test]
    fn a_header_whose_length_does_not_lead_to_another_header_is_skipped() {
        // A mono false sync first, 12 bytes long: before, its config was locked in and every real frame skipped.
        let audio = audio(&[packet(&[sized(adts(2, 4, 1, 9), 12), adts(2, 4, 2, 1), adts(2, 4, 2, 3)])], None).unwrap();
        assert_eq!((audio.rate, audio.channels), (44100, 2));
        assert_eq!(payloads(&audio), [1, 3]);
    }

    #[test]
    fn a_header_shorter_than_itself_is_skipped_and_the_scan_goes_on() {
        for len in [0, 5] {
            let audio = audio(&[packet(&[adts(2, 4, 2, 1), sized(adts(2, 4, 2, 2), len), adts(2, 4, 2, 3), adts(2, 4, 2, 5)])], None).unwrap();
            assert_eq!(payloads(&audio), [1, 3, 5], "length {len}");
        }
    }

    #[test]
    fn a_header_with_layer_bits_set_is_not_a_sync_word() {
        // A mono header whose length does lead to the next one: only its layer bits give it away.
        let mut fake = adts(2, 4, 1, 9);
        fake[1] |= 0x06;
        let audio = audio(&[packet(&[fake, adts(2, 4, 2, 1), adts(2, 4, 2, 3)])], None).unwrap();
        assert_eq!((audio.rate, audio.channels), (44100, 2));
        assert_eq!(payloads(&audio), [1, 3]);
    }

    /// A PES timestamp field: `prefix` in the top four bits, `t` modulo 2^33 (as a stream carries it) around marker bits.
    fn time(prefix: u8, t: i64) -> [u8; 5] {
        [prefix << 4 | (t >> 29) as u8 & 0x0e | 1, (t >> 22) as u8, (t >> 14) as u8 | 1, (t >> 7) as u8, (t << 1) as u8 | 1]
    }

    /// One video PES packet with a PTS and a DTS, the parameter sets of a 16x16 picture and an IDR slice.
    fn frame(pts: i64, dts: i64) -> Vec<u8> {
        let mut packet = vec![0, 0, 1, 0xe0, 0, 0, 0x80, 0xc0, 10];
        packet.extend(time(3, pts));
        packet.extend(time(1, dts));
        packet.extend([0, 0, 1, 0x67, 0x42, 0, 0x1e, 0xfb, 0xc0, 0, 0, 1, 0x68, 0xce, 0, 0, 1, 0x65, 0x88]);
        packet
    }

    /// (pts, dts) of each video sample.
    fn times(video: &Video) -> Vec<(i64, i64)> {
        video.samples.iter().map(|s| (s.pts, s.dts)).collect()
    }

    #[test]
    fn a_wrap_inside_a_segment_keeps_sample_order_and_durations() {
        // DTS steps over the wrap on the second frame; the first frame's PTS is past it already.
        let list = [frame(WRAP + 3000, WRAP - 3000), frame(WRAP + 6000, WRAP), frame(WRAP + 9000, WRAP + 3000)];
        let video = video(&list, None).unwrap();
        assert_eq!(times(&video), [(WRAP + 3000, WRAP - 3000), (WRAP + 6000, WRAP), (WRAP + 9000, WRAP + 3000)]);
    }

    #[test]
    fn a_segment_after_a_wrap_goes_on_from_the_timestamp_it_is_near() {
        // The previous segment ended just before the wrap (or, 26.5 h later, the second one): this one starts after.
        for wraps in [1, 2] {
            let near = wraps * WRAP - 90_000;
            let video = video(&[frame(3000, 0), frame(6000, 3000)], Some(near)).unwrap();
            assert_eq!(times(&video), [(wraps * WRAP + 3000, wraps * WRAP), (wraps * WRAP + 6000, wraps * WRAP + 3000)]);
            assert_eq!(audio(&[packet(&[adts(2, 4, 2, 1)])], Some(near)).unwrap().samples[0].pts, wraps * WRAP, "PTS 0");
        }
    }

    #[test]
    fn a_segment_before_a_wrap_stays_there_when_near_a_later_one() {
        // A seek back from after the wrap to before it.
        let video = video(&[frame(WRAP - 87_000, WRAP - 90_000)], Some(WRAP + 90_000)).unwrap();
        assert_eq!(times(&video), [(WRAP - 87_000, WRAP - 90_000)]);
    }
}
