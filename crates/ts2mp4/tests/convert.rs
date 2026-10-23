const SAMPLE: &[u8] = include_bytes!("../../../fixtures/sample.ts");

#[test]
fn demuxes_h264_and_aac() {
    let media = ts2mp4::demux(SAMPLE);
    let video = media.video.as_ref().unwrap();
    let audio = media.audio.as_ref().unwrap();
    assert_eq!((video.width, video.height), (160, 90));
    assert_eq!(video.samples.len(), 60, "2 s at 30 fps");
    assert!(video.samples[0].key);
    assert_eq!(video.samples.iter().filter(|s| s.key).count(), 2, "a keyframe every 30 frames");
    assert_eq!((audio.rate, audio.channels), (44100, 2));
    assert!((86..=88).contains(&audio.samples.len()), "~2 s of 1024-sample frames: {}", audio.samples.len());
    assert_eq!(media.mime(), "video/mp4; codecs=\"avc1.64000b,mp4a.40.2\"");
}

#[test]
fn the_setup_changes_with_what_the_init_segment_is_made_from() {
    let media = ts2mp4::demux(SAMPLE);
    assert_eq!(media.setup(), ts2mp4::demux(SAMPLE).setup());
    let mut moved = ts2mp4::demux(SAMPLE);
    // The same bytes, one moved from the SPS to the PPS: another init segment, so another setup.
    let video = moved.video.as_mut().unwrap();
    let last = video.sps.pop().unwrap();
    video.pps.insert(0, last);
    assert_ne!(ts2mp4::init(&moved), ts2mp4::init(&media));
    assert_ne!(moved.setup(), media.setup());
    let silent = ts2mp4::Media { audio: None, ..ts2mp4::demux(SAMPLE) };
    assert_ne!(silent.setup(), media.setup());
}

#[test]
fn writes_well_formed_fragmented_mp4() {
    let (_, mp4) = ts2mp4::convert(SAMPLE).unwrap();
    let mut boxes = Vec::new();
    let mut at = 0;
    while at < mp4.len() {
        let size = u32::from_be_bytes(mp4[at..at + 4].try_into().unwrap()) as usize;
        boxes.push(String::from_utf8_lossy(&mp4[at + 4..at + 8]).into_owned());
        at += size;
    }
    assert_eq!(at, mp4.len(), "box sizes add up");
    assert_eq!(boxes, ["ftyp", "moov", "moof", "mdat"]);
}

#[test]
fn a_track_moved_before_0_is_an_error_not_a_fragment_at_0() {
    let before = Err("a decode time before the start of the stream".to_string());
    let video = ts2mp4::Media { audio: None, ..ts2mp4::demux(SAMPLE) };
    let audio = ts2mp4::Media { video: None, ..ts2mp4::demux(SAMPLE) };
    for media in [&video, &audio] {
        let start = media.start().unwrap();
        assert!(ts2mp4::fragment(media, 1, -start).is_ok(), "at 0");
        assert_eq!(ts2mp4::fragment(media, 1, -start - 1), before, "one tick before 0");
    }
    // With both tracks, moving the later one to 0 would put the earlier one before it.
    let (video_start, audio_start) = (video.start().unwrap(), audio.start().unwrap());
    assert_ne!(video_start, audio_start, "the tracks start apart");
    assert_eq!(ts2mp4::fragment(&ts2mp4::demux(SAMPLE), 1, -video_start.max(audio_start)), before);
}

#[test]
fn ignores_garbage() {
    assert!(ts2mp4::convert(b"<html>not a video</html>").is_none());
    assert!(ts2mp4::convert(&[0x47; 188 * 3]).is_none());
}

/// `SAMPLE` with a stream entry of its PMT (type, PID, ES info) replaced by another, the section length to match.
fn sample_with(entry: &[u8], with: &[u8]) -> Vec<u8> {
    let pmt = [0x00, 0x02, 0xb0, 0x17]; // pointer field, table_id 2, section length 0x17
    SAMPLE
        .chunks(188)
        .flat_map(|p| {
            let Some(at) = p.windows(4).position(|w| w == pmt) else { return p.to_vec() };
            let i = p.windows(entry.len()).position(|w| w == entry).unwrap();
            let mut p = [&p[..i], with, &p[i + entry.len()..]].concat();
            p[at + 3] = (0x17 + with.len() - entry.len()) as u8;
            p.resize(188, 0xff);
            p
        })
        .collect()
}

const AAC: &[u8] = &[0x0f, 0xe1, 0x01, 0xf0, 0x00];

#[test]
fn names_the_audio_it_cannot_play_and_keeps_the_video() {
    for (with, name) in [
        (&[0x81, 0xe1, 0x01, 0xf0, 0x00][..], "AC-3"),
        (&[0x06, 0xe1, 0x01, 0xf0, 0x03, 0x6a, 0x01, 0x00], "AC-3"),
        (&[0x06, 0xe1, 0x01, 0xf0, 0x06, 0x05, 0x04, b'A', b'C', b'-', b'3'], "AC-3"),
        (&[0x87, 0xe1, 0x01, 0xf0, 0x00], "E-AC-3"),
        (&[0x06, 0xe1, 0x01, 0xf0, 0x03, 0x7a, 0x01, 0x00], "E-AC-3"),
        (&[0x03, 0xe1, 0x01, 0xf0, 0x00], "MPEG audio"),
        (&[0x04, 0xe1, 0x01, 0xf0, 0x00], "MPEG audio"),
    ] {
        let media = ts2mp4::demux(&sample_with(AAC, with));
        assert_eq!(media.skipped, [name], "{with:x?}");
        assert!(media.audio.is_none() && media.video.is_some(), "{name}");
        assert_eq!(media.mime(), "video/mp4; codecs=\"avc1.64000b\"");
    }
    assert!(ts2mp4::demux(SAMPLE).skipped.is_empty());
    let private = sample_with(AAC, &[0x06, 0xe1, 0x01, 0xf0, 0x00]);
    assert!(ts2mp4::demux(&private).skipped.is_empty(), "private data that says nothing of what it is");
}
