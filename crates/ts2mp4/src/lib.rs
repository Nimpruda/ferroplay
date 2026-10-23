//! MPEG-TS (H.264 + AAC, the usual HLS payload) to fragmented MP4, which Media Source plays.
//!
//! ```text
//! let media = ts2mp4::demux(&ts);                              // samples and codec setup
//! let init = ts2mp4::init(&media);                             // once per codec setup (media.setup())
//! let fragment = ts2mp4::fragment(&media, sequence, offset)?;  // per segment
//! ```
//!
//! Timestamps wrap every 26.5 h; `demux_near(&ts, Some(near))` unwraps a segment's to the turn nearest `near`.
//!
//! Segments that are fMP4 already only need reading and moving onto the playlist's timeline:
//!
//! ```text
//! let init = ts2mp4::read_init(&init_section)?;                // codecs and tracks
//! let offset = ts2mp4::offset(&init.tracks, &first, start)?;    // seconds, once, from the first segment
//! ts2mp4::shift(&mut segment, &init.tracks, offset)?;           // per segment, with its own init's tracks
//! ```

#![no_std]
extern crate alloc;

mod codec;
mod fmp4;
mod mp4;
mod ts;

use alloc::{string::String, vec::Vec};
pub use fmp4::{Init, Track, offset, read_init, shift};
pub use mp4::{fragment, init};
pub use ts::{Audio, Media, Sample, Video, demux, demux_near, sniff};

/// Why `fragment` and `shift` refuse a segment that would start before 0.
const BEFORE_START: &str = "a decode time before the start of the stream";

/// One segment as a self-contained MP4 file (init + fragment, starting at 0), and its MIME type.
pub fn convert(ts: &[u8]) -> Option<(String, Vec<u8>)> {
    let media = demux(ts);
    let start = media.start()?;
    Some((media.mime(), [init(&media), fragment(&media, 1, -start).ok()?].concat()))
}
