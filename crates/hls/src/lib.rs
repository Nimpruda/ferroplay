//! HLS playback (VOD and live, MPEG-TS or fMP4 segments) as a player plugin.
//!
//! It takes over `Intent::Load` for `.m3u8` URLs. Then it asks the host to download playlists
//! and segments (`Cmd::Fetch`), converts MPEG-TS segments to fragmented MP4 (`ts2mp4`) or moves fMP4
//! ones onto the playlist's timeline, and feeds them to a Media Source (`Cmd::Open`, `Cmd::Append`).
//! Quality follows the download speed.
//! Live playlists (no `ENDLIST`) are reloaded as they grow. Assumes segments aligned across qualities, no encryption.

#![no_std]
extern crate alloc;

pub mod playlist;

use alloc::{boxed::Box, string::String, vec::Vec};
use player::{Cmd, Event, Fact, Intent, Plugin, State};
use playlist::{Map, Segment, Variant};
use ts2mp4::Track;

/// Seconds of media to keep loaded ahead of the playhead, and behind it.
const AHEAD: f64 = 30.0;
const BEHIND: f64 = 30.0;
/// Pick qualities using at most this share of the measured bandwidth.
const SAFETY: f64 = 0.8;
/// Retries after a failure, before giving up: on a stream that ends, a live one, and after an unplayable init section.
const RETRIES: f64 = 3.0;
const LIVE_RETRIES: f64 = 10.0;
const INIT_RETRIES: f64 = 0.0;
/// `EXT-X-TARGETDURATION` when a playlist has none.
const DEFAULT_TARGET: f64 = 6.0;
/// Seconds before a skipped segment's start that still count as its hole: the media before it may run out a
/// little early (an audio track shorter than the video, a rounded `EXTINF`), and the element waits there.
const EDGE: f64 = 0.5;

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
    /// Retries after a failed download, or a playlist or segment that cannot be used, before giving up on a
    /// stream that ends (default 3).
    Retries,
    /// The same for a live stream (default 10): its reloads come every target duration, so a few failures in a
    /// row are a short outage, not the end.
    LiveRetries,
    /// Retries after an init section this player cannot play (another codec, a byte-range `EXT-X-MAP`) before
    /// giving up (default 0: at once, as every segment after it needs it).
    InitRetries,
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
            Knob::InitRetries => "initRetries",
        }
    }

    /// The target duration the knob depends on, or 0 for the knobs that depend on nothing.
    pub fn target(self) -> f64 {
        match self {
            Knob::Ahead(target) | Knob::Behind(target) | Knob::Holdback(target) => target,
            Knob::Safety | Knob::Retries | Knob::LiveRetries | Knob::InitRetries => 0.0,
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
    /// The segments in the Media Source: sequence number (the same across variants), end time, and the start
    /// time of one that could not be used. Kept apart from the playlists, because a live window can slide past a
    /// segment that is still buffered. A segment that could not be used is in too, so it is not fetched again;
    /// nothing is buffered for it, and the gaps plugin leaves holes that large alone (`past_skipped`).
    appended: Vec<(u64, f64, Option<f64>)>,
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
    retry: Retry,
    /// Download speed, bits per second; 0 until the first segment.
    bandwidth: f64,
    /// Seconds that move the stream's own timestamps onto the playlist's timeline, from its first segment.
    offset: Option<f64>,
    /// The last init segment appended; a new one is needed when the quality changes.
    init: Vec<u8>,
    /// The `EXT-X-MAP` of that init segment and its tracks, when it came from the stream (fMP4).
    map: Option<(Map, Vec<Track>)>,
}

#[derive(Clone, Copy)]
enum Request {
    Master,
    Playlist(usize),
    /// A live variant's playlist again, after a wait of so many seconds.
    Reload(usize, f64),
    /// A variant and the sequence number of its segment.
    Segment(usize, u64),
    /// The init section (`EXT-X-MAP`) of that segment, before it.
    Init(usize, u64),
}

/// Why a download could not be used. Its kind decides how many in a row a stream survives (`Hls::retries`).
enum Failure {
    /// The download failed (network, HTTP status); the host has said so already.
    Download(String),
    /// A playlist or segment that cannot be used; the next one may be fine.
    Unusable(String),
    /// An init section this player cannot play: the segments that need it are lost too.
    Unplayable(String),
}

/// Failures in a row, and whether the stream has given up on them. With `Failure`, what a give-up policy shared
/// with other streaming plugins can take over: the plugin only says how many retries a failure gets.
#[derive(Default)]
struct Retry {
    failures: u32,
    gave_up: bool,
}

impl Retry {
    /// Counts a failure; true if the stream gives up on it, being one more than `retries` in a row. Nothing is
    /// counted after that, so it is said once.
    fn fail(&mut self, retries: f64) -> bool {
        if self.gave_up {
            return false;
        }
        self.failures += 1;
        self.gave_up = self.failures as f64 > retries;
        self.gave_up
    }

    fn succeed(&mut self) {
        self.failures = 0;
    }
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

    /// The retries a stream gets after a failure of this kind, from the knobs: the one place they are read for it.
    fn retries(&self, failure: &Failure) -> f64 {
        match failure {
            Failure::Unplayable(_) => self.knob(Knob::InitRetries, INIT_RETRIES),
            _ if self.live => self.knob(Knob::LiveRetries, LIVE_RETRIES),
            _ => self.knob(Knob::Retries, RETRIES),
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
        self.appended.iter().any(|&(s, ..)| s == sequence)
    }

    /// Handles a finished download; why it cannot be used, if it cannot.
    fn receive(&mut self, request: Request, ms: f64, data: &[u8], out: &mut Vec<Cmd>) -> Result<(), Failure> {
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
                    return Err(Failure::Unusable([&self.fetching, " lists no segments: not a media playlist"].concat()));
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
                let (start, end, fmp4) = (segment.start, segment.end(), segment.map.is_some());
                let fragment = if fmp4 { self.fmp4_fragment(data, start) } else { self.ts_fragment(data, start, sequence, out) };
                // Even if unusable: skip a broken segment rather than retry it forever.
                self.appended.push((sequence, end, fragment.is_err().then_some(start)));
                let fragment = fragment.map_err(|why| Failure::Unusable(["segment ", &self.fetching, " skipped: ", &why].concat()))?;
                out.push(Cmd::Append(fragment));
            }
            Request::Init(v, sequence) => {
                let Some(map) = self.variants[v].segment(sequence).and_then(|s| s.map.clone()) else { return Ok(()) };
                let init = ts2mp4::read_init(data).map_err(|why| Failure::Unplayable([&self.fetching, ": ", &why].concat()))?;
                self.use_init(init.mime, data, out);
                self.map = Some((map, init.tracks));
            }
        }
        Ok(())
    }

    /// An MPEG-TS segment as an fMP4 fragment on the playlist's timeline, after the init segment for it if that
    /// changed.
    fn ts_fragment(&mut self, data: &[u8], start: f64, sequence: u64, out: &mut Vec<Cmd>) -> Result<Vec<u8>, String> {
        let media = ts2mp4::demux(data);
        let pts = media.start().ok_or("no MPEG-TS media this player can play")?;
        let offset = *self.offset.get_or_insert(start - pts as f64 / 90_000.0);
        self.use_init(media.mime(), &ts2mp4::init(&media), out);
        self.map = None;
        // The fragment number only has to grow; it wraps rather than overflow on huge sequence numbers.
        Ok(ts2mp4::fragment(&media, (sequence as u32).wrapping_add(1), (offset * 90_000.0) as i64))
    }

    /// An fMP4 segment as it is, moved onto the playlist's timeline. Its init section is in already: `next`
    /// fetches it first.
    fn fmp4_fragment(&mut self, data: &[u8], start: f64) -> Result<Vec<u8>, String> {
        let tracks = self.map.as_ref().map_or(&[][..], |(_, tracks)| tracks);
        let mut data = data.to_vec();
        let offset = match self.offset {
            Some(offset) => offset,
            None => ts2mp4::offset(tracks, &data, start)?,
        };
        ts2mp4::shift(&mut data, tracks, offset)?;
        self.offset = Some(offset);
        Ok(data)
    }

    /// Appends an init segment, with its MIME type, unless it is the one in the Media Source already.
    fn use_init(&mut self, mime: String, init: &[u8], out: &mut Vec<Cmd>) {
        if init != self.init {
            out.push(Cmd::Codec(mime));
            out.push(Cmd::Append(init.to_vec()));
            self.init = init.to_vec();
        }
    }

    /// Says what failed (not a failed download: the host has said so already) and counts it, giving up when
    /// there are more in a row than its kind's retries. Messages are concatenated, not `format!`ted: `core::fmt`
    /// adds 4 KB to the wasm.
    fn failed(&mut self, failure: Failure, out: &mut Vec<Cmd>) {
        let (Failure::Download(reason) | Failure::Unusable(reason) | Failure::Unplayable(reason)) = &failure;
        if !matches!(failure, Failure::Download(_)) {
            out.push(Cmd::Warn(reason.clone()));
        }
        if self.retry.fail(self.retries(&failure)) {
            out.push(Cmd::Fail(["HLS: giving up: ", reason].concat()));
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
        if let Some(until) = self.appended.iter().map(|&(_, end, _)| end).filter(|&end| end < behind).reduce(f64::max) {
            self.appended.retain(|&(_, end, _)| end >= behind);
            out.push(Cmd::Remove(until));
        }

        // Segments the buffer wants go first; a live playlist is reloaded when nothing else is left.
        if let Some(s) = self.needed(v, time) {
            let (sequence, uri) = (s.sequence, s.uri.clone());
            // An fMP4 segment needs its init section in the Media Source first.
            let Some(map) = s.map.as_ref().filter(|&map| self.map.as_ref().map(|(m, _)| m) != Some(map)).cloned() else {
                return self.fetch(Request::Segment(v, sequence), uri, 0.0, out);
            };
            if map.range.is_some() {
                let reason = [&map.uri, " is a byte range (EXT-X-MAP BYTERANGE), which this player cannot fetch yet"].concat();
                return self.failed(Failure::Unplayable(reason), out);
            }
            return self.fetch(Request::Init(v, sequence), map.uri, 0.0, out);
        }
        if self.live {
            self.reload(v, out);
        }
    }

    /// Where to move a playhead at `time` that waits in the hole a skipped segment left: that segment's end. Only
    /// while the element waits to play: paused or playing, it is not stuck. By the playlist's times, not by what
    /// is buffered: the element reports that only after an append, so it may not show a segment that is in.
    fn past_skipped(&self, state: &State, time: f64) -> Option<f64> {
        if !state.waiting || state.paused {
            return None;
        }
        let mut holes = self.appended.iter().filter_map(|&(_, end, hole)| Some((hole?, end)));
        holes.find(|&(start, end)| start - EDGE <= time && time < end).map(|(_, end)| end)
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
            Some(Request::Segment(v, sequence) | Request::Init(v, sequence)) => !self.wanted(v, sequence, time),
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
        if let Event::Saw(Fact::Time(t) | Fact::Seeking(t)) = event && *t > 0.0 {
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
                        Ok(()) => self.retry.succeed(),
                        Err(failure) => self.failed(failure, &mut cmds),
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
                    self.failed(Failure::Download(["the download of ", &self.fetching, " failed"].concat()), &mut cmds);
                }
            }
            _ => {}
        }
        // Until the element reports its position, downloads go from the hold-back (set by a playlist just now).
        let mut time = self.start_at.unwrap_or(state.time);
        if let Some(end) = self.past_skipped(state, time) {
            // An intent, not a `Cmd::Seek`: `State` moves its time, so the hole is not jumped again.
            out.push(Intent::Seek(end));
            (self.start_at, time) = (None, end);
        }
        self.reconcile(time);
        if !self.url.is_empty() && self.pending.is_none() && !self.retry.gave_up {
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
