//! Built-in plugins. Each one is a few lines; write your own the same way.

use crate::{Event, Fact, Intent, Plugin, State};
use alloc::vec::Vec;

/// YouTube-style shortcuts. Arrow keys arrive as `← ↑ → ↓`.
pub struct Keyboard {
    pub step: f64,
}

impl Plugin for Keyboard {
    fn on(&mut self, state: &State, event: &Event, out: &mut Vec<Intent>) {
        let &Event::Key(key) = event else { return };
        out.push(match key {
            ' ' | 'k' => Intent::Toggle,
            'j' => Intent::SeekBy(-10.0),
            'l' => Intent::SeekBy(10.0),
            '←' => Intent::SeekBy(-self.step),
            '→' => Intent::SeekBy(self.step),
            '↑' => Intent::VolumeBy(0.1),
            '↓' => Intent::VolumeBy(-0.1),
            'm' => Intent::ToggleMute,
            'f' => Intent::ToggleFullscreen,
            '<' => Intent::Rate(state.rate - 0.25),
            '>' => Intent::Rate(state.rate + 0.25),
            '0'..='9' => Intent::SeekFrac((key as u32 - '0' as u32) as f64 / 10.0),
            _ => return,
        });
    }
}

/// Start over when the video ends.
pub struct Loop;

impl Plugin for Loop {
    fn on(&mut self, _: &State, event: &Event, out: &mut Vec<Intent>) {
        if *event == Event::Saw(Fact::Ended) {
            out.extend([Intent::Seek(0.0), Intent::Play]);
        }
    }
}

/// Repeat a section: `a` marks the start, `b` marks the end and starts looping, `b` again stops.
#[derive(Default)]
pub struct AbLoop {
    a: Option<f64>,
    b: Option<f64>,
}

impl Plugin for AbLoop {
    fn on(&mut self, state: &State, event: &Event, out: &mut Vec<Intent>) {
        match (event, self.a, self.b) {
            (Event::Key('a'), _, _) => (self.a, self.b) = (Some(state.time), None),
            (Event::Key('b'), Some(_), Some(_)) => self.b = None,
            (Event::Key('b'), Some(a), None) if state.time > a => {
                self.b = Some(state.time);
                out.push(Intent::Seek(a));
            }
            (&Event::Saw(Fact::Time(t) | Fact::Seeking(t)), Some(a), Some(b)) if t >= b => out.push(Intent::Seek(a)),
            _ => {}
        }
    }
}

/// Jumps a gap in the buffer that the media element would wait at forever, as hls.js's gap controller does.
/// Until playback has moved since the last load or seek, a gap of up to `start` seconds: media that starts a
/// little late (B-frames, say). After that, only a hole of up to `hole` seconds: a larger one is data still to
/// come, for the source plugin to fetch.
pub struct Gaps {
    pub start: f64,
    pub hole: f64,
}

impl Default for Gaps {
    fn default() -> Self {
        Self { start: 2.0, hole: 0.1 }
    }
}

impl Plugin for Gaps {
    fn on(&mut self, state: &State, event: &Event, out: &mut Vec<Intent>) {
        // Stuck: waiting to play, and asked again when new data comes in.
        if !matches!(event, Event::Saw(Fact::Waiting | Fact::Buffered(_))) || !state.waiting || state.paused {
            return;
        }
        let t = state.time;
        let Some((next, end)) = state.buffered.next_after(t) else { return };
        // Nothing, or next to nothing, left to play here: buffered media under the playhead is never skipped.
        let starved = state.buffered.end_at(t).is_none_or(|end| end - t <= self.hole);
        let most = if t > state.seeked { self.hole } else { self.start };
        if !starved || next - t > most {
            return;
        }
        // A little into the range (hls.js's skipBufferHolePadding), so the element is clear of the gap. The seek
        // updates `time` at once, so the same gap is not jumped twice.
        out.push(Intent::Seek((next + 0.1).min(end)));
    }
}

/// Jump to a timestamp once the duration is known (e.g. resume, skip intro). Not on a live stream: its
/// times are not a position in a known whole, and the stream picks its own start.
pub struct StartAt(Option<f64>);

impl StartAt {
    pub fn new(time: f64) -> Self {
        Self(Some(time))
    }
}

impl Plugin for StartAt {
    fn on(&mut self, state: &State, event: &Event, out: &mut Vec<Intent>) {
        if let Event::Saw(Fact::Duration(_)) = event
            && state.duration > 0.0
            && state.duration.is_finite()
            && let Some(time) = self.0.take()
        {
            out.push(Intent::Seek(time));
        }
    }
}
