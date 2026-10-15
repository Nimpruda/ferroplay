//! Converts one MPEG-TS segment to fragmented MP4, as the HLS plugin does in the browser.
//! Usage: cargo run -p ts2mp4 < segment.ts > segment.mp4

use std::io::{Read, Write};

fn main() {
    let mut ts = Vec::new();
    std::io::stdin().read_to_end(&mut ts).unwrap();
    let (mime, mp4) = ts2mp4::convert(&ts).expect("no H.264 or AAC stream found");
    eprintln!("{mime}");
    std::io::stdout().write_all(&mp4).unwrap();
}
