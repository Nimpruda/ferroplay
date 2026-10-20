use alloc::vec::Vec;

/// Time ranges (start, end) in seconds, as the media element reports them (`video.buffered`): in order, not
/// overlapping. The host sends them as they are; each reader asks what it needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ranges(pub Vec<(f64, f64)>);

impl Ranges {
    /// Ranges as the host sends them: (start, end) pairs of 8 little-endian bytes each (`buffered()` in
    /// `web/player.js`). None if the bytes are not whole pairs.
    pub fn from_le_bytes(data: &[u8]) -> Option<Self> {
        let (numbers, []) = data.as_chunks::<8>() else { return None };
        let (pairs, []) = numbers.as_chunks::<2>() else { return None };
        let mut out = Vec::new();
        for [start, end] in pairs {
            out.push((f64::from_le_bytes(*start), f64::from_le_bytes(*end)));
        }
        Some(Self(out))
    }

    /// The end of the range that `t` is in.
    pub fn end_at(&self, t: f64) -> Option<f64> {
        self.0.iter().find(|&&(start, end)| start <= t && t <= end).map(|&(_, end)| end)
    }

    /// The start of the first range after `t`.
    pub fn next_start(&self, t: f64) -> Option<f64> {
        self.0.iter().map(|&(start, _)| start).find(|&start| start > t)
    }
}
