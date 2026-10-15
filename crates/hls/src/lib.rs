//! HLS playback (VOD, MPEG-TS segments) as a player plugin.
//!
//! It takes over `Intent::Load` for `.m3u8` URLs. Then it asks the host to download playlists
//! and segments (`Cmd::Fetch`), converts each segment to fragmented MP4 (`ts2mp4`), and feeds
//! it to a Media Source (`Cmd::Open`, `Cmd::Append`). Quality follows the download speed.
//! Assumes what VOD streams normally do: segments aligned across qualities, no encryption.

#![no_std]
extern crate alloc;

pub mod playlist;

use alloc::{string::String, vec, vec::Vec};
use player::{Cmd, Event, Fact, Intent, Plugin, State};
use playlist::Variant;

/// Seconds of media to keep loaded ahead of the playhead, and behind it.
const AHEAD: f64 = 30.0;
const BEHIND: f64 = 30.0;
/// Pick qualities using at most this share of the measured bandwidth.
const SAFETY: f64 = 0.8;
/// Consecutive failed downloads before giving up.
const RETRIES: u32 = 3;

#[derive(Default)]
pub struct Hls {
    /// The master playlist; empty when not playing HLS.
    url: String,
    variants: Vec<Variant>,
    /// Which segments are in the Media Source, by index (the same across variants).
    appended: Vec<bool>,
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
    Segment(usize, usize),
}

impl Hls {
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
                let segments = playlist::segments(text, &self.variants[v].uri);
                if segments.is_empty() {
                    return false; // e.g. an error page: counts as a failed download
                }
                if self.appended.is_empty() {
                    // The first media playlist sets the timeline.
                    self.appended = vec![false; segments.len()];
                    out.push(Cmd::Open(segments.last().map_or(0.0, |s| s.start + s.duration)));
                }
                self.variants[v].segments = segments;
            }
            Request::Segment(v, i) => {
                // Believe a slow download at once, a fast one gradually.
                let sample = data.len() as f64 * 8.0 / (ms.max(1.0) / 1000.0);
                self.bandwidth = if self.bandwidth == 0.0 || sample < self.bandwidth {
                    sample
                } else {
                    0.7 * self.bandwidth + 0.3 * sample
                };
                self.appended[i] = true; // even if unusable: skip a broken segment rather than retry it forever
                let media = ts2mp4::demux(data);
                let Some(start) = media.start() else { return true };
                let offset = *self.offset.get_or_insert((self.variants[v].segments[i].start * 90_000.0) as i64 - start);
                let init = ts2mp4::init(&media);
                if init != self.init {
                    out.push(Cmd::Codec(media.mime()));
                    out.push(Cmd::Append(init.clone()));
                    self.init = init;
                }
                out.push(Cmd::Append(ts2mp4::fragment(&media, i as u32 + 1, offset)));
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

        // Forget what is far behind the playhead, and have the host drop it.
        let mut drop_until = None;
        for (s, appended) in variant.segments.iter().zip(&mut self.appended) {
            if *appended && s.start + s.duration < time - BEHIND {
                *appended = false;
                drop_until = Some(s.start + s.duration);
            }
        }
        out.extend(drop_until.map(Cmd::Remove));

        // Load the first missing segment from the playhead on, once it is close enough.
        let missing = (variant.segments.iter().enumerate())
            .skip_while(|(_, s)| s.start + s.duration <= time)
            .find(|(i, _)| self.appended.get(*i) == Some(&false));
        if let Some((i, s)) = missing
            && s.start < time + AHEAD
        {
            let uri = s.uri.clone();
            self.fetch(Request::Segment(v, i), uri, out);
        }
    }

    /// The best quality the bandwidth allows; the first listed until something is measured.
    fn pick(&self) -> usize {
        if self.bandwidth == 0.0 {
            return 0;
        }
        let variants = self.variants.iter().enumerate();
        (variants.clone().filter(|(_, v)| v.bandwidth as f64 <= self.bandwidth * SAFETY))
            .max_by_key(|(_, v)| v.bandwidth)
            .or_else(|| variants.min_by_key(|(_, v)| v.bandwidth))
            .map_or(0, |(i, _)| i)
    }

    /// Whether segment `i` of variant `v` is still worth loading with the playhead at `time`.
    fn wanted(&self, v: usize, i: usize, time: f64) -> bool {
        let s = &self.variants[v].segments[i];
        s.start + s.duration > time && s.start < time + AHEAD
    }

    fn fetch(&mut self, request: Request, url: String, out: &mut Vec<Cmd>) {
        self.pending = Some(request);
        out.push(Cmd::Fetch(url));
    }
}

impl Plugin for Hls {
    fn on(&mut self, state: &State, event: &Event, out: &mut Vec<Intent>) {
        let mut cmds = Vec::new();
        match event {
            Event::Do(Intent::Load(url)) => {
                // Start over, keeping only what was learned about the network.
                *self = Self { bandwidth: self.bandwidth, ..Self::default() };
                if playlist::is_playlist(url) {
                    self.url = url.clone();
                    out.retain(|intent| !matches!(intent, Intent::Load(_))); // ours: not a `video.src`
                }
            }
            Event::Saw(Fact::Loaded { ms, data }) => {
                if let Some(request) = self.pending.take() {
                    let ok = self.receive(request, *ms, data, &mut cmds);
                    self.failures = if ok { 0 } else { self.failures + 1 };
                }
            }
            Event::Saw(Fact::LoadFailed) => {
                self.pending = None;
                self.failures += 1;
            }
            _ => {}
        }
        // After a seek, the segment being downloaded may be useless: asking for another one
        // replaces it (the host keeps one download at a time).
        if let Some(Request::Segment(v, i)) = self.pending
            && !self.wanted(v, i, state.time)
        {
            self.pending = None;
        }
        if !self.url.is_empty() && self.pending.is_none() && self.failures < RETRIES {
            self.next(state.time, &mut cmds);
        }
        // Any change reopens a finished Media Source: end it again while the last segment is in.
        if self.appended.last() == Some(&true) && cmds.iter().any(|c| matches!(c, Cmd::Append(_) | Cmd::Remove(_))) {
            cmds.push(Cmd::End);
        }
        out.extend(cmds.into_iter().map(Intent::Cmd));
    }
}
