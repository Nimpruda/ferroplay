//! MPEG-TS (H.264 + AAC, the usual HLS payload) to fragmented MP4, which Media Source plays.
//!
//! ```text
//! let media = ts2mp4::demux(&ts);                              // samples and codec setup
//! let init = ts2mp4::init(&media);                             // once per codec setup
//! let fragment = ts2mp4::fragment(&media, sequence, offset);   // per segment
//! ```

#![no_std]
extern crate alloc;

mod mp4;
mod ts;

use alloc::{string::String, vec::Vec};
pub use mp4::{fragment, init};
pub use ts::{Audio, Media, Sample, Video, demux};

/// One segment as a self-contained MP4 file (init + fragment, starting at 0), and its MIME type.
pub fn convert(ts: &[u8]) -> Option<(String, Vec<u8>)> {
    let media = demux(ts);
    let start = media.start()?;
    Some((media.mime(), [init(&media), fragment(&media, 1, -start)].concat()))
}
