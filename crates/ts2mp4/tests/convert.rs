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
fn ignores_garbage() {
    assert!(ts2mp4::convert(b"<html>not a video</html>").is_none());
    assert!(ts2mp4::convert(&[0x47; 188 * 3]).is_none());
}
