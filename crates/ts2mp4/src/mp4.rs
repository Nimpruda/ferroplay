//! Fragmented MP4 writer (ISO/IEC 14496-12): an init segment, then one `moof` + `mdat` per segment.

use super::ts::{Audio, Media, Video};
use alloc::vec::Vec;

const VIDEO: u32 = 1;
const AUDIO: u32 = 2;
const MATRIX: [u8; 36] = [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0, 0];

/// A box: size, type, body.
fn bx(kind: &[u8; 4], body: &[&[u8]]) -> Vec<u8> {
    let body = body.concat();
    [&(body.len() as u32 + 8).to_be_bytes()[..], kind, &body].concat()
}

/// A full box: its body starts with a version byte and 24 bits of flags.
fn full(kind: &[u8; 4], version_flags: u32, body: &[&[u8]]) -> Vec<u8> {
    bx(kind, &[&version_flags.to_be_bytes(), &body.concat()])
}

/// `ftyp` + `moov`: what the tracks are, no samples.
pub fn init(m: &Media) -> Vec<u8> {
    let (mut traks, mut trexs) = (Vec::new(), Vec::new());
    if let Some(v) = &m.video {
        traks.push(trak(VIDEO, 90_000, v.width, v.height, &avc1(v)));
        trexs.push(trex(VIDEO));
    }
    if let Some(a) = &m.audio {
        traks.push(trak(AUDIO, a.rate, 0, 0, &mp4a(a)));
        trexs.push(trex(AUDIO));
    }
    let mvhd = full(b"mvhd", 0, &[&[0; 8], &1000u32.to_be_bytes(), &[0; 4], &[0, 1, 0, 0, 1, 0], &[0; 10], &MATRIX, &[0; 24], &[0xff; 4]]);
    let moov = bx(b"moov", &[&mvhd, &traks.concat(), &bx(b"mvex", &[&trexs.concat()])]);
    [bx(b"ftyp", &[b"isom", &[0, 0, 0, 1], b"isomavc1"]), moov].concat()
}

/// Track defaults: sample description 1, everything else per sample.
fn trex(id: u32) -> Vec<u8> {
    full(b"trex", 0, &[&id.to_be_bytes(), &1u32.to_be_bytes(), &[0; 12]])
}

fn trak(id: u32, timescale: u32, width: u32, height: u32, entry: &[u8]) -> Vec<u8> {
    let video = id == VIDEO;
    let tkhd = full(b"tkhd", 3, &[&[0; 8], &id.to_be_bytes(), &[0; 20], &[u8::from(!video), 0, 0, 0], &MATRIX, &(width << 16).to_be_bytes(), &(height << 16).to_be_bytes()]);
    let mdhd = full(b"mdhd", 0, &[&[0; 8], &timescale.to_be_bytes(), &[0; 4], &[0x55, 0xc4, 0, 0]]);
    let hdlr = full(b"hdlr", 0, &[&[0; 4], if video { b"vide" } else { b"soun" }, &[0; 13]]);
    let header = if video { full(b"vmhd", 1, &[&[0; 8]]) } else { full(b"smhd", 0, &[&[0; 4]]) };
    let dinf = bx(b"dinf", &[&full(b"dref", 0, &[&1u32.to_be_bytes(), &full(b"url ", 1, &[])])]);
    let stbl = bx(b"stbl", &[
        &full(b"stsd", 0, &[&1u32.to_be_bytes(), entry]),
        &full(b"stts", 0, &[&[0; 4]]),
        &full(b"stsc", 0, &[&[0; 4]]),
        &full(b"stsz", 0, &[&[0; 8]]),
        &full(b"stco", 0, &[&[0; 4]]),
    ]);
    bx(b"trak", &[&tkhd, &bx(b"mdia", &[&mdhd, &hdlr, &bx(b"minf", &[&header, &dinf, &stbl])])])
}

fn avc1(v: &Video) -> Vec<u8> {
    let avcc = bx(b"avcC", &[
        &[1, v.sps[1], v.sps[2], v.sps[3], 0xff, 0xe1], // 4-byte NAL lengths, one SPS
        &(v.sps.len() as u16).to_be_bytes(),
        &v.sps,
        &[1],
        &(v.pps.len() as u16).to_be_bytes(),
        &v.pps,
    ]);
    let size = [(v.width as u16).to_be_bytes(), (v.height as u16).to_be_bytes()].concat();
    bx(b"avc1", &[&[0; 6], &[0, 1], &[0; 16], &size, &[0, 0x48, 0, 0, 0, 0x48, 0, 0], &[0; 4], &[0, 1], &[0; 32], &[0, 0x18, 0xff, 0xff], &avcc])
}

fn mp4a(a: &Audio) -> Vec<u8> {
    let desc = |tag: u8, body: &[&[u8]]| {
        let body = body.concat();
        [&[tag, body.len() as u8][..], &body].concat()
    };
    let decoder = desc(4, &[&[0x40, 0x15], &[0; 11], &desc(5, &[&a.config])]); // AAC, audio stream
    let esds = full(b"esds", 0, &[&desc(3, &[&[0, 0, 0], &decoder, &desc(6, &[&[2]])])]);
    let rate = ((a.rate & 0xffff) << 16).to_be_bytes();
    bx(b"mp4a", &[&[0; 6], &[0, 1], &[0; 8], &[0, a.channels, 0, 16], &[0; 4], &rate, &esds])
}

/// One `moof` + `mdat` with all of a segment's samples. `offset` shifts the 90 kHz timestamps.
pub fn fragment(m: &Media, sequence: u32, offset: i64) -> Vec<u8> {
    let mut runs = Vec::new();
    if let Some(v) = &m.video {
        runs.push(video_run(v, offset));
    }
    if let Some(a) = &m.audio {
        runs.push(audio_run(a, offset));
    }
    let moof = |offsets: &[u32]| {
        let trafs: Vec<Vec<u8>> = runs.iter().zip(offsets).map(|(run, &at)| run.traf(at)).collect();
        bx(b"moof", &[&full(b"mfhd", 0, &[&sequence.to_be_bytes()]), &trafs.concat()])
    };
    // Sample data offsets count from the start of the moof, so its size is needed first.
    let mut at = moof(&[0, 0]).len() as u32 + 8;
    let mut offsets = Vec::new();
    for run in &runs {
        offsets.push(at);
        at += run.data.len() as u32;
    }
    let data: Vec<&[u8]> = runs.iter().map(|run| run.data.as_slice()).collect();
    [moof(&offsets), bx(b"mdat", &data)].concat()
}

/// One track's part of a fragment.
struct Run {
    track: u32,
    base: u64,
    /// Which per-sample trun fields are present: 0x100 duration, 0x200 size, 0x400 flags, 0x800 time offset.
    fields: u32,
    samples: Vec<[u32; 4]>,
    data: Vec<u8>,
}

impl Run {
    fn traf(&self, data_offset: u32) -> Vec<u8> {
        let mut table = Vec::new();
        for sample in &self.samples {
            for (k, value) in sample.iter().enumerate() {
                if self.fields & (0x100 << k) != 0 {
                    table.extend(value.to_be_bytes());
                }
            }
        }
        bx(b"traf", &[
            &full(b"tfhd", 0x2_0000, &[&self.track.to_be_bytes()]), // default-base-is-moof
            &full(b"tfdt", 1 << 24, &[&self.base.to_be_bytes()]),
            &full(b"trun", 1 | self.fields, &[&(self.samples.len() as u32).to_be_bytes(), &data_offset.to_be_bytes(), &table]),
        ])
    }
}

fn video_run(v: &Video, offset: i64) -> Run {
    let mut duration = 3000; // 1/30 s, kept for the last frame
    let samples = v.samples.iter().enumerate().map(|(i, s)| {
        if let Some(next) = v.samples.get(i + 1) {
            duration = (next.dts - s.dts).max(0) as u32;
        }
        let flags = if s.key { 0x0200_0000 } else { 0x0101_0000 }; // sync / depends on others, non-sync
        [duration, s.data.len() as u32, flags, (s.pts - s.dts).max(0) as u32]
    });
    Run {
        track: VIDEO,
        base: (v.samples[0].dts + offset).max(0) as u64,
        fields: 0xf00,
        samples: samples.collect(),
        data: v.samples.iter().flat_map(|s| s.data.iter().copied()).collect(),
    }
}

fn audio_run(a: &Audio, offset: i64) -> Run {
    Run {
        track: AUDIO,
        base: ((a.samples[0].pts + offset).max(0) * i64::from(a.rate) / 90_000) as u64,
        fields: 0x300,
        samples: a.samples.iter().map(|s| [1024, s.data.len() as u32, 0, 0]).collect(),
        data: a.samples.iter().flat_map(|s| s.data.iter().copied()).collect(),
    }
}
