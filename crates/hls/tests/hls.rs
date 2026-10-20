use hls::{Hls, Knob};
use player::{Cmd, Event, Fact, Intent, Player};

const SAMPLE: &[u8] = include_bytes!("../../../fixtures/sample.ts");

fn loaded(data: &[u8]) -> Event {
    // 10 ms per download: fast enough to always pick the best quality.
    Event::Saw(Fact::Loaded { ms: 10.0, data: data.to_vec() })
}

fn fetch(url: &str) -> Cmd {
    Cmd::Fetch(url.into(), 0.0)
}

/// Whether `cmds` give up on the stream with a message that contains `reason`.
fn fails_with(cmds: &[Cmd], reason: &str) -> bool {
    matches!(cmds.last(), Some(Cmd::Fail(message)) if message.contains(reason))
}

/// What an fMP4 segment, or an HTML page served with 200, looks like to the MPEG-TS demuxer: nothing.
const NOT_TS: &[u8] = b"<html>not a transport stream</html>";

const MASTER: &str = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=500000\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=5000000\nhigh.m3u8\n";

/// A player with the HLS plugin, asked to play `url`: it fetches the playlist instead of setting `video.src`.
fn playing(url: &str) -> Player {
    playing_with(url, Hls::default())
}

fn playing_with(url: &str, hls: Hls) -> Player {
    let mut p = Player::new().with(hls);
    assert_eq!(p.dispatch(Event::Do(Intent::Load(url.into()))), [fetch(url)]);
    p
}

/// A live window: like `window`, but no `EXT-X-ENDLIST`, so more segments will come.
fn live_window(name: &str, first: u64, count: u64, duration: f64) -> String {
    window(name, first, count, duration).replace("#EXT-X-ENDLIST\n", "")
}

/// A media playlist whose window starts at `first`: segments `first..first+count`, named `name{sequence}.ts`.
fn window(name: &str, first: u64, count: u64, duration: f64) -> String {
    let mut text = format!("#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXT-X-MEDIA-SEQUENCE:{first}\n");
    for sequence in first..first + count {
        text += &format!("#EXTINF:{duration:.3},\n{name}{sequence}.ts\n");
    }
    text + "#EXT-X-ENDLIST\n"
}

#[test]
fn plays_a_master_playlist_and_moves_up_in_quality() {
    let mut p = playing("https://cdn.test/v/master.m3u8");
    assert_eq!(p.dispatch(loaded(MASTER.as_bytes())), [fetch("https://cdn.test/v/low.m3u8")]);
    assert_eq!(
        p.dispatch(loaded(window("low", 0, 3, 2.0).as_bytes())),
        [Cmd::Open(6.0), fetch("https://cdn.test/v/low0.ts")]
    );

    // 45 KB in 10 ms is ~36 Mbit/s: switch to the high quality, loading its playlist first.
    let cmds = p.dispatch(loaded(SAMPLE));
    let [Cmd::Codec(mime), Cmd::Append(init), Cmd::Append(_), next] = &cmds[..] else { panic!("{:?}", cmds.len()) };
    assert_eq!(mime, "video/mp4; codecs=\"avc1.64000b,mp4a.40.2\"");
    assert_eq!(&init[4..8], b"ftyp");
    assert_eq!(next, &fetch("https://cdn.test/v/high.m3u8"));
    assert_eq!(p.dispatch(loaded(window("high", 0, 3, 2.0).as_bytes())), [fetch("https://cdn.test/v/high1.ts")]);

    // Same codec setup: no new init segment, just the fragment.
    assert!(matches!(&p.dispatch(loaded(SAMPLE))[..], [Cmd::Append(_), Cmd::Fetch(url, _)] if url.ends_with("high2.ts")));
    // The last segment ends the stream.
    assert!(matches!(&p.dispatch(loaded(SAMPLE))[..], [Cmd::Append(_), Cmd::End]));
}

#[test]
fn moves_down_in_quality_after_a_single_slow_download() {
    let mut p = playing("https://cdn.test/v/master.m3u8");
    p.dispatch(loaded(MASTER.as_bytes()));
    p.dispatch(loaded(window("low", 0, 9, 2.0).as_bytes()));
    p.dispatch(loaded(SAMPLE)); // very fast: up to high
    assert_eq!(p.dispatch(loaded(window("high", 0, 9, 2.0).as_bytes())), [fetch("https://cdn.test/v/high1.ts")]);
    // 45 KB in 2 s is ~180 kbit/s: straight back down, without waiting for an average to settle.
    let slow = Event::Saw(Fact::Loaded { ms: 2000.0, data: SAMPLE.to_vec() });
    assert!(matches!(p.dispatch(slow).last(), Some(Cmd::Fetch(u, _)) if u.ends_with("low2.ts")));
}

#[test]
fn buffers_30_s_ahead_follows_seeks_and_drops_the_far_past() {
    let mut p = playing("https://cdn.test/movie.m3u8");
    // A media playlist straight away (no master): 100 segments of 10 s.
    assert_eq!(p.dispatch(loaded(window("s", 0, 100, 10.0).as_bytes())), [Cmd::Open(1000.0), fetch("https://cdn.test/s0.ts")]);
    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(Cmd::Fetch(u, _)) if u.ends_with("s1.ts")));
    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(Cmd::Fetch(u, _)) if u.ends_with("s2.ts")));
    // 0–30 s is loaded: wait until the playhead moves on.
    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(Cmd::Append(_))));
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(5.0))), [fetch("https://cdn.test/s3.ts")]);
    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(Cmd::Append(_))));

    // Seek to 500 s: drop what is more than 30 s behind, load from the new playhead.
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(500.0))), [Cmd::Remove(40.0), fetch("https://cdn.test/s50.ts")]);

    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(Cmd::Fetch(u, _)) if u.ends_with("s51.ts")));

    // Seek back: the download in flight is no use any more, the dropped segments load at once.
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(0.0))), [fetch("https://cdn.test/s0.ts")]);
}

#[test]
fn ends_the_stream_again_after_dropping_data() {
    // Removing or appending data reopens a finished Media Source.
    let mut p = playing("https://cdn.test/short.m3u8");
    p.dispatch(loaded(window("s", 0, 3, 10.0).as_bytes()));
    p.dispatch(loaded(SAMPLE));
    p.dispatch(loaded(SAMPLE));
    assert!(matches!(&p.dispatch(loaded(SAMPLE))[..], [Cmd::Append(_), Cmd::End]));
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(45.0))), [Cmd::Remove(10.0), Cmd::End]);
}

#[test]
fn an_error_page_instead_of_a_playlist_is_a_failed_download() {
    let mut p = playing("https://cdn.test/x.m3u8");
    let page = loaded(b"<html>404</html>");
    let warning = Cmd::Warn("https://cdn.test/x.m3u8 lists no segments: not a media playlist".into());
    for _ in 0..3 {
        assert_eq!(p.dispatch(page.clone()), [warning.clone(), fetch("https://cdn.test/x.m3u8")]);
    }
    let cmds = p.dispatch(page);
    assert_eq!(cmds[0], warning);
    assert!(fails_with(&cmds, "x.m3u8 lists no segments"), "gives up instead of asking forever: {cmds:?}");
}

#[test]
fn gives_up_after_three_retries() {
    let mut p = playing("https://cdn.test/x.m3u8");
    for _ in 0..3 {
        assert_eq!(p.dispatch(Event::Saw(Fact::LoadFailed)), [fetch("https://cdn.test/x.m3u8")]);
    }
    let cmds = p.dispatch(Event::Saw(Fact::LoadFailed));
    assert_eq!(cmds, [Cmd::Fail("HLS: giving up: the download of https://cdn.test/x.m3u8 failed".into())]);
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(1.0))), [], "said once");
}

#[test]
fn an_unusable_segment_is_said_and_skipped() {
    let mut p = playing("https://cdn.test/movie.m3u8");
    p.dispatch(loaded(window("s", 0, 10, 2.0).as_bytes()));
    assert_eq!(
        p.dispatch(loaded(NOT_TS)),
        [Cmd::Warn("segment https://cdn.test/s0.ts: no MPEG-TS media this player can play".into()), fetch("https://cdn.test/s1.ts")]
    );
}

#[test]
fn unusable_segments_in_a_row_fail_the_stream_once() {
    let mut p = playing("https://cdn.test/movie.m3u8");
    p.dispatch(loaded(window("s", 0, 10, 2.0).as_bytes()));
    for _ in 0..3 {
        p.dispatch(loaded(NOT_TS));
    }
    let cmds = p.dispatch(loaded(NOT_TS));
    assert!(matches!(&cmds[..], [Cmd::Warn(_), Cmd::Fail(_)]), "{cmds:?}");
    assert!(fails_with(&cmds, "giving up: segment https://cdn.test/s3.ts: no MPEG-TS media"), "{cmds:?}");
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(1.0))), [], "said once, and nothing more is downloaded");
}

#[test]
fn a_usable_segment_resets_the_count_of_unusable_ones() {
    let mut p = playing("https://cdn.test/movie.m3u8");
    p.dispatch(loaded(window("s", 0, 10, 2.0).as_bytes()));
    for data in [NOT_TS, NOT_TS, NOT_TS, SAMPLE, NOT_TS, NOT_TS, NOT_TS] {
        let cmds = p.dispatch(loaded(data));
        assert!(matches!(cmds.last(), Some(Cmd::Fetch(..))), "{cmds:?}");
    }
}

#[test]
fn a_live_stream_fails_after_its_live_limit_of_unusable_segments() {
    let mut p = playing(LIVE);
    p.dispatch(loaded(live_window("s", 0, 12, 2.0).as_bytes()));
    for unusable in 1..=10 {
        assert!(matches!(p.dispatch(loaded(NOT_TS)).last(), Some(Cmd::Fetch(..))), "unusable segment {unusable} should go on");
    }
    assert!(fails_with(&p.dispatch(loaded(NOT_TS)), "giving up: segment https://cdn.test/s10.ts"), "the eleventh gives up");
}

#[test]
fn other_urls_go_to_the_media_element_and_stop_the_stream() {
    let mut p = playing("https://cdn.test/live.M3U8?token=1");
    let mp4 = "https://cdn.test/clip.mp4";
    assert_eq!(p.dispatch(Event::Do(Intent::Load(mp4.into()))), [Cmd::Source(mp4.into())]);
    // The playlist download still in flight lands: ignored.
    assert_eq!(p.dispatch(loaded(window("s", 0, 3, 2.0).as_bytes())), []);
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(1.0))), []);
}

#[test]
fn a_quality_switch_loads_the_same_sequence_number() {
    let mut p = playing("https://cdn.test/v/master.m3u8");
    p.dispatch(loaded(MASTER.as_bytes()));
    p.dispatch(loaded(window("low", 0, 3, 2.0).as_bytes()));
    p.dispatch(loaded(SAMPLE)); // seq 0 in: the fast download switches to high, which loads its playlist first
    p.dispatch(loaded(SAMPLE)); // seq 1 in, from the low playlist
    // The high playlist has moved on: its window starts at seq 1. Seq 2 is the next one, which is high1.ts.
    assert_eq!(p.dispatch(loaded(window("high", 1, 3, 2.0).as_bytes())), [fetch("https://cdn.test/v/high1.ts")]);
}

#[test]
fn a_switch_to_a_later_window_keeps_its_timeline() {
    let mut p = playing("https://cdn.test/v/master.m3u8");
    p.dispatch(loaded(MASTER.as_bytes()));
    p.dispatch(loaded(window("low", 0, 10, 10.0).as_bytes()));
    p.dispatch(loaded(SAMPLE)); // seq 0 in, then the switch to high
    // High's window is seq 5..9, at 50 s on the timeline (the low playlist's times). Nothing else is loaded.
    p.dispatch(loaded(window("high", 5, 5, 10.0).as_bytes()));
    // At 45 s, seq 5 (50 s) is within reach: it loads, instead of being taken for a segment at 0 s.
    assert!(matches!(p.dispatch(Event::Saw(Fact::Time(45.0))).last(), Some(Cmd::Fetch(u, _)) if u.ends_with("high5.ts")));
}

#[test]
fn a_live_playlist_opens_endless_and_sends_its_window() {
    let mut p = playing("https://cdn.test/live.m3u8");
    assert_eq!(
        p.dispatch(loaded(live_window("s", 0, 3, 2.0).as_bytes())),
        [Cmd::Open(f64::INFINITY), Cmd::SeekableRange(0.0, 6.0), fetch("https://cdn.test/s0.ts")]
    );
}

#[test]
fn a_vod_playlist_never_sends_the_live_command() {
    let mut p = playing("https://cdn.test/movie.m3u8");
    let cmds = p.dispatch(loaded(window("s", 0, 3, 2.0).as_bytes()));
    assert_eq!(cmds, [Cmd::Open(6.0), fetch("https://cdn.test/s0.ts")]);
    assert!(!p.dispatch(loaded(SAMPLE)).iter().any(|c| matches!(c, Cmd::SeekableRange(..) | Cmd::Open(_))));
}

const LIVE: &str = "https://cdn.test/live.m3u8";

fn is_delay(cmd: &Cmd, url: &str, seconds: f64) -> bool {
    matches!(cmd, Cmd::Fetch(u, d) if u == url && (d - seconds).abs() < 1e-9)
}

#[test]
fn a_live_playlist_is_reloaded_once_its_segments_are_in_and_keeps_playing() {
    let mut p = playing(LIVE);
    assert_eq!(
        p.dispatch(loaded(live_window("s", 0, 3, 2.0).as_bytes())),
        [Cmd::Open(f64::INFINITY), Cmd::SeekableRange(0.0, 6.0), fetch("https://cdn.test/s0.ts")]
    );
    p.dispatch(loaded(SAMPLE));
    p.dispatch(loaded(SAMPLE));
    // The last segment is in, and the stream is live: no End, and the reload waits until it is due
    // (10 s after the playlist arrived at 0.01 s on the download clock, and 0.04 s have passed since).
    let cmds = p.dispatch(loaded(SAMPLE));
    assert!(!cmds.iter().any(|c| matches!(c, Cmd::End)), "{cmds:?}");
    assert!(matches!(cmds.last(), Some(c) if is_delay(c, LIVE, 9.97)), "{cmds:?}");

    // The reload brings the next segment, the first one has slid out of the window: only s3 is asked for.
    let cmds = p.dispatch(loaded(live_window("s", 1, 3, 2.0).as_bytes()));
    assert_eq!(cmds, [Cmd::SeekableRange(2.0, 8.0), fetch("https://cdn.test/s3.ts")]);
}

#[test]
fn a_reload_that_brings_nothing_new_comes_back_in_half_a_target_duration() {
    let mut p = playing(LIVE);
    p.dispatch(loaded(live_window("s", 0, 3, 2.0).as_bytes()));
    for _ in 0..3 {
        p.dispatch(loaded(SAMPLE));
    }
    // The reload brings nothing new, so the next one is due after half of the 10 s target: it waits 5 s.
    let cmds = p.dispatch(loaded(live_window("s", 0, 3, 2.0).as_bytes()));
    assert!(matches!(cmds.last(), Some(c) if is_delay(c, LIVE, 5.0)), "{cmds:?}");
}

#[test]
fn a_needed_segment_replaces_a_waiting_reload_and_the_reload_is_rescheduled() {
    // 20 s segments and a 30 s target: no hold-back, and only the first two segments are within reach at 0 s.
    // The reload is due at 0.01 s + 30 s; 0.03 s have passed when it is asked for.
    let mut p = playing(LIVE);
    p.dispatch(loaded(live_window("s", 0, 3, 20.0).replace("TARGETDURATION:10", "TARGETDURATION:30").as_bytes()));
    p.dispatch(loaded(SAMPLE));
    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(c) if is_delay(c, LIVE, 29.98)));
    // The playhead moves on: s2 (at 40 s) is within 30 s of 15 s. It replaces the waiting reload.
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(15.0))), [fetch("https://cdn.test/s2.ts")]);
}

#[test]
fn a_quality_switch_while_live_fetches_the_new_playlist_first() {
    let mut p = playing("https://cdn.test/v/master.m3u8");
    p.dispatch(loaded(MASTER.as_bytes()));
    p.dispatch(loaded(live_window("low", 0, 3, 2.0).as_bytes()));
    let cmds = p.dispatch(loaded(SAMPLE));
    assert_eq!(cmds.last(), Some(&fetch("https://cdn.test/v/high.m3u8")));
    assert_eq!(
        p.dispatch(loaded(live_window("high", 0, 3, 2.0).as_bytes())),
        [Cmd::SeekableRange(0.0, 6.0), fetch("https://cdn.test/v/high1.ts")]
    );
}

#[test]
fn an_endlist_on_a_reload_stops_the_reloads_and_ends_the_stream() {
    let mut p = playing(LIVE);
    p.dispatch(loaded(live_window("s", 0, 3, 2.0).as_bytes()));
    for _ in 0..3 {
        p.dispatch(loaded(SAMPLE));
    }
    // The reload finds the stream has ended: the last segment is already in, so End now.
    assert_eq!(p.dispatch(loaded(window("s", 0, 3, 2.0).as_bytes())), [Cmd::End]);
}

/// A live window of `count` 2 s segments with a 4 s target: the hold-back is 12 s from the end (at 20 s).
fn live_window_4s_target(count: u64) -> String {
    live_window("s", 0, count, 2.0).replace("#EXT-X-TARGETDURATION:10", "#EXT-X-TARGETDURATION:4")
}

#[test]
fn a_live_stream_starts_three_target_durations_from_its_end() {
    let mut p = playing(LIVE);
    // 10 segments of 2 s end at 20 s; 20 − 3 × 4 = 8 s, which is in segment 4 (8–10 s).
    assert_eq!(
        p.dispatch(loaded(live_window_4s_target(10).as_bytes())),
        [Cmd::Open(f64::INFINITY), Cmd::SeekableRange(0.0, 20.0), Cmd::Seek(8.0), fetch("https://cdn.test/s4.ts")]
    );
}

#[test]
fn seeking_anywhere_in_the_window_plays_from_there() {
    let mut p = playing(LIVE);
    p.dispatch(loaded(live_window_4s_target(10).as_bytes()));
    // The element reports the seek to the hold-back: from then on the playhead decides.
    assert_eq!(p.dispatch(Event::Saw(Fact::Seeking(8.0))), [], "segment 4 is already on its way");
    // The user seeks to 15 s: segment 7 (14–16 s) is loaded next.
    assert_eq!(p.dispatch(Event::Saw(Fact::Seeking(15.0))), [fetch("https://cdn.test/s7.ts")]);
}

#[test]
fn a_tuned_holdback_changes_where_a_live_stream_starts() {
    // Two target durations instead of three: 20 − 2 × 4 = 12 s, which is in segment 6 (12–14 s).
    let mut p = playing_with(LIVE, Hls::tuned(|knob| match knob {
        Knob::Holdback(target) => Some(2.0 * target),
        _ => None,
    }));
    assert_eq!(
        p.dispatch(loaded(live_window_4s_target(10).as_bytes())),
        [Cmd::Open(f64::INFINITY), Cmd::SeekableRange(0.0, 20.0), Cmd::Seek(12.0), fetch("https://cdn.test/s6.ts")]
    );
}

#[test]
fn a_tuned_ahead_of_zero_loads_no_segment_until_the_playhead_moves() {
    let mut p = playing_with(LIVE, Hls::tuned(|knob| matches!(knob, Knob::Ahead(_)).then_some(0.0)));
    // Nothing is within reach, so the only download is the reload, due 10 s after the playlist.
    assert_eq!(
        p.dispatch(loaded(live_window("s", 0, 3, 2.0).as_bytes())),
        [Cmd::Open(f64::INFINITY), Cmd::SeekableRange(0.0, 6.0), Cmd::Fetch(LIVE.into(), 10.0)]
    );
}

#[test]
fn a_tuned_value_that_makes_no_sense_is_not_hidden() {
    // A negative, infinite or NaN hold-back panics, naming the knob: nothing falls back to a default.
    for nonsense in [-1.0, f64::INFINITY, f64::NAN] {
        let crashed = std::panic::catch_unwind(move || {
            let mut p = playing_with(LIVE, Hls::tuned(move |knob| matches!(knob, Knob::Holdback(_)).then_some(nonsense)));
            p.dispatch(loaded(live_window_4s_target(10).as_bytes()));
        });
        assert!(crashed.is_err(), "{nonsense} should crash");
    }
}

#[test]
fn a_live_stream_keeps_trying_until_its_live_limit() {
    let mut p = playing(LIVE);
    p.dispatch(loaded(live_window("s", 0, 3, 2.0).as_bytes()));
    // Ten failures in a row are retried (the default live retries); the eleventh gives up.
    for failure in 1..=10 {
        assert!(!p.dispatch(Event::Saw(Fact::LoadFailed)).is_empty(), "failure {failure} should retry");
    }
    assert!(fails_with(&p.dispatch(Event::Saw(Fact::LoadFailed)), "giving up: the download of"), "the eleventh gives up");
}

#[test]
fn a_tuned_live_limit_changes_when_a_live_stream_gives_up() {
    let mut p = playing_with(LIVE, Hls::tuned(|knob| matches!(knob, Knob::LiveRetries).then_some(2.0)));
    p.dispatch(loaded(live_window("s", 0, 3, 2.0).as_bytes()));
    for _ in 0..2 {
        assert!(!p.dispatch(Event::Saw(Fact::LoadFailed)).is_empty());
    }
    assert!(fails_with(&p.dispatch(Event::Saw(Fact::LoadFailed)), "giving up: the download of"));
}

/// A live master: low first, then a fast download switches to high, whose two next segments load. A reload
/// of high is then waiting.
fn live_switch_to_high() -> Player {
    let mut p = playing("https://cdn.test/v/master.m3u8");
    p.dispatch(loaded(MASTER.as_bytes()));
    p.dispatch(loaded(live_window("low", 0, 3, 2.0).as_bytes()));
    assert_eq!(p.dispatch(loaded(SAMPLE)).last(), Some(&fetch("https://cdn.test/v/high.m3u8")));
    p.dispatch(loaded(live_window("high", 0, 3, 2.0).as_bytes()));
    p.dispatch(loaded(SAMPLE));
    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(Cmd::Fetch(u, d)) if u.ends_with("high.m3u8") && *d > 0.0));
    p
}

#[test]
fn after_a_quality_switch_live_reloads_follow_the_new_quality() {
    // Reloads of high slide one segment each time: sequence k starts at 2k s. They line up with high's own
    // last playlist, not with low's, which stopped being reloaded at the switch.
    let mut p = live_switch_to_high();
    for k in 1..=5u64 {
        let cmds = p.dispatch(loaded(live_window("high", k, 3, 2.0).as_bytes()));
        assert_eq!(cmds.first(), Some(&Cmd::SeekableRange(2.0 * k as f64, 2.0 * k as f64 + 6.0)), "reload {k}: {cmds:?}");
        p.dispatch(loaded(SAMPLE));
    }
}

#[test]
fn an_endlist_on_the_new_quality_ends_only_once_its_own_last_segment_is_in() {
    let mut p = live_switch_to_high();
    // high3..high5 are missing: fetch the next one, no End (low's last segment being in does not count).
    assert_eq!(p.dispatch(loaded(window("high", 0, 6, 2.0).as_bytes())), [fetch("https://cdn.test/v/high3.ts")]);
}

#[test]
fn a_failed_live_reload_is_retried_after_half_a_target_duration() {
    let mut p = playing(LIVE);
    p.dispatch(loaded(live_window("s", 0, 3, 2.0).as_bytes()));
    for _ in 0..3 {
        p.dispatch(loaded(SAMPLE));
    }
    // The reload waited its 9.97 s and failed: the retry waits half the 10 s target, not nothing.
    let cmds = p.dispatch(Event::Saw(Fact::LoadFailed));
    assert!(matches!(cmds.last(), Some(c) if is_delay(c, LIVE, 5.0)), "{cmds:?}");
}

#[test]
fn a_short_live_window_still_drops_what_is_far_behind() {
    // Three 2 s segments slide on every reload, so a segment leaves the window long before it is 30 s
    // behind the playhead. It is dropped by its time, not by the window.
    let target_2s = |first| live_window("s", first, 3, 2.0).replace("TARGETDURATION:10", "TARGETDURATION:2");
    let mut p = playing(LIVE);
    let mut cmds = p.dispatch(loaded(target_2s(0).as_bytes()));
    for _ in 0..3 {
        cmds.extend(p.dispatch(loaded(SAMPLE)));
    }
    for k in 1..=30u64 {
        cmds.extend(p.dispatch(loaded(target_2s(k).as_bytes())));
        cmds.extend(p.dispatch(loaded(SAMPLE)));
        cmds.extend(p.dispatch(Event::Saw(Fact::Time(2.0 * k as f64 + 1.0))));
    }
    // Dropping happens when the next download is decided, so it keeps up with the playhead (61 s at the end,
    // 30 s kept) to within a segment or two.
    let last_drop = cmds.iter().rev().find_map(|c| if let Cmd::Remove(until) = c { Some(*until) } else { None });
    assert!(last_drop.is_some_and(|until| until > 25.0), "last drop: {last_drop:?}");
}

#[test]
fn a_huge_sequence_number_does_not_overflow() {
    let mut p = playing(LIVE);
    p.dispatch(loaded(live_window("s", u32::MAX as u64, 3, 2.0).as_bytes()));
    assert!(matches!(p.dispatch(loaded(SAMPLE)).last(), Some(Cmd::Fetch(..))));
}

/// ffmpeg's fMP4 version of fixtures/sample.ts: the init section, then two 1 s segments (stream times 0 and 1 s).
const INIT: &[u8] = include_bytes!("../../../fixtures/fmp4-init.mp4");
const SEGMENT_0: &[u8] = include_bytes!("../../../fixtures/fmp4-0.m4s");
const SEGMENT_1: &[u8] = include_bytes!("../../../fixtures/fmp4-1.m4s");
const FMP4_MIME: &str = "video/mp4; codecs=\"avc1.64000b,mp4a.40.2\"";

/// The init section with its H.264 sample entry renamed hvc1: a codec this player cannot play.
const HEVC_INIT: &[u8] = include_bytes!("../../../fixtures/fmp4-hevc-init.mp4");

/// Like `window`, with 1 s fMP4 segments `{name}{sequence}.m4s` after `#EXT-X-MAP` `{name}init.mp4` (and `map` attributes).
fn fmp4_window(name: &str, count: u64, map: &str) -> String {
    let segments = window(name, 0, count, 1.0).replace(".ts\n", ".m4s\n");
    segments.replacen("#EXTM3U\n", &format!("#EXTM3U\n#EXT-X-MAP:URI=\"{name}init.mp4\"{map}\n"), 1)
}

#[test]
fn fmp4_segments_are_appended_after_their_init_section() {
    let mut p = playing("https://cdn.test/f.m3u8");
    assert_eq!(p.dispatch(loaded(fmp4_window("", 3, "").as_bytes())), [Cmd::Open(3.0), fetch("https://cdn.test/init.mp4")]);
    assert_eq!(
        p.dispatch(loaded(INIT)),
        [Cmd::Codec(FMP4_MIME.into()), Cmd::Append(INIT.to_vec()), fetch("https://cdn.test/0.m4s")]
    );
    // The stream's own times start at 1 s here: the segment is moved to its playlist time, 0, and sent as it is
    // otherwise (no conversion). The init section is in: the next download is the next segment.
    let cmds = p.dispatch(loaded(SEGMENT_1));
    let [Cmd::Append(data), next] = &cmds[..] else { panic!("{cmds:?}") };
    assert_eq!(next, &fetch("https://cdn.test/1.m4s"));
    assert_eq!(data.len(), SEGMENT_1.len());
    let tracks = ts2mp4::read_init(INIT).unwrap().tracks;
    assert_eq!(ts2mp4::offset(&tracks, data, 0.0).unwrap(), 0.0, "the earliest track starts at 0");
}

#[test]
fn a_quality_switch_fetches_the_new_init_section_and_appends_it_only_if_it_differs() {
    let mut p = playing("https://cdn.test/v/master.m3u8");
    p.dispatch(loaded(MASTER.as_bytes()));
    assert_eq!(p.dispatch(loaded(fmp4_window("low", 3, "").as_bytes())), [Cmd::Open(3.0), fetch("https://cdn.test/v/lowinit.mp4")]);
    p.dispatch(loaded(INIT));
    // A fast download: up to high, whose playlist names its own init section.
    assert_eq!(p.dispatch(loaded(SEGMENT_0)).last(), Some(&fetch("https://cdn.test/v/high.m3u8")));
    assert_eq!(p.dispatch(loaded(fmp4_window("high", 3, "").as_bytes())), [fetch("https://cdn.test/v/highinit.mp4")]);
    // The same bytes as the init section in the Media Source: nothing to append, on to the segment.
    assert_eq!(p.dispatch(loaded(INIT)), [fetch("https://cdn.test/v/high1.m4s")]);
}

#[test]
fn an_unplayable_init_section_fails_the_stream_at_once() {
    let mut p = playing("https://cdn.test/f.m3u8");
    p.dispatch(loaded(fmp4_window("", 3, "").as_bytes()));
    let reason = "https://cdn.test/init.mp4: init section: hvc1 is a codec this player cannot play";
    assert_eq!(p.dispatch(loaded(HEVC_INIT)), [Cmd::Warn(reason.into()), Cmd::Fail(["HLS: giving up: ", reason].concat())]);
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(1.0))), [], "nothing more is downloaded");
}

#[test]
fn tuned_init_retries_try_the_init_section_again() {
    let mut p = playing_with("https://cdn.test/f.m3u8", Hls::tuned(|knob| matches!(knob, Knob::InitRetries).then_some(2.0)));
    p.dispatch(loaded(fmp4_window("", 3, "").as_bytes()));
    for _ in 0..2 {
        assert!(matches!(&p.dispatch(loaded(HEVC_INIT))[..], [Cmd::Warn(_), Cmd::Fetch(url, _)] if url.ends_with("init.mp4")));
    }
    assert!(matches!(&p.dispatch(loaded(HEVC_INIT))[..], [Cmd::Warn(_), Cmd::Fail(_)]));
}

#[test]
fn a_byte_range_init_section_fails_without_being_fetched() {
    let mut p = playing("https://cdn.test/f.m3u8");
    let reason = "https://cdn.test/init.mp4 is a byte range (EXT-X-MAP BYTERANGE), which this player cannot fetch yet";
    assert_eq!(
        p.dispatch(loaded(fmp4_window("", 3, ",BYTERANGE=\"720@0\"").as_bytes())),
        [Cmd::Open(3.0), Cmd::Warn(reason.into()), Cmd::Fail(["HLS: giving up: ", reason].concat())]
    );
}

#[test]
fn an_fmp4_segment_that_goes_back_in_time_is_unusable() {
    // The first segment sets the offset (its stream time 1 s at 0 s); one at stream time 0 would land before 0.
    let mut p = playing("https://cdn.test/f.m3u8");
    p.dispatch(loaded(fmp4_window("", 3, "").as_bytes()));
    p.dispatch(loaded(INIT));
    p.dispatch(loaded(SEGMENT_1));
    let reason = "segment https://cdn.test/1.m4s: media segment: a decode time before the start of the stream";
    assert_eq!(p.dispatch(loaded(SEGMENT_0)), [Cmd::Warn(reason.into()), fetch("https://cdn.test/2.m4s")]);
}
