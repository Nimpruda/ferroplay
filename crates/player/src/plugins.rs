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
            (&Event::Saw(Fact::Time(t)), Some(a), Some(b)) if t >= b => out.push(Intent::Seek(a)),
            _ => {}
        }
    }
}

/// Jump to a timestamp once the duration is known (e.g. resume, skip intro).
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
            && let Some(time) = self.0.take()
        {
            out.push(Intent::Seek(time));
        }
    }
}
