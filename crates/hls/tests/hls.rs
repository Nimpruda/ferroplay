use hls::Hls;
use player::{Cmd, Event, Fact, Intent, Player};

const SAMPLE: &[u8] = include_bytes!("../../../fixtures/sample.ts");

fn loaded(data: &[u8]) -> Event {
    // 10 ms per download: fast enough to always pick the best quality.
    Event::Saw(Fact::Loaded { ms: 10.0, data: data.to_vec() })
}

fn fetch(url: &str) -> Cmd {
    Cmd::Fetch(url.into())
}

/// A player with the HLS plugin, asked to play `url`: it fetches the playlist instead of setting `video.src`.
fn playing(url: &str) -> Player {
    let mut p = Player::new().with(Hls::default());
    assert_eq!(p.dispatch(Event::Do(Intent::Load(url.into()))), [fetch(url)]);
    p
}

fn media_playlist(name: &str, segments: usize, duration: f64) -> String {
    let mut text = String::from("#EXTM3U\n#EXT-X-TARGETDURATION:10\n");
    for i in 0..segments {
        text += &format!("#EXTINF:{duration:.3},\n{name}{i}.ts\n");
    }
    text + "#EXT-X-ENDLIST\n"
}

#[test]
fn plays_a_master_playlist_and_moves_up_in_quality() {
    let mut p = playing("https://cdn.test/v/master.m3u8");
    let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=500000\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=5000000\nhigh.m3u8\n";
    assert_eq!(p.dispatch(loaded(master.as_bytes())), [fetch("https://cdn.test/v/low.m3u8")]);
    assert_eq!(
        p.dispatch(loaded(media_playlist("low", 3, 2.0).as_bytes())),
        [Cmd::Open(6.0), fetch("https://cdn.test/v/low0.ts")]
    );

    // 45 KB in 10 ms is ~36 Mbit/s: switch to the high quality, loading its playlist first.
    let cmds = p.dispatch(loaded(SAMPLE));
    let [Cmd::Codec(mime), Cmd::Append(init), Cmd::Append(_), next] = &cmds[..] else { panic!("{:?}", cmds.len()) };
    assert_eq!(mime, "video/mp4; codecs=\"avc1.64000b,mp4a.40.2\"");
    assert_eq!(&init[4..8], b"ftyp");
    assert_eq!(next, &fetch("https://cdn.test/v/high.m3u8"));
    assert_eq!(p.dispatch(loaded(media_playlist("high", 3, 2.0).as_bytes())), [fetch("https://cdn.test/v/high1.ts")]);

    // Same codec setup: no new init segment, just the fragment.
    assert!(matches!(&p.dispatch(loaded(SAMPLE))[..], [Cmd::Append(_), Cmd::Fetch(url)] if url.ends_with("high2.ts")));
    // The last segment ends the stream.
    assert!(matches!(&p.dispatch(loaded(SAMPLE))[..], [Cmd::Append(_), Cmd::End]));
}

#[test]
fn moves_down_in_quality_after_a_single_slow_download() {
    let mut p = playing("https://cdn.test/v/master.m3u8");
    let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=500000\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=5000000\nhigh.m3u8\n";
    p.dispatch(loaded(master.as_bytes()));
    p.dispatch(loaded(media_playlist("low", 9, 2.0).as_bytes()));
    p.dispatch(loaded(SAMPLE)); // very fast: up to high
    assert_eq!(p.dispatch(loaded(media_playlist("high", 9, 2.0).as_bytes())), [fetch("https://cdn.test/v/high1.ts")]);
    // 45 KB in 2 s is ~180 kbit/s: straight back down, without waiting for an average to settle.
    let slow = Event::Saw(Fact::Loaded { ms: 2000.0, data: SAMPLE.to_vec() });
    assert!(matches!(p.dispatch(slow).last(), Some(Cmd::Fetch(u)) if u.ends_with("low2.ts")));
}

#[test]
fn buffers_30_s_ahead_follows_seeks_and_drops_the_far_past() {
    let mut p = playing("https://cdn.test/movie.m3u8");
    // A media playlist straight away (no master): 100 segments of 10 s.
    assert_eq!(p.dispatch(loaded(media_playlist("s", 100, 10.0).as_bytes())), [Cmd::Open(1000.0), fetch("https://cdn.test/s0.ts")]);
    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(Cmd::Fetch(u)) if u.ends_with("s1.ts")));
    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(Cmd::Fetch(u)) if u.ends_with("s2.ts")));
    // 0–30 s is loaded: wait until the playhead moves on.
    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(Cmd::Append(_))));
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(5.0))), [fetch("https://cdn.test/s3.ts")]);
    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(Cmd::Append(_))));

    // Seek to 500 s: drop what is more than 30 s behind, load from the new playhead.
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(500.0))), [Cmd::Remove(40.0), fetch("https://cdn.test/s50.ts")]);

    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(Cmd::Fetch(u)) if u.ends_with("s51.ts")));

    // Seek back: the download in flight is no use any more, the dropped segments load at once.
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(0.0))), [fetch("https://cdn.test/s0.ts")]);
}

#[test]
fn ends_the_stream_again_after_dropping_data() {
    // Removing or appending data reopens a finished Media Source.
    let mut p = playing("https://cdn.test/short.m3u8");
    p.dispatch(loaded(media_playlist("s", 3, 10.0).as_bytes()));
    p.dispatch(loaded(SAMPLE));
    p.dispatch(loaded(SAMPLE));
    assert!(matches!(&p.dispatch(loaded(SAMPLE))[..], [Cmd::Append(_), Cmd::End]));
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(45.0))), [Cmd::Remove(10.0), Cmd::End]);
}

#[test]
fn an_error_page_instead_of_a_playlist_is_a_failed_download() {
    let mut p = playing("https://cdn.test/x.m3u8");
    let page = loaded(b"<html>404</html>");
    assert_eq!(p.dispatch(page.clone()), [fetch("https://cdn.test/x.m3u8")]);
    assert_eq!(p.dispatch(page.clone()), [fetch("https://cdn.test/x.m3u8")]);
    assert_eq!(p.dispatch(page), [], "gives up instead of asking forever");
}

#[test]
fn gives_up_after_three_failed_downloads() {
    let mut p = playing("https://cdn.test/x.m3u8");
    assert_eq!(p.dispatch(Event::Saw(Fact::LoadFailed)), [fetch("https://cdn.test/x.m3u8")]);
    assert_eq!(p.dispatch(Event::Saw(Fact::LoadFailed)), [fetch("https://cdn.test/x.m3u8")]);
    assert_eq!(p.dispatch(Event::Saw(Fact::LoadFailed)), []);
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(1.0))), []);
}

#[test]
fn other_urls_go_to_the_media_element_and_stop_the_stream() {
    let mut p = playing("https://cdn.test/live.M3U8?token=1");
    let mp4 = "https://cdn.test/clip.mp4";
    assert_eq!(p.dispatch(Event::Do(Intent::Load(mp4.into()))), [Cmd::Source(mp4.into())]);
    // The playlist download still in flight lands: ignored.
    assert_eq!(p.dispatch(loaded(media_playlist("s", 3, 2.0).as_bytes())), []);
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(1.0))), []);
}
