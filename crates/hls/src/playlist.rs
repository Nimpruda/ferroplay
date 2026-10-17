//! M3U8 playlists: just enough for VOD and live (variants, segments, durations, sequence numbers, init sections).

use alloc::{string::String, vec::Vec};

pub struct Variant {
    pub bandwidth: u64,
    pub uri: String,
    /// Empty until this variant's media playlist is loaded.
    pub segments: Vec<Segment>,
}

pub struct Segment {
    /// Seconds from the first segment of this playlist (live windows slide, so use `sequence` to match).
    pub start: f64,
    pub duration: f64,
    pub uri: String,
    /// `EXT-X-MEDIA-SEQUENCE` plus the index in the playlist. Stable across reloads of a live playlist.
    pub sequence: u64,
    /// The init section to append before this segment (fMP4): the last `EXT-X-MAP` above it.
    pub map: Option<Map>,
}

/// An `EXT-X-MAP`: where a segment's init section is.
#[derive(Clone, Debug, PartialEq)]
pub struct Map {
    pub uri: String,
    /// `BYTERANGE`, when the init section is a slice of the file.
    pub range: Option<ByteRange>,
}

/// A slice of a file, in bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ByteRange {
    pub length: u64,
    pub offset: u64,
}

impl Segment {
    pub fn end(&self) -> f64 {
        self.start + self.duration
    }
}

impl Variant {
    /// The segment with this sequence number, if the playlist still lists it.
    pub fn segment(&self, sequence: u64) -> Option<&Segment> {
        self.segments.iter().find(|s| s.sequence == sequence)
    }
}

/// A media playlist and the tags around its segments.
pub struct Media {
    /// `EXT-X-TARGETDURATION` in seconds (0 if absent).
    pub target_duration: f64,
    /// `EXT-X-MEDIA-SEQUENCE`: the sequence number of the first segment (0 if absent).
    pub media_sequence: u64,
    /// Whether `EXT-X-ENDLIST` is there. Without it the playlist is live, or an event that still grows.
    pub ended: bool,
    pub segments: Vec<Segment>,
}

impl Media {
    /// Live means more segments will come: reload the playlist to find them.
    pub fn live(&self) -> bool {
        !self.ended
    }
}

/// Whether a URL points at a playlist (`.m3u8`, before any query).
pub fn is_playlist(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url).as_bytes();
    path.len() >= 5 && path[path.len() - 5..].eq_ignore_ascii_case(b".m3u8")
}

/// The variants of a master playlist (none if `text` is a media playlist).
pub fn variants(text: &str, base: &str) -> Vec<Variant> {
    let mut out = Vec::new();
    let mut bandwidth = None;
    for line in text.lines().map(str::trim) {
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            bandwidth = Some(attr(attrs, "BANDWIDTH").and_then(|v| v.parse().ok()).unwrap_or(0));
        } else if is_uri(line)
            && let Some(bandwidth) = bandwidth.take()
        {
            out.push(Variant { bandwidth, uri: join(base, line), segments: Vec::new() });
        }
    }
    out
}

/// A media playlist: its segments, numbered, and whether it has ended.
pub fn media(text: &str, base: &str) -> Media {
    let mut out = Media { target_duration: 0.0, media_sequence: 0, ended: false, segments: Vec::new() };
    let (mut start, mut duration, mut map) = (0.0, None, None);
    for line in text.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            out.target_duration = decimal(value);
        } else if let Some(value) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            out.media_sequence = value.parse().unwrap_or(0);
        } else if let Some(attrs) = line.strip_prefix("#EXT-X-MAP:") {
            map = init_section(attrs, base);
        } else if line == "#EXT-X-ENDLIST" {
            out.ended = true;
        } else if let Some(info) = line.strip_prefix("#EXTINF:") {
            duration = Some(decimal(info.split(',').next().unwrap_or("")));
        } else if is_uri(line)
            && let Some(duration) = duration.take()
        {
            // The spec puts EXT-X-MEDIA-SEQUENCE before the first segment, so it is known here.
            let sequence = out.media_sequence + out.segments.len() as u64;
            out.segments.push(Segment { start, duration, uri: join(base, line), sequence, map: map.clone() });
            start += duration;
        }
    }
    out
}

/// An `EXT-X-MAP` from its attributes. None without a URI, or with a `BYTERANGE` that does not parse: then the
/// segments after it get no init section.
fn init_section(attrs: &str, base: &str) -> Option<Map> {
    let uri = join(base, unquote(attr(attrs, "URI")?));
    let range = match attr(attrs, "BYTERANGE") {
        None => None,
        // The offset is 0 when absent: nothing comes before an init section.
        Some(range) => {
            let (length, offset) = byte_range(unquote(range))?;
            Some(ByteRange { length, offset: offset.unwrap_or(0) })
        }
    };
    Some(Map { uri, range })
}

/// `length[@offset]`, as in `EXT-X-BYTERANGE` and a MAP's `BYTERANGE`. What a missing offset means depends on
/// the tag, so it is left to the caller.
fn byte_range(text: &str) -> Option<(u64, Option<u64>)> {
    let (length, offset) = text.split_once('@').map_or((text, None), |(length, offset)| (length, Some(offset)));
    Some((length.parse().ok()?, offset.map(str::parse).transpose().ok()?))
}

/// Moves a freshly parsed playlist onto the timeline of `previous` (the last one parsed for the same stream).
/// A sliding window keeps its start times by sequence number. With no segment in common, it follows on
/// from the end of `previous`.
pub fn align(segments: &mut [Segment], previous: &[Segment]) {
    let Some(first) = segments.first() else { return };
    let shared = segments.iter().find_map(|new| {
        let old = previous.iter().find(|old| old.sequence == new.sequence)?;
        Some(old.start - new.start)
    });
    let shift = shared.unwrap_or_else(|| previous.last().map_or(0.0, Segment::end) - first.start);
    for segment in segments {
        segment.start += shift;
    }
}

/// The segments of a media playlist, with their start times.
pub fn segments(text: &str, base: &str) -> Vec<Segment> {
    media(text, base).segments
}

/// Replaces every URI in a playlist (URI lines and `URI="…"` attributes) with `map(resolved URI)`.
pub fn rewrite(text: &str, base: &str, map: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    for line in text.lines() {
        if is_uri(line.trim()) {
            out += &map(&join(base, line.trim()));
        } else if let Some((head, rest)) = line.split_once("URI=\"")
            && let Some((uri, tail)) = rest.split_once('"')
        {
            out += &[head, "URI=\"", &map(&join(base, uri)), "\"", tail].concat();
        } else {
            out += line;
        }
        out.push('\n');
    }
    out
}

/// Resolves a playlist URI against the playlist's own (absolute) URL.
pub fn join(base: &str, uri: &str) -> String {
    if has_scheme(uri) {
        return uri.into();
    }
    let base = base.split(['?', '#']).next().unwrap_or(base);
    let host = base.find("://").map_or(0, |i| i + 3);
    let cut = if uri.starts_with("//") {
        host.saturating_sub(2) // keep "https:"
    } else if uri.starts_with('/') {
        base.get(host..).and_then(|rest| rest.find('/')).map_or(base.len(), |i| host + i) // keep "https://host"
    } else {
        base.rfind('/').map_or(0, |i| i + 1) // keep the directory
    };
    [base.get(..cut).unwrap_or(base), uri].concat()
}

fn has_scheme(uri: &str) -> bool {
    uri.split_once("://").is_some_and(|(scheme, _)| !scheme.is_empty() && scheme.bytes().all(|b| b.is_ascii_alphabetic()))
}

fn is_uri(line: &str) -> bool {
    !line.is_empty() && !line.starts_with('#')
}

/// Value of `key` in an attribute list like `BANDWIDTH=1280000,CODECS="avc1.4d401f,mp4a.40.2"`.
fn attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let mut quoted = false;
    attrs
        .split(|c| {
            quoted ^= c == '"';
            c == ',' && !quoted
        })
        .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
}

/// A quoted-string attribute value without its quotes.
fn unquote(value: &str) -> &str {
    value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value)
}

/// Parses `10.000`. Hand-rolled: the float parser in `core` would double the wasm size.
fn decimal(s: &str) -> f64 {
    let (mut value, mut scale) = (0.0, 0.0);
    for c in s.trim().bytes() {
        match c {
            b'0'..=b'9' if scale == 0.0 => value = value * 10.0 + f64::from(c - b'0'),
            b'0'..=b'9' => {
                value += f64::from(c - b'0') * scale;
                scale /= 10.0;
            }
            b'.' => scale = 0.1,
            _ => break,
        }
    }
    value
}
