use hls::playlist::{ByteRange, Map, align, is_playlist, join, media, rewrite, segments, variants};

const MASTER: &str = "#EXTM3U
#EXT-X-STREAM-INF:PROGRAM-ID=1,AVERAGE-BANDWIDTH=900,BANDWIDTH=2149280,CODECS=\"mp4a.40.2,avc1.64001f\",RESOLUTION=1280x720
url_0/hd.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=246440,CODECS=\"mp4a.40.5,avc1.42000d\"
/abs/ld.m3u8
";

#[test]
fn master_playlist() {
    let v = variants(MASTER, "https://cdn.test/x/master.m3u8?token=a/b");
    assert_eq!(v.len(), 2);
    assert_eq!((v[0].bandwidth, v[0].uri.as_str()), (2149280, "https://cdn.test/x/url_0/hd.m3u8"));
    assert_eq!((v[1].bandwidth, v[1].uri.as_str()), (246440, "https://cdn.test/abs/ld.m3u8"));
}

#[test]
fn media_playlist() {
    let text = "#EXTM3U\n#EXT-X-TARGETDURATION:11\n#EXTINF:10.000,\na.ts\n#EXTINF:9.5,title\nhttp://o.test/b.ts\n#EXT-X-ENDLIST\n";
    let s = segments(text, "https://cdn.test/v/index.m3u8");
    assert_eq!(s.len(), 2);
    assert_eq!((s[0].start, s[0].duration, s[0].uri.as_str()), (0.0, 10.0, "https://cdn.test/v/a.ts"));
    assert_eq!((s[1].start, s[1].duration, s[1].uri.as_str()), (10.0, 9.5, "http://o.test/b.ts"));
    assert!(variants(text, "").is_empty());
}

#[test]
fn live_playlist_has_no_endlist() {
    let text = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:100\n#EXTINF:6.0,\na.ts\n#EXTINF:5.5,\nb.ts\n";
    let m = media(text, "https://cdn.test/v/index.m3u8");
    assert!(m.live() && !m.ended);
    assert_eq!((m.target_duration, m.media_sequence), (6.0, 100));
    let seq: Vec<u64> = m.segments.iter().map(|s| s.sequence).collect();
    assert_eq!(seq, [100, 101]);
    assert_eq!(m.segments[1].start, 6.0);
}

#[test]
fn endlist_ends_an_event_playlist_and_keeps_its_segments() {
    let head = "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:EVENT\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:0\n";
    let first = media(&(head.to_string() + "#EXTINF:4.0,\na.ts\n"), "https://cdn.test/v/index.m3u8");
    assert!(first.live());
    let second = media(&(head.to_string() + "#EXTINF:4.0,\na.ts\n#EXTINF:3.0,\nb.ts\n#EXT-X-ENDLIST\n"), "https://cdn.test/v/index.m3u8");
    assert!(!second.live() && second.ended);
    // Same first segment on both reloads: the sequence number is what links them.
    assert_eq!((first.segments[0].sequence, second.segments[0].sequence), (0, 0));
    assert_eq!(second.segments[1].sequence, 1);
    assert_eq!(second.segments[0].uri, first.segments[0].uri);
}

#[test]
fn a_sliding_window_keeps_its_start_times() {
    let base = "https://cdn.test/v/index.m3u8";
    let before = media("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:4,\na0.ts\n#EXTINF:4,\na1.ts\n#EXTINF:4,\na2.ts\n", base).segments;
    // The next reload drops seq 0 and adds seq 3: seq 1 and 2 keep their times (4 s and 8 s).
    let mut after = media("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:4,\na1.ts\n#EXTINF:4,\na2.ts\n#EXTINF:4,\na3.ts\n", base).segments;
    align(&mut after, &before);
    assert_eq!(after.iter().map(|s| (s.sequence, s.start)).collect::<Vec<_>>(), [(1, 4.0), (2, 8.0), (3, 12.0)]);
}

#[test]
fn a_window_with_no_segment_in_common_follows_on() {
    let base = "https://cdn.test/v/index.m3u8";
    let before = media("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:4,\na0.ts\n#EXTINF:4,\na1.ts\n", base).segments;
    let mut after = media("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:9\n#EXTINF:4,\nb9.ts\n", base).segments;
    align(&mut after, &before);
    assert_eq!((after[0].sequence, after[0].start), (9, 8.0));
}

#[test]
fn joins_every_kind_of_uri() {
    let base = "https://cdn.test/a/b/index.m3u8?t=1";
    assert_eq!(join(base, "seg.ts"), "https://cdn.test/a/b/seg.ts");
    assert_eq!(join(base, "seg.ts?next=http://x.test/"), "https://cdn.test/a/b/seg.ts?next=http://x.test/");
    assert_eq!(join(base, "/root.ts"), "https://cdn.test/root.ts");
    assert_eq!(join(base, "//other.test/x.ts"), "https://other.test/x.ts");
    assert_eq!(join(base, "http://other.test/x.ts"), "http://other.test/x.ts");
    // Paths of our own proxy contain a URL: still a path.
    assert_eq!(join("http://127.0.0.1:8080/proxy/https://cdn.test/a.m3u8", "/proxy/https://cdn.test/b.ts"), "http://127.0.0.1:8080/proxy/https://cdn.test/b.ts");
}

#[test]
fn rewrites_uri_lines_and_attributes() {
    let text = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"1@0\"\n#EXTINF:4,\nseg.ts\n";
    let out = rewrite(text, "https://cdn.test/v/index.m3u8", |uri| format!("/proxy/{uri}"));
    assert_eq!(out, "#EXTM3U\n#EXT-X-MAP:URI=\"/proxy/https://cdn.test/v/init.mp4\",BYTERANGE=\"1@0\"\n#EXTINF:4,\n/proxy/https://cdn.test/v/seg.ts\n");
}

#[test]
fn recognizes_playlists() {
    assert!(is_playlist("https://cdn.test/live.M3U8?token=1"));
    assert!(!is_playlist("https://cdn.test/movie.mp4?x=.m3u8"));
}

/// The shape of https://my.penisland.app/video/live.m3u8: fMP4 segments after one init section.
const FMP4: &str = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:17934\n\
#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:2.000,\nvid_17934.m4s\n#EXTINF:2.000,\nvid_17935.m4s\n";

fn map(uri: &str, range: Option<(u64, u64)>) -> Option<Map> {
    Some(Map { uri: uri.into(), range: range.map(|(length, offset)| ByteRange { length, offset }) })
}

#[test]
fn every_segment_after_a_map_gets_its_init_section() {
    let s = segments(FMP4, "https://my.test/video/live.m3u8");
    assert_eq!(s.len(), 2);
    assert!(s.iter().all(|s| s.map == map("https://my.test/video/init.mp4", None)));
}

#[test]
fn a_map_applies_only_to_the_segments_after_it() {
    let text = "#EXTM3U\n#EXTINF:2,\na.ts\n#EXT-X-MAP:URI=\"one.mp4\"\n#EXTINF:2,\nb.m4s\n\
#EXT-X-DISCONTINUITY\n#EXT-X-MAP:URI=\"https://other.test/two.mp4\"\n#EXTINF:2,\nc.m4s\n";
    let maps: Vec<_> = segments(text, "https://cdn.test/v/index.m3u8").into_iter().map(|s| s.map).collect();
    assert_eq!(maps, [None, map("https://cdn.test/v/one.mp4", None), map("https://other.test/two.mp4", None)]);
}

#[test]
fn a_map_byte_range_is_length_at_offset() {
    let with = |range: &str| {
        let text = format!("#EXTM3U\n#EXT-X-MAP:URI=\"all.mp4\",BYTERANGE=\"{range}\"\n#EXTINF:2,\nall.mp4\n");
        segments(&text, "https://cdn.test/index.m3u8").remove(0).map
    };
    assert_eq!(with("720@0"), map("https://cdn.test/all.mp4", Some((720, 0))));
    assert_eq!(with("720@1024"), map("https://cdn.test/all.mp4", Some((720, 1024))));
    assert_eq!(with("720"), map("https://cdn.test/all.mp4", Some((720, 0))), "no offset: from the start");
    assert_eq!(with("lots"), None, "a range that does not parse is not taken for the whole file");
    assert_eq!(with("720@x"), None);
}

#[test]
fn a_map_without_a_uri_is_no_init_section() {
    let text = "#EXTM3U\n#EXT-X-MAP:BYTERANGE=\"720@0\"\n#EXTINF:2,\na.m4s\n";
    assert_eq!(segments(text, "https://cdn.test/index.m3u8")[0].map, None);
}
