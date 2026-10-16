//! HLS playback (VOD, MPEG-TS segments) as a player plugin.
//!
//! It takes over `Intent::Load` for `.m3u8` URLs. Then it asks the host to download playlists
//! and segments (`Cmd::Fetch`), converts each segment to fragmented MP4 (`ts2mp4`), and feeds
//! it to a Media Source (`Cmd::Open`, `Cmd::Append`). Quality follows the download speed.
//! Live playlists (no `ENDLIST`) are reloaded as they grow. Assumes segments aligned across qualities, no encryption.

#![no_std]
extern crate alloc;

pub mod playlist;

use alloc::{boxed::Box, collections::BTreeSet, string::String, vec::Vec};
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
    /// Which segments are in the Media Source, by sequence number (the same across variants).
    appended: BTreeSet<u64>,
    /// Whether the Media Source is open (after the first media playlist).
    opened: bool,
    /// Whether the last media playlist had no `EXT-X-ENDLIST`: more segments will come.
    live: bool,
    /// `EXT-X-TARGETDURATION` of the last media playlist, in seconds.
    target: f64,
    /// Seconds the download slot has been busy so far: waits and downloads, in order. The core has no
    /// clock, and downloads run one at a time, so this follows real time.
    elapsed: f64,
    /// `elapsed` when the last media playlist arrived.
    reloaded: f64,
    /// Whether the last reload brought no new segment: then it is due sooner.
    unchanged: bool,
    /// The wait of the reload in flight, while it waits.
    reload_delay: Option<f64>,
    /// Where a live stream starts (its hold-back): downloads go from here until the element reports a time.
    start_at: Option<f64>,
    pending: Option<Request>,
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
        segment.start + segment.duration > time && segment.start < time + ahead
    }

    /// Handles a finished download; false if it was unusable.
    fn receive(&mut self, request: Request, ms: f64, data: &[u8], out: &mut Vec<Cmd>) -> bool {
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
            Request::Playlist(v) => {
                let media = playlist::media(text, &self.variants[v].uri);
                let (live, target, mut segments) = (media.live(), media.target_duration, media.segments);
                if segments.is_empty() {
                    return false; // e.g. an error page: counts as a failed download
                }
                self.unchanged = self.variants[v].segments.last().map(|s| s.sequence) == segments.last().map(|s| s.sequence);
                // Times are on the timeline of the variants already known (they share sequence numbers).
                let known = self.variants.iter().find(|v| !v.segments.is_empty()).map_or(&[][..], |v| &v.segments);
                playlist::align(&mut segments, known);
                let end = segments.last().map_or(0.0, |s| s.start + s.duration);
                self.live = live;
                // The target in use: the playlist's, or the default when it has none. Every later use reads it from here.
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
            }
            Request::Segment(v, sequence) => {
                // Believe a slow download at once, a fast one gradually.
                let sample = data.len() as f64 * 8.0 / (ms.max(1.0) / 1000.0);
                self.bandwidth = if self.bandwidth == 0.0 || sample < self.bandwidth {
                    sample
                } else {
                    0.7 * self.bandwidth + 0.3 * sample
                };
                self.appended.insert(sequence); // even if unusable: skip a broken segment rather than retry it forever
                let media = ts2mp4::demux(data);
                let Some(start) = media.start() else { return true };
                let Some(segment) = self.variants[v].segments.iter().find(|s| s.sequence == sequence) else { return true };
                let offset = *self.offset.get_or_insert((segment.start * 90_000.0) as i64 - start);
                let init = ts2mp4::init(&media);
                if init != self.init {
                    out.push(Cmd::Codec(media.mime()));
                    out.push(Cmd::Append(init.clone()));
                    self.init = init;
                }
                out.push(Cmd::Append(ts2mp4::fragment(&media, sequence as u32 + 1, offset)));
            }
        }
        true
    }

    /// Decides the next download, if any.
    fn next(&mut self, time: f64, out: &mut Vec<Cmd>) {
        if self.variants.is_empty() {
            return self.fetch(Request::Master, self.url.clone(), out);
        }
        let v = self.pick();
        let variant = &self.variants[v];
        if variant.segments.is_empty() {
            let uri = variant.uri.clone();
            return self.fetch(Request::Playlist(v), uri, out);
        }
        let behind = self.behind();

        // Forget what is far behind the playhead, and have the host drop it.
        let mut drop_until = None;
        for s in &variant.segments {
            if self.appended.contains(&s.sequence) && s.start + s.duration < time - behind {
                self.appended.remove(&s.sequence);
                drop_until = Some(s.start + s.duration);
            }
        }
        out.extend(drop_until.map(Cmd::Remove));

        // Segments the buffer wants go first; a live playlist is reloaded when nothing else is left.
        if let Some(s) = self.needed(v, time) {
            let (sequence, uri) = (s.sequence, s.uri.clone());
            return self.fetch(Request::Segment(v, sequence), uri, out);
        }
        if self.live {
            self.reload(v, out);
        }
    }

    /// The first segment of variant `v` the buffer wants now: not loaded, and within reach of the playhead.
    fn needed(&self, v: usize, time: f64) -> Option<&Segment> {
        let ahead = self.ahead();
        (self.variants[v].segments.iter())
            .find(|s| !self.appended.contains(&s.sequence) && Self::reachable(s, time, ahead))
    }

    /// Asks for the playlist again, delayed until it is due: one target duration after the last reload, or
    /// half of it if the last reload brought nothing new.
    fn reload(&mut self, v: usize, out: &mut Vec<Cmd>) {
        let due = self.reloaded + if self.unchanged { self.target / 2.0 } else { self.target };
        let delay = (due - self.elapsed).max(0.0);
        self.elapsed += delay;
        self.reload_delay = Some(delay);
        let uri = self.variants[v].uri.clone();
        self.fetch_after(Request::Playlist(v), uri, delay, out);
    }

    /// Drops the download in flight if the playhead has moved past it, or a waiting reload if a segment is
    /// needed now (after a seek, say): the host keeps one download at a time, so the new one replaces it.
    /// A cancelled reload's wait is not known, so it is not counted: the next reload comes that much late.
    fn reconcile(&mut self, time: f64) {
        if self.reload_delay.is_some() && self.needed(self.pick(), time).is_some() {
            self.pending = None;
            self.elapsed -= self.reload_delay.take().unwrap_or(0.0);
        }
        if let Some(Request::Segment(v, sequence)) = self.pending
            && !self.wanted(v, sequence, time)
        {
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
        (self.variants[v].segments.iter())
            .find(|s| s.sequence == sequence)
            .is_some_and(|s| Self::reachable(s, time, ahead))
    }

    fn fetch(&mut self, request: Request, url: String, out: &mut Vec<Cmd>) {
        self.fetch_after(request, url, 0.0, out);
    }

    fn fetch_after(&mut self, request: Request, url: String, delay: f64, out: &mut Vec<Cmd>) {
        self.pending = Some(request);
        out.push(Cmd::Fetch(url, delay));
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
                self.elapsed += *ms / 1000.0; // the download slot: see `reload`
                if let Some(request) = self.pending.take() {
                    self.reload_delay = None;
                    let ok = self.receive(request, *ms, data, &mut cmds);
                    self.failures = if ok { 0 } else { self.failures + 1 };
                }
            }
            Event::Saw(Fact::LoadFailed) => {
                self.pending = None;
                self.reload_delay = None;
                self.failures += 1;
            }
            _ => {}
        }
        // Until the element reports its position, downloads go from the hold-back (set by a playlist just now).
        let time = self.start_at.unwrap_or(state.time);
        self.reconcile(time);
        if !self.url.is_empty() && self.pending.is_none() && (self.failures as f64) < self.retry_limit() {
            self.next(time, &mut cmds);
        }
        // The end: once the last segment is in, after the playlist says so, and again after any change
        // reopens a finished Media Source. Never while live: a live playlist is not finished.
        let ended_now = was_live && !self.live;
        let changed = cmds.iter().any(|c| matches!(c, Cmd::Append(_) | Cmd::Remove(_)));
        let last_loaded = (self.variants.iter()).any(|v| v.segments.last().is_some_and(|s| self.appended.contains(&s.sequence)));
        if last_loaded && !self.live && (changed || ended_now) {
            cmds.push(Cmd::End);
        }
        out.extend(cmds.into_iter().map(Intent::Cmd));
    }
}
