use ts2mp4::{Track, offset, read_init, shift};

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

/// ffmpeg's two 1 s media segments for the init above: styp, two sidx, then moof + mdat; v1 tfdt starting at 0
/// (video 0 then 15360, audio 0 then 43008).
const SEGMENT_0: &[u8] = include_bytes!("../../../fixtures/fmp4-0.m4s");
const SEGMENT_1: &[u8] = include_bytes!("../../../fixtures/fmp4-1.m4s");

fn init_tracks() -> Vec<Track> {
    read_init(INIT).unwrap().tracks
}

/// Each traf's (track_ID, base decode time), read independently of the crate: moof > traf > tfhd, tfdt.
fn times(segment: &[u8]) -> Vec<(u32, u64)> {
    fn boxes(data: &[u8]) -> Vec<(&[u8], &[u8])> {
        let (mut out, mut at) = (Vec::new(), 0);
        while at + 8 <= data.len() {
            let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            out.push((&data[at + 4..at + 8], &data[at + 8..at + size]));
            at += size;
        }
        out
    }
    let mut out = Vec::new();
    for (_, moof) in boxes(segment).into_iter().filter(|(kind, _)| kind == b"moof") {
        for (_, traf) in boxes(moof).into_iter().filter(|(kind, _)| kind == b"traf") {
            let inside = boxes(traf);
            let body = |name: &[u8]| inside.iter().find(|(kind, _)| *kind == name).unwrap().1;
            let track = u32::from_be_bytes(body(b"tfhd")[4..8].try_into().unwrap());
            let tfdt = body(b"tfdt");
            let time = if tfdt[0] == 1 { u64::from_be_bytes(tfdt[4..12].try_into().unwrap()) } else { u32::from_be_bytes(tfdt[4..8].try_into().unwrap()).into() };
            out.push((track, time));
        }
    }
    out
}

fn shifted(segment: &[u8], tracks: &[Track], offset: f64) -> Result<Vec<u8>, String> {
    let mut segment = segment.to_vec();
    shift(&mut segment, tracks, offset).map(|()| segment)
}

/// A bare media segment: one moof, one traf per (track_ID, tfdt version, time).
fn moof(trafs: &[(u32, u8, u64)]) -> Vec<u8> {
    let bx = |kind: &[u8], body: &[u8]| [&(body.len() as u32 + 8).to_be_bytes()[..], kind, body].concat();
    let traf = |&(track, version, time): &(u32, u8, u64)| {
        let time = if version == 1 { time.to_be_bytes().to_vec() } else { (time as u32).to_be_bytes().to_vec() };
        let tfhd = bx(b"tfhd", &[&[0, 0, 0, 0][..], &track.to_be_bytes()].concat());
        bx(b"traf", &[tfhd, bx(b"tfdt", &[&[version, 0, 0, 0][..], &time].concat())].concat())
    };
    bx(b"moof", &trafs.iter().flat_map(traf).collect::<Vec<_>>())
}

#[test]
fn the_first_segment_lands_at_its_playlist_time() {
    let tracks = init_tracks();
    let offset = offset(&tracks, SEGMENT_0, 100.0).unwrap();
    assert_eq!(offset, 100.0, "both tracks start at 0");
    let moved = shifted(SEGMENT_0, &tracks, offset).unwrap();
    assert_eq!(times(&moved), [(1, 100 * 15360), (2, 100 * 44100)]);
    // In place: same length, and only the two 8-byte times differ.
    assert_eq!(moved.len(), SEGMENT_0.len());
    assert!(moved.iter().zip(SEGMENT_0).filter(|(a, b)| a != b).count() <= 16);
}

#[test]
fn later_segments_keep_their_distance() {
    let tracks = init_tracks();
    let offset = offset(&tracks, SEGMENT_0, 100.0).unwrap();
    let moved = shifted(SEGMENT_1, &tracks, offset).unwrap();
    assert_eq!(times(&moved), [(1, 100 * 15360 + 15360), (2, 100 * 44100 + 43008)]);
}

#[test]
fn tracks_that_start_apart_stay_in_sync() {
    // Video at 1.0 s, audio at 1.1 s: the earliest lands at 10 s, the audio 0.1 s after it, not at 10 s too.
    let tracks = tracks(&[(1, 1000), (2, 44100)]);
    let segment = moof(&[(1, 1, 1000), (2, 1, 48510)]);
    let offset = offset(&tracks, &segment, 10.0).unwrap();
    assert_eq!(offset, 9.0);
    assert_eq!(times(&shifted(&segment, &tracks, offset).unwrap()), [(1, 10_000), (2, 445_410)]);
}

#[test]
fn the_offset_holds_for_an_init_section_with_other_timescales() {
    // A quality switch: the next init counts video in 2000ths. 2.0 s plus the same 9 s is 11 s, in its own units.
    let offset = offset(&tracks(&[(1, 1000)]), &moof(&[(1, 1, 1000)]), 10.0).unwrap();
    assert_eq!(times(&shifted(&moof(&[(1, 1, 4000)]), &tracks(&[(1, 2000)]), offset).unwrap()), [(1, 22_000)]);
}

#[test]
fn a_32_bit_decode_time_is_shifted_too_unless_it_no_longer_fits() {
    let tracks = tracks(&[(1, 1000)]);
    assert_eq!(times(&shifted(&moof(&[(1, 0, 1000)]), &tracks, 4.0).unwrap()), [(1, 5000)]);
    let err = shifted(&moof(&[(1, 0, u64::from(u32::MAX) - 10)]), &tracks, 4.0).unwrap_err();
    assert_eq!(err, "media segment: a decode time too large for a 32-bit tfdt");
}

#[test]
fn a_segment_from_before_the_offsets_start_is_an_error() {
    // An offset that puts the second segment at 0 would move the first one before 0: a stream that went back.
    let tracks = init_tracks();
    let offset = offset(&tracks, SEGMENT_1, 0.0).unwrap();
    assert_eq!(shifted(SEGMENT_0, &tracks, offset).unwrap_err(), "media segment: a decode time before the start of the stream");
}

#[test]
fn every_traf_needs_a_track_of_the_init_section() {
    let video_only = tracks(&[(1, 15360)]);
    let err = "media segment: a traf for a track the init section does not have";
    assert_eq!(shifted(SEGMENT_0, &video_only, 0.0).unwrap_err(), err);
    assert_eq!(offset(&video_only, SEGMENT_0, 0.0).unwrap_err(), err);
}

#[test]
fn a_segment_without_moof_is_an_error() {
    assert_eq!(shifted(INIT, &init_tracks(), 0.0).unwrap_err(), "media segment: no moof with a traf");
}

#[test]
fn truncated_segments_never_panic() {
    let tracks = init_tracks();
    for len in 0..SEGMENT_0.len() {
        let _ = offset(&tracks, &SEGMENT_0[..len], 100.0);
        let _ = shifted(&SEGMENT_0[..len], &tracks, 100.0);
    }
}
