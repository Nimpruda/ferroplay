//! Codec strings for Media Source (RFC 6381), shared by the MPEG-TS and fMP4 paths so they agree.

use alloc::string::String;

/// `avc1.PPCCLL` from the profile, compatibility and level bytes (an SPS's bytes 1..4, or an `avcC`'s).
pub fn avc1(profile: &[u8]) -> String {
    let mut c = String::from("avc1.");
    for &b in profile {
        for n in [b >> 4, b & 15] {
            c.push(char::from(b"0123456789abcdef"[usize::from(n)]));
        }
    }
    c
}

/// The audio object type of an AudioSpecificConfig: its first 5 bits, or 32 plus the next 6 when those are 31.
pub fn aac_object(config: &[u8]) -> Option<u8> {
    match config.first()? >> 3 {
        31 => Some(32 + ((config[0] & 7) << 3 | config.get(1)? >> 5)),
        object => Some(object),
    }
}

/// `mp4a.40.N` for AAC of audio object type `object`.
pub fn mp4a(object: u8) -> String {
    let mut c = String::from("mp4a.40.");
    if object >= 10 {
        c.push(char::from(b'0' + object / 10));
    }
    c.push(char::from(b'0' + object % 10));
    c
}

/// `video/mp4; codecs="…"`, or `audio/mp4` when there is no video.
pub fn mime(codecs: &[String]) -> String {
    let kind = if codecs.iter().any(|c| c.starts_with("avc1")) { "video" } else { "audio" };
    [kind, "/mp4; codecs=\"", &codecs.join(","), "\""].concat()
}
