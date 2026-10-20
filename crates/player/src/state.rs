use crate::Ranges;
use alloc::{string::String, vec::Vec};

/// Something the host reported.
#[derive(Clone, Debug, PartialEq)]
pub enum Fact {
    Time(f64),
    /// The media element started a seek to this time, whoever asked for it.
    Seeking(f64),
    Duration(f64),
    /// Every buffered range of the media element.
    Buffered(Ranges),
    Playing,
    Paused,
    Ended,
    Waiting,
    Volume(f64),
    Muted(bool),
    Rate(f64),
    Fullscreen(bool),
    /// A `Cmd::Fetch` finished, after `ms` milliseconds.
    Loaded { ms: f64, data: Vec<u8> },
    LoadFailed,
    /// The media element's seekable range (start, end), as the host reads it.
    Seekable(f64, f64),
}

/// Something a user or plugin wants, relative to the current state.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    Toggle,
    Play,
    Pause,
    Seek(f64),
    SeekBy(f64),
    SeekFrac(f64),
    Volume(f64),
    VolumeBy(f64),
    ToggleMute,
    ToggleFullscreen,
    Rate(f64),
    /// Make this URL the source.
    Load(String),
    /// An exact command, passed through as is.
    Cmd(Cmd),
}

/// An absolute instruction for the host.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Play,
    Pause,
    Seek(f64),
    Volume(f64),
    Muted(bool),
    Rate(f64),
    Fullscreen(bool),
    /// Set the media element's source (if it is not that already).
    Source(String),
    /// Download a URL after `delay` seconds (0: now); the host answers with `Fact::Loaded` or `Fact::LoadFailed`.
    /// A newer `Fetch` cancels a delay that has not run out yet.
    Fetch(String, f64),
    /// Play from a Media Source of this duration, fed by the commands below.
    Open(f64),
    /// MIME type with codecs for the following appends.
    Codec(String),
    Append(Vec<u8>),
    /// Drop everything buffered before this time.
    Remove(f64),
    /// No more data: let playback reach the end.
    End,
    /// The live seekable range (start, end) for `setLiveSeekableRange`.
    SeekableRange(f64, f64),
    /// Something is wrong, but playback goes on: say so.
    Warn(String),
    /// Playback cannot go on: say why, where the viewer can see it.
    Fail(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub time: f64,
    pub duration: f64,
    pub buffered: Ranges,
    pub volume: f64,
    pub rate: f64,
    pub paused: bool,
    pub waiting: bool,
    pub muted: bool,
    pub fullscreen: bool,
    /// The seekable range the host last reported, if any.
    pub seekable: Option<(f64, f64)>,
    /// Where the last seek or load left the playhead: playback has moved once `time` is past it.
    pub seeked: f64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            time: 0.0,
            duration: 0.0,
            buffered: Ranges::default(),
            volume: 1.0,
            rate: 1.0,
            paused: true,
            waiting: false,
            muted: false,
            fullscreen: false,
            seekable: None,
            seeked: 0.0,
        }
    }
}

impl State {
    /// Forgets what the last source said about its media, and keeps what belongs to the element: the new source
    /// has not reported anything yet, and the element may never say the old values are gone (Firefox fires no
    /// `durationchange` for a live Media Source that has no data yet).
    pub(crate) fn reset_media(&mut self) {
        let Self { volume, rate, paused, waiting, muted, fullscreen, .. } = *self;
        *self = Self { volume, rate, paused, waiting, muted, fullscreen, ..Self::default() };
    }

    pub fn apply(&mut self, fact: &Fact) {
        match *fact {
            Fact::Time(t) => self.time = t,
            Fact::Seeking(t) => (self.time, self.seeked) = (t, t),
            // An infinite duration is a live stream: keep it, and seek within the seekable window instead.
            Fact::Duration(d) => self.duration = if d.is_nan() { 0.0 } else { d },
            Fact::Buffered(ref ranges) => self.buffered = ranges.clone(),
            Fact::Playing => (self.paused, self.waiting) = (false, false),
            Fact::Paused | Fact::Ended => (self.paused, self.waiting) = (true, false),
            Fact::Waiting => self.waiting = true,
            Fact::Volume(v) => self.volume = v,
            Fact::Muted(m) => self.muted = m,
            Fact::Rate(r) => self.rate = r,
            Fact::Fullscreen(f) => self.fullscreen = f,
            Fact::Seekable(start, end) => self.seekable = Some((start, end)),
            Fact::Loaded { .. } | Fact::LoadFailed => {}
        }
    }

    pub fn resolve(&mut self, intent: Intent, out: &mut Vec<Cmd>) {
        match intent {
            Intent::Toggle if self.paused => out.push(Cmd::Play),
            Intent::Toggle | Intent::Pause => out.push(Cmd::Pause),
            Intent::Play => out.push(Cmd::Play),
            Intent::Seek(t) => self.seek(t, out),
            Intent::SeekBy(dt) => self.seek(self.time + dt, out),
            Intent::SeekFrac(f) => {
                // Maps onto the window; with no known end (live, before the window is reported) there is nothing to map.
                let (start, end) = self.window();
                if end.is_finite() {
                    self.seek(start + f * (end - start), out);
                }
            }
            Intent::Volume(v) => {
                let v = percent(v);
                out.extend([Cmd::Volume(v), Cmd::Muted(v == 0.0)]);
            }
            Intent::VolumeBy(dv) => self.resolve(Intent::Volume(self.volume + dv), out),
            Intent::ToggleMute => {
                if self.muted && self.volume == 0.0 {
                    out.push(Cmd::Volume(1.0));
                }
                out.push(Cmd::Muted(!self.muted));
            }
            Intent::ToggleFullscreen => out.push(Cmd::Fullscreen(!self.fullscreen)),
            Intent::Rate(r) => out.push(Cmd::Rate(clamp(r, 0.25, 4.0))),
            Intent::Load(url) => out.push(Cmd::Source(url)),
            Intent::Cmd(cmd) => out.push(cmd),
        }
    }

    /// The times a seek may go to. A finite duration is all of it. An infinite one (live) is the
    /// seekable range the host reported, or everything from 0 until it reports one.
    pub fn window(&self) -> (f64, f64) {
        if self.duration.is_finite() {
            (0.0, self.duration)
        } else {
            self.seekable.unwrap_or((0.0, f64::INFINITY))
        }
    }

    // Updates time optimistically so repeated seeks stack up before the media reports back.
    fn seek(&mut self, t: f64, out: &mut Vec<Cmd>) {
        let (start, end) = self.window();
        self.time = clamp(t, start, end);
        out.push(Cmd::Seek(self.time));
    }
}

// Unlike `f64::clamp`, never panics, so no float formatting lands in the binary.
pub(crate) fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    x.max(lo).min(hi)
}

// Clamps to 0..=1 and snaps to whole percents, so ten steps of -0.1 land exactly on 0.
fn percent(v: f64) -> f64 {
    (clamp(v, 0.0, 1.0) * 100.0 + 0.5) as u32 as f64 / 100.0
}
