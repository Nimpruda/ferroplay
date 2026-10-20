use crate::state::{State, clamp};
use alloc::string::String;

/// What the UI should show. Derived from state, never stored.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub playing: bool,
    pub waiting: bool,
    pub muted: bool,
    pub progress: f64,
    pub buffered: f64,
    pub volume: f64,
    pub label: String,
}

impl State {
    pub fn view(&self) -> View {
        // Progress is relative to the window: the whole video, or the seekable range of a live stream.
        let (start, end) = self.window();
        // Clamped: the playhead or the buffered end can sit outside a live window that has slid on.
        let frac = |t: f64| if end > start && end.is_finite() { clamp((t - start) / (end - start), 0.0, 1.0) } else { 0.0 };
        let mut label = String::new();
        clock(self.time, &mut label);
        // A live stream has no total to show.
        if self.duration.is_finite() {
            label.push_str(" / ");
            clock(self.duration, &mut label);
        }
        View {
            playing: !self.paused,
            waiting: self.waiting,
            muted: self.muted,
            progress: frac(self.time),
            // The end of the range under the playhead: what can play from here.
            buffered: frac(self.buffered.end_at(self.time).unwrap_or(0.0)),
            volume: if self.muted { 0.0 } else { self.volume },
            label,
        }
    }
}

/// `m:ss` or `h:mm:ss`.
fn clock(seconds: f64, out: &mut String) {
    let s = seconds.max(0.0) as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        digits(h, 1, out);
        out.push(':');
    }
    digits(m, if h > 0 { 2 } else { 1 }, out);
    out.push(':');
    digits(s, 2, out);
}

/// `n` zero-padded to `width`. Hand-rolled to keep `core::fmt` out of the wasm binary.
fn digits(n: u64, width: u32, out: &mut String) {
    if n >= 10 || width > 1 {
        digits(n / 10, width.saturating_sub(1), out);
    }
    out.push((b'0' + (n % 10) as u8) as char);
}
