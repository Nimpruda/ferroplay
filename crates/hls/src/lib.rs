//! HLS playback (VOD, MPEG-TS segments) as a player plugin.
//!
//! It takes over `Intent::Load` for `.m3u8` URLs. Then it asks the host to download playlists
//! and segments (`Cmd::Fetch`), converts each segment to fragmented MP4 (`ts2mp4`), and feeds
//! it to a Media Source (`Cmd::Open`, `Cmd::Append`). Quality follows the download speed.
//! Live playlists (no `ENDLIST`) are reloaded as they grow. Assumes segments aligned across qualities, no encryption.

#![no_std]
extern crate alloc;

pub mod playlist;

use alloc::{boxed::Box, string::String, vec::Vec};
use player::{Cmd, Event, Fact, Intent, Plugin, State};
use playlist::{Segment, Variant};

/// Seconds of media to keep loaded ahead of the playhead, and behind it.
const AHEAD: f64 = 30.0;
const BEHIND: f64 = 30.0;
/// Pick qualities using at most this share of the measured bandwidth.
const SAFETY: f64 = 0.8;
/// Consecutive failed downloads before giving up.
const RETRIES: f64 = 3.0;
/// Consecutive failed downloads before giving up on a live stream.
const LIVE_RETRIES: f64 = 10.0;
/// `EXT-X-TARGETDURATION` when a playlist has none.
const DEFAULT_TARGET: f64 = 6.0;

/// A value the plugin can be tuned with. Where it depends on the target duration, that comes in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Knob {
    /// Seconds to load ahead of the playhead (default 30).
    Ahead(f64),
    /// Seconds to keep behind the playhead (default 30).
    Behind(f64),
    /// Seconds from the end of a live playlist to start at (default: three target durations).
    Holdback(f64),
    /// The share of the measured bandwidth a quality may use (default 0.8).
    Safety,
    /// Consecutive failed downloads before giving up on a stream that ends (default 3).
    Retries,
    /// Consecutive failed downloads before giving up on a live stream (default 10): its reloads come every
    /// target duration, so a few failures in a row are a short outage, not the end.
    LiveRetries,
}

impl Knob {
    /// The name the host knows it by (`hlsWith({ ahead: … })` in `web/plugins/hls.js`).
    pub fn name(self) -> &'static str {
        match self {
            Knob::Ahead(_) => "ahead",
            Knob::Behind(_) => "behind",
            Knob::Holdback(_) => "holdback",
            Knob::Safety => "safety",
            Knob::Retries => "retries",
            Knob::LiveRetries => "liveRetries",
        }
    }

    /// The target duration the knob depends on, or 0 for the knobs that depend on nothing.
    pub fn target(self) -> f64 {
        match self {
            Knob::Ahead(target) | Knob::Behind(target) | Knob::Holdback(target) => target,
            Knob::Safety | Knob::Retries | Knob::LiveRetries => 0.0,
        }
    }
}

/// Answers for a knob, or `None` to keep the default.
type Tuner = Box<dyn Fn(Knob) -> Option<f64>>;

#[derive(Default)]
pub struct Hls {
    /// Tuning values from the caller; empty keeps every default.
    tune: Option<Tuner>,
    /// The master playlist; empty when not playing HLS.
    url: String,
    variants: Vec<Variant>,
    /// The variant whose media playlist arrived last. Its list is the current timeline: only the variant
    /// being played is reloaded, so the others' lists go stale.
    current: usize,
    /// The segments in the Media Source: sequence number (the same across variants) and end time. Kept
    /// apart from the playlists, because a live window can slide past a segment that is still buffered.
    appended: Vec<(u64, f64)>,
    /// Whether the Media Source is open (after the first media playlist).
    opened: bool,
    /// Whether the last media playlist had no `EXT-X-ENDLIST`: more segments will come.
    live: bool,
    /// The target duration in use, in seconds: the last media playlist's, or the default when it has none.
    target: f64,
    /// Seconds the download slot has been busy so far: waits and downloads, counted as they finish. The
    /// core has no clock, and downloads run one at a time, so this follows real time. A wait that is
    /// cancelled is not counted, so the next reload comes that much late.
    elapsed: f64,
    /// `elapsed` when the last media playlist arrived.
    reloaded: f64,
    /// Whether the last reload brought no new segment (or failed): then the next one is due sooner.
    unchanged: bool,
    /// Where a live stream starts (its hold-back): downloads go from here until the element reports a time.
    start_at: Option<f64>,
    pending: Option<Request>,
    /// The URL of the download in flight, or of the last one.
    fetching: String,
    failures: u32,
    /// Download speed, bits per second; 0 until the first segment.
    bandwidth: f64,
    /// Shifts 90 kHz media timestamps onto the playlist's timeline.
    offset: Option<i64>,
    /// The last init segment appended; a new one is needed when the quality changes.
    init: Vec<u8>,
}

#[derive(Clone, Copy)]
enum Request {
    Master,
    Playlist(usize),
    /// A live variant's playlist again, after a wait of so many seconds.
    Reload(usize, f64),
    /// A variant and the sequence number of its segment.
    Segment(usize, u64),
}

impl Hls {
    /// A plugin tuned by `tune`: for each knob it asks, `None` keeps the default.
    pub fn tuned(tune: impl Fn(Knob) -> Option<f64> + 'static) -> Self {
        Self { tune: Some(Box::new(tune)), ..Self::default() }
    }

    /// The value of a knob: the caller's, or `default` if the caller gives nothing. Nonsense is not hidden:
    /// it panics, naming the knob.
    fn knob(&self, knob: Knob, default: f64) -> f64 {
        let Some(value) = self.tune.as_ref().and_then(|tune| tune(knob)) else { return default };
        assert!(value.is_finite() && value >= 0.0, "tuning {knob:?} is {value}, not a finite, non-negative number");
        value
    }

    /// Consecutive failures before giving up: the live stream's limit while live, the stream's otherwise.
    fn retry_limit(&self) -> f64 {
        if self.live {
            self.knob(Knob::LiveRetries, LIVE_RETRIES)
        } else {
            self.knob(Knob::Retries, RETRIES)
        }
    }

    /// Seconds to load ahead of the playhead.
    fn ahead(&self) -> f64 {
        self.knob(Knob::Ahead(self.target), AHEAD)
    }

    /// Seconds to keep behind the playhead.
    fn behind(&self) -> f64 {
        self.knob(Knob::Behind(self.target), BEHIND)
    }

    /// Whether a segment is on screen or within reach of the playhead at `time`, given the seconds to load ahead.
    fn reachable(segment: &Segment, time: f64, ahead: f64) -> bool {
        segment.end() > time && segment.start < time + ahead
    }

    /// Whether the segment with this sequence number is in the Media Source.
    fn loaded(&self, sequence: u64) -> bool {
        self.appended.iter().any(|&(s, _)| s == sequence)
    }

    /// Handles a finished download; why it is unusable, if it is.
    fn receive(&mut self, request: Request, ms: f64, data: &[u8], out: &mut Vec<Cmd>) -> Result<(), String> {
        let text = core::str::from_utf8(data).unwrap_or_default();
        match request {
            Request::Master => {
                self.variants = playlist::variants(text, &self.url);
                if self.variants.is_empty() {
                    // Not a master playlist: the URL is the one and only media playlist.
                    self.variants.push(Variant { bandwidth: 0, uri: self.url.clone(), segments: Vec::new() });
                    return self.receive(Request::Playlist(0), ms, data, out);
                }
            }
            Request::Playlist(v) | Request::Reload(v, _) => {
                let media = playlist::media(text, &self.variants[v].uri);
                let (live, target, mut segments) = (media.live(), media.target_duration, media.segments);
                if segments.is_empty() {
                    // e.g. an error page: counts as a failed download
                    return Err([&self.fetching, " lists no segments: not a media playlist"].concat());
                }
                self.unchanged = self.variants[v].segments.last().map(|s| s.sequence) == segments.last().map(|s| s.sequence);
                // Times follow the playlist that arrived last, of whichever variant: they share sequence numbers.
                let known = self.variants.get(self.current).map_or(&[][..], |v| &v.segments);
                playlist::align(&mut segments, known);
                let end = segments.last().map_or(0.0, Segment::end);
                self.live = live;
                self.target = if target > 0.0 { target } else { DEFAULT_TARGET };
                self.reloaded = self.elapsed;
                let first = !self.opened;
                if first {
                    // The first media playlist sets the timeline. A live one has no end yet.
                    self.opened = true;
                    out.push(Cmd::Open(if self.live { f64::INFINITY } else { end }));
                }
                if self.live {
                    out.push(Cmd::SeekableRange(segments[0].start, end));
                }
                if first && self.live {
                    // HLS hold-back: start three target durations from the end, not at the start of the window.
                    let holdback = (end - self.knob(Knob::Holdback(self.target), 3.0 * self.target)).max(segments[0].start);
                    if holdback > 0.0 {
                        self.start_at = Some(holdback);
                        out.push(Cmd::Seek(holdback));
                    }
                }
                self.variants[v].segments = segments;
                self.current = v;
            }
            Request::Segment(v, sequence) => {
                // Believe a slow download at once, a fast one gradually.
                let sample = data.len() as f64 * 8.0 / (ms.max(1.0) / 1000.0);
                self.bandwidth = if self.bandwidth == 0.0 || sample < self.bandwidth {
                    sample
                } else {
                    0.7 * self.bandwidth + 0.3 * sample
                };
                let Some(segment) = self.variants[v].segment(sequence) else { return Ok(()) };
                let (start, end) = (segment.start, segment.end());
                self.appended.push((sequence, end)); // even if unusable: skip a broken segment rather than retry it forever
                let media = ts2mp4::demux(data);
                let Some(pts) = media.start() else {
                    return Err(["segment ", &self.fetching, " holds no MPEG-TS media this player can play"].concat());
                };
                let offset = *self.offset.get_or_insert((start * 90_000.0) as i64 - pts);
                let init = ts2mp4::init(&media);
                if init != self.init {
                    out.push(Cmd::Codec(media.mime()));
                    out.push(Cmd::Append(init.clone()));
                    self.init = init;
                }
                // The fragment number only has to grow; it wraps rather than overflow on huge sequence numbers.
                out.push(Cmd::Append(ts2mp4::fragment(&media, (sequence as u32).wrapping_add(1), offset)));
            }
        }
        Ok(())
    }

    /// Counts a failed download, and gives up once there are too many in a row, saying why: once, as nothing is
    /// counted past the limit. Messages are concatenated, not `format!`ted: `core::fmt` adds 4 KB to the wasm.
    fn failed(&mut self, reason: String, out: &mut Vec<Cmd>) {
        let limit = self.retry_limit();
        if (self.failures as f64) < limit {
            self.failures += 1;
            if self.failures as f64 >= limit {
                out.push(Cmd::Fail(["HLS: giving up after too many failures in a row; the last: ", &reason].concat()));
            }
        }
    }

    /// Decides the next download, if any.
    fn next(&mut self, time: f64, out: &mut Vec<Cmd>) {
        if self.variants.is_empty() {
            return self.fetch(Request::Master, self.url.clone(), 0.0, out);
        }
        let v = self.pick();
        if self.variants[v].segments.is_empty() {
            let uri = self.variants[v].uri.clone();
            return self.fetch(Request::Playlist(v), uri, 0.0, out);
        }

        // Forget what is far behind the playhead, and have the host drop it. By time, not by playlist:
        // a live window slides past segments before they are far behind.
        let behind = time - self.behind();
        if let Some(until) = self.appended.iter().map(|&(_, end)| end).filter(|&end| end < behind).reduce(f64::max) {
            self.appended.retain(|&(_, end)| end >= behind);
            out.push(Cmd::Remove(until));
        }

        // Segments the buffer wants go first; a live playlist is reloaded when nothing else is left.
        if let Some(s) = self.needed(v, time) {
            let (sequence, uri) = (s.sequence, s.uri.clone());
            return self.fetch(Request::Segment(v, sequence), uri, 0.0, out);
        }
        if self.live {
            self.reload(v, out);
        }
    }

    /// The first segment of variant `v` the buffer wants now: not loaded, and within reach of the playhead.
    fn needed(&self, v: usize, time: f64) -> Option<&Segment> {
        let ahead = self.ahead();
        (self.variants[v].segments.iter()).find(|s| !self.loaded(s.sequence) && Self::reachable(s, time, ahead))
    }

    /// Asks for the playlist again, delayed until it is due: one target duration after the last one arrived,
    /// or half of it if that one brought nothing new.
    fn reload(&mut self, v: usize, out: &mut Vec<Cmd>) {
        let due = self.reloaded + if self.unchanged { self.target / 2.0 } else { self.target };
        let delay = (due - self.elapsed).max(0.0);
        let uri = self.variants[v].uri.clone();
        self.fetch(Request::Reload(v, delay), uri, delay, out);
    }

    /// Drops the download in flight if the playhead has moved past it, or a waiting reload if a segment is
    /// needed now (after a seek, say): the host keeps one download at a time, so the new one replaces it.
    fn reconcile(&mut self, time: f64) {
        let drop = match self.pending {
            Some(Request::Reload(..)) => self.needed(self.pick(), time).is_some(),
            Some(Request::Segment(v, sequence)) => !self.wanted(v, sequence, time),
            _ => false,
        };
        if drop {
            self.pending = None;
        }
    }

    /// The best quality the bandwidth allows; the first listed until something is measured.
    fn pick(&self) -> usize {
        if self.bandwidth == 0.0 {
            return 0;
        }
        let variants = self.variants.iter().enumerate();
        let safety = self.knob(Knob::Safety, SAFETY);
        (variants.clone().filter(|(_, v)| v.bandwidth as f64 <= self.bandwidth * safety))
            .max_by_key(|(_, v)| v.bandwidth)
            .or_else(|| variants.min_by_key(|(_, v)| v.bandwidth))
            .map_or(0, |(i, _)| i)
    }

    /// Whether segment `sequence` of variant `v` is still worth loading with the playhead at `time`.
    fn wanted(&self, v: usize, sequence: u64, time: f64) -> bool {
        let ahead = self.ahead();
        self.variants[v].segment(sequence).is_some_and(|s| Self::reachable(s, time, ahead))
    }

    fn fetch(&mut self, request: Request, url: String, delay: f64, out: &mut Vec<Cmd>) {
        self.pending = Some(request);
        self.fetching = url.clone();
        out.push(Cmd::Fetch(url, delay));
    }

    /// Takes the request in flight and counts the time its wait took (see `elapsed`).
    fn finish(&mut self) -> Option<Request> {
        let request = self.pending.take()?;
        if let Request::Reload(_, delay) = request {
            self.elapsed += delay;
        }
        Some(request)
    }
}

impl Plugin for Hls {
    fn on(&mut self, state: &State, event: &Event, out: &mut Vec<Intent>) {
        let mut cmds = Vec::new();
        let was_live = self.live;
        // The element reports 0 until it has seeked to the hold-back: any other time means it has.
        if let Event::Saw(Fact::Time(t)) = event && *t > 0.0 {
            self.start_at = None;
        }
        match event {
            Event::Do(Intent::Load(url)) => {
                // Start over, keeping only what was learned about the network.
                *self = Self { bandwidth: self.bandwidth, tune: self.tune.take(), ..Self::default() };
                if playlist::is_playlist(url) {
                    self.url = url.clone();
                    out.retain(|intent| !matches!(intent, Intent::Load(_))); // ours: not a `video.src`
                }
            }
            Event::Saw(Fact::Loaded { ms, data }) => {
                if let Some(request) = self.finish() {
                    self.elapsed += *ms / 1000.0;
                    match self.receive(request, *ms, data, &mut cmds) {
                        Ok(()) => self.failures = 0,
                        Err(reason) => {
                            cmds.push(Cmd::Warn(reason.clone()));
                            self.failed(reason, &mut cmds);
                        }
                    }
                }
            }
            Event::Saw(Fact::LoadFailed) => {
                // A failed reload is tried again after half a target duration, like one that brought nothing
                // new: retrying at once would use up the live limit in moments.
                if let Some(request) = self.finish() {
                    if let Request::Reload(..) = request {
                        (self.reloaded, self.unchanged) = (self.elapsed, true);
                    }
                    self.failed(["the download of ", &self.fetching, " failed"].concat(), &mut cmds);
                }
            }
            _ => {}
        }
        // Until the element reports its position, downloads go from the hold-back (set by a playlist just now).
        let time = self.start_at.unwrap_or(state.time);
        self.reconcile(time);
        if !self.url.is_empty() && self.pending.is_none() && (self.failures as f64) < self.retry_limit() {
            self.next(time, &mut cmds);
        }
        // The end: once the last segment of the playlist being played is in, after the playlist says so, and
        // again after any change reopens a finished Media Source. Never while live: a live playlist is not finished.
        let ended_now = was_live && !self.live;
        let changed = cmds.iter().any(|c| matches!(c, Cmd::Append(_) | Cmd::Remove(_)));
        let last = self.variants.get(self.current).and_then(|v| v.segments.last());
        if last.is_some_and(|s| self.loaded(s.sequence)) && !self.live && (changed || ended_now) {
            cmds.push(Cmd::End);
        }
        out.extend(cmds.into_iter().map(Intent::Cmd));
    }
}
