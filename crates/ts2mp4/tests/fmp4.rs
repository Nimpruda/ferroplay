use ts2mp4::{Track, read_init};

/// ffmpeg's init section for fixtures/sample.ts (`-f hls -hls_segment_type fmp4`).
const INIT: &[u8] = include_bytes!("../../../fixtures/fmp4-init.mp4");
const SAMPLE: &[u8] = include_bytes!("../../../fixtures/sample.ts");

fn tracks(list: &[(u32, u32)]) -> Vec<Track> {
    list.iter().map(|&(id, timescale)| Track { id, timescale }).collect()
}

#[test]
fn reads_an_ffmpeg_init_section() {
    let init = read_init(INIT).unwrap();
    assert_eq!(init.mime, "video/mp4; codecs=\"avc1.64000b,mp4a.40.2\"");
    assert_eq!(init.tracks, tracks(&[(1, 15360), (2, 44100)]));
}

#[test]
fn reads_what_the_converter_writes_as_the_converter_names_it() {
    let media = ts2mp4::demux(SAMPLE);
    let init = read_init(&ts2mp4::init(&media)).unwrap();
    assert_eq!(init.mime, media.mime());
    assert_eq!(init.tracks, tracks(&[(1, 90_000), (2, 44100)]));
}

#[test]
fn audio_only_is_audio_mp4() {
    let mut media = ts2mp4::demux(SAMPLE);
    media.video = None;
    assert_eq!(read_init(&ts2mp4::init(&media)).unwrap().mime, "audio/mp4; codecs=\"mp4a.40.2\"");
}

#[test]
fn reads_an_escaped_audio_object_type() {
    // Object types from 32 up: 31 in the first 5 bits, then 6 more. 0xf9 0x40 is 31, then 10: type 42 (USAC).
    let mut media = ts2mp4::demux(SAMPLE);
    media.audio.as_mut().unwrap().config = [0xf9, 0x40];
    assert!(media.mime().ends_with(",mp4a.40.42\""), "{}", media.mime());
    assert_eq!(read_init(&ts2mp4::init(&media)).unwrap().mime, media.mime(), "both paths name it the same");
}

#[test]
fn truncated_data_is_an_error_not_a_panic() {
    for len in 0..INIT.len() {
        assert!(read_init(&INIT[..len]).is_err(), "cut at {len}");
    }
}

#[test]
fn a_codec_it_cannot_play_is_named() {
    // The video sample entry is the last "avc1" before its avcC (the ftyp may list avc1 as a brand too).
    let avcc = INIT.windows(4).position(|w| w == b"avcC").unwrap();
    let entry = INIT[..avcc].windows(4).rposition(|w| w == b"avc1").unwrap();
    let mut hevc = INIT.to_vec();
    hevc[entry..entry + 4].copy_from_slice(b"hvc1");
    assert_eq!(read_init(&hevc).err().unwrap(), "init section: hvc1 is a codec this player cannot play");
}

#[test]
fn no_moov_is_an_error() {
    assert_eq!(read_init(b"\0\0\0\x08free").err().unwrap(), "init section: no moov box");
}
