//! Fragmented MP4 reader (ISO/IEC 14496-12): what an init section (`ftyp` + `moov`) holds, for Media Source and
//! for putting each track's fragments on the playlist's timeline.

use crate::codec;
use alloc::{string::String, vec::Vec};

/// An init section: the MIME type with codecs that Media Source needs before the first append, and its tracks.
pub struct Init {
    pub mime: String,
    pub tracks: Vec<Track>,
}

/// A track: its `track_ID` (what each fragment's `tfhd` names) and the units its timestamps count in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Track {
    pub id: u32,
    pub timescale: u32,
}

/// Reads an init section, or says why it cannot be played: a codec other than H.264 and AAC, or boxes that do not
/// add up.
pub fn read_init(data: &[u8]) -> Result<Init, String> {
    read(data).map_err(|why| ["init section: ", &why].concat())
}

fn read(data: &[u8]) -> Result<Init, String> {
    let moov = child(data, b"moov").ok_or("no moov box")?;
    let (mut tracks, mut codecs) = (Vec::new(), Vec::new());
    for (_, trak) in boxes(moov).filter(|(kind, _)| kind == b"trak") {
        let id = child(trak, b"tkhd").and_then(after_times).ok_or("a trak without a readable tkhd")?;
        let mdia = child(trak, b"mdia").ok_or("a trak without mdia")?;
        let timescale = child(mdia, b"mdhd").and_then(after_times).ok_or("a trak without a readable mdhd")?;
        let stsd = find(mdia, &[b"minf", b"stbl", b"stsd"]).ok_or("a trak without stsd")?;
        // The first sample entry, after the version, flags and entry count.
        let (kind, entry) = stsd.get(8..).and_then(|entries| boxes(entries).next()).ok_or("an empty stsd")?;
        let codec = match kind {
            b"avc1" | b"avc3" => {
                // The child boxes follow the 78 bytes of a VisualSampleEntry; avcC's bytes 1..4 are profile,
                // compatibility and level.
                entry.get(78..).and_then(|c| child(c, b"avcC")).and_then(|avcc| avcc.get(1..4)).map(codec::avc1)
            }
            b"mp4a" => aac_object(entry).map(codec::mp4a),
            _ => return Err([name(kind), " is a codec this player cannot play"].concat()),
        };
        codecs.push(codec.ok_or_else(|| ["an unreadable ", name(kind), " sample entry"].concat())?);
        tracks.push(Track { id, timescale });
    }
    if tracks.is_empty() {
        return Err("no tracks".into());
    }
    Ok(Init { mime: codec::mime(&codecs), tracks })
}

fn name(kind: &[u8]) -> &str {
    core::str::from_utf8(kind).unwrap_or("an unknown")
}

/// The boxes in `data`, as (type, body). Stops at the first one whose size does not fit, so truncated data
/// reads as missing boxes.
fn boxes(data: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut rest = data;
    core::iter::from_fn(move || {
        let (header, size) = match be32(rest, 0)? {
            0 => (8, rest.len()), // to the end
            1 => (16, usize::try_from(u64::from_be_bytes(rest.get(8..16)?.try_into().ok()?)).ok()?),
            size => (8, size as usize),
        };
        let (kind, body) = (rest.get(4..8)?, rest.get(header..size)?);
        rest = &rest[size..];
        Some((kind, body))
    })
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).find(|(k, _)| k == kind).map(|(_, body)| body)
}

/// The box at the end of `path`, one level down per step.
fn find<'a>(data: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    path.iter().try_fold(data, |data, kind| child(data, kind))
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// The field after a full box's creation and modification times (`tkhd`'s track_ID, `mdhd`'s timescale): they
/// take 32 bits each in version 0, 64 in version 1.
fn after_times(body: &[u8]) -> Option<u32> {
    be32(body, if *body.first()? == 1 { 20 } else { 12 })
}

/// The AAC audio object type of an `mp4a` sample entry, from its `esds`: ES_Descriptor (tag 3) >
/// DecoderConfigDescriptor (4, MPEG-4 audio) > DecoderSpecificInfo (5).
fn aac_object(entry: &[u8]) -> Option<u8> {
    // AudioSampleEntry: 28 bytes, 16 or 36 more in QuickTime's sound versions 1 and 2.
    let extra = match entry.get(8..10)? {
        [0, 0] => 0,
        [0, 1] => 16,
        [0, 2] => 36,
        _ => return None,
    };
    let esds = child(entry.get(28 + extra..)?, b"esds")?;
    let es = descriptor(esds.get(4..)?, 3)?; // after the full box's version and flags
    // ES_ID, then flags saying which optional fields follow.
    let flags = *es.get(2)?;
    let mut at = 3;
    if flags & 0x80 != 0 {
        at += 2; // dependsOn_ES_ID
    }
    if flags & 0x40 != 0 {
        at += 1 + usize::from(*es.get(at)?); // URL
    }
    if flags & 0x20 != 0 {
        at += 2; // OCR_ES_ID
    }
    let config = descriptor(es.get(at..)?, 4)?;
    if config.first() != Some(&0x40) {
        return None; // not MPEG-4 audio (MP3 is 0x6b)
    }
    // DecoderSpecificInfo follows the type, stream type, buffer size and bitrates; for AAC it is an
    // AudioSpecificConfig.
    codec::aac_object(descriptor(config.get(13..)?, 5)?)
}

/// The body of the descriptor at the start of `data`, if it has this tag. Its size takes 1 to 4 bytes of 7 bits.
fn descriptor(data: &[u8], tag: u8) -> Option<&[u8]> {
    if *data.first()? != tag {
        return None;
    }
    let (mut size, mut at) = (0, 1);
    loop {
        let b = *data.get(at)?;
        at += 1;
        size = size << 7 | usize::from(b & 0x7f);
        if b & 0x80 == 0 || at == 5 {
            break;
        }
    }
    data.get(at..at + size)
}
