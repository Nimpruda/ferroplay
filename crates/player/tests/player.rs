use player::plugins::{AbLoop, Keyboard, Loop, StartAt};
use player::{Cmd, Event, Fact, Intent, Player, Ranges};

fn loaded(duration: f64) -> Player {
    let mut p = Player::new().with(Keyboard { step: 5.0 });
    p.dispatch(Event::Saw(Fact::Duration(duration)));
    p
}

#[test]
fn toggle_follows_paused_state() {
    let mut p = loaded(60.0);
    assert_eq!(p.dispatch(Event::Do(Intent::Toggle)), [Cmd::Play]);
    p.dispatch(Event::Saw(Fact::Playing));
    assert_eq!(p.dispatch(Event::Do(Intent::Toggle)), [Cmd::Pause]);
}

#[test]
fn seeks_are_clamped_and_stack_before_media_reports() {
    let mut p = loaded(12.0);
    assert_eq!(p.dispatch(Event::Key('→')), [Cmd::Seek(5.0)]);
    assert_eq!(p.dispatch(Event::Key('→')), [Cmd::Seek(10.0)]);
    assert_eq!(p.dispatch(Event::Key('→')), [Cmd::Seek(12.0)]);
    assert_eq!(p.dispatch(Event::Key('5')), [Cmd::Seek(6.0)]);
}

#[test]
fn volume_keys_clamp_and_unmute() {
    let mut p = loaded(10.0);
    p.dispatch(Event::Saw(Fact::Muted(true)));
    assert_eq!(p.dispatch(Event::Key('↑')), [Cmd::Volume(1.0), Cmd::Muted(false)]);
    assert_eq!(p.dispatch(Event::Do(Intent::Volume(0.0))), [Cmd::Volume(0.0), Cmd::Muted(true)]);
}

#[test]
fn ten_volume_steps_down_reach_exactly_zero_and_mute() {
    let mut p = loaded(10.0);
    let mut cmds = Vec::new();
    for _ in 0..10 {
        cmds = p.dispatch(Event::Key('↓'));
        let Cmd::Volume(v) = cmds[0].clone() else { panic!("{cmds:?}") };
        p.dispatch(Event::Saw(Fact::Volume(v)));
    }
    assert_eq!(cmds, [Cmd::Volume(0.0), Cmd::Muted(true)]);
}

#[test]
fn pausing_hides_the_spinner() {
    let mut p = loaded(10.0);
    p.dispatch(Event::Saw(Fact::Playing));
    p.dispatch(Event::Saw(Fact::Waiting));
    assert!(p.state.view().waiting);
    p.dispatch(Event::Saw(Fact::Paused));
    assert!(!p.state.view().waiting);
}

#[test]
fn unknown_keys_do_nothing() {
    assert_eq!(loaded(10.0).dispatch(Event::Key('z')), []);
}

#[test]
fn plugins_compose() {
    let mut p = loaded(30.0).with(Loop).with(StartAt::new(7.0));
    assert_eq!(p.dispatch(Event::Saw(Fact::Duration(30.0))), [Cmd::Seek(7.0)]);
    assert_eq!(p.dispatch(Event::Saw(Fact::Duration(30.0))), []);
    assert_eq!(p.dispatch(Event::Saw(Fact::Ended)), [Cmd::Seek(0.0), Cmd::Play]);
}

#[test]
fn ab_loop_repeats_a_section_until_b_is_pressed_again() {
    let mut p = loaded(60.0).with(AbLoop::default());
    p.dispatch(Event::Saw(Fact::Time(10.0)));
    assert_eq!(p.dispatch(Event::Key('a')), []);
    assert_eq!(p.dispatch(Event::Key('b')), [], "end must come after start");
    p.dispatch(Event::Saw(Fact::Time(14.0)));
    assert_eq!(p.dispatch(Event::Key('b')), [Cmd::Seek(10.0)]);
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(12.0))), []);
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(14.1))), [Cmd::Seek(10.0)]);
    p.dispatch(Event::Key('b'));
    assert_eq!(p.dispatch(Event::Saw(Fact::Time(14.1))), []);
}

#[test]
fn closures_are_plugins() {
    let mut p = loaded(100.0).with(|_: &_, e: &Event, out: &mut Vec<_>| {
        if *e == Event::Key('s') {
            out.push(Intent::Seek(90.0)); // "skip intro"
        }
    });
    assert_eq!(p.dispatch(Event::Key('s')), [Cmd::Seek(90.0)]);
}

#[test]
fn view_formats_time() {
    let mut p = loaded(3725.0);
    p.dispatch(Event::Saw(Fact::Time(65.9)));
    let v = p.state.view();
    assert_eq!(v.label, "1:05 / 1:02:05");
    assert_eq!(loaded(0.0).state.view().label, "0:00 / 0:00");
}

fn live(window: (f64, f64)) -> Player {
    let mut p = live_without_window();
    p.dispatch(Event::Saw(Fact::Seekable(window.0, window.1)));
    p
}

#[test]
fn live_seeks_clamp_to_the_reported_window() {
    let mut p = live((10.0, 60.0));
    assert_eq!(p.dispatch(Event::Key('→')), [Cmd::Seek(10.0)], "before the window: clamped to its start");
    p.dispatch(Event::Saw(Fact::Time(60.0)));
    assert_eq!(p.dispatch(Event::Key('→')), [Cmd::Seek(60.0)], "after the window: clamped to its end");
}

#[test]
fn live_seek_frac_maps_onto_the_window() {
    let mut p = live((10.0, 60.0));
    assert_eq!(p.dispatch(Event::Key('5')), [Cmd::Seek(35.0)]);
    // Nothing to map onto before the host reports a window.
    assert_eq!(live_without_window().dispatch(Event::Key('5')), []);
}

fn live_without_window() -> Player {
    let mut p = Player::new().with(Keyboard { step: 5.0 });
    p.dispatch(Event::Saw(Fact::Duration(f64::INFINITY)));
    p
}

#[test]
fn live_progress_is_relative_to_the_window_and_the_label_has_no_total() {
    let mut p = live((10.0, 60.0));
    p.dispatch(Event::Saw(Fact::Time(35.0)));
    p.dispatch(Event::Saw(Fact::Buffered(Ranges(vec![(30.0, 45.0)]))));
    let v = p.state.view();
    assert_eq!((v.progress, v.buffered), (0.5, 0.7));
    assert_eq!(v.label, "0:35");
}

#[test]
fn start_at_waits_for_a_finite_duration() {
    // A live stream has no position in a known whole: StartAt leaves it to pick its own start.
    let mut p = Player::new().with(StartAt::new(20.0));
    assert_eq!(p.dispatch(Event::Saw(Fact::Duration(f64::INFINITY))), []);
    assert_eq!(p.dispatch(Event::Saw(Fact::Duration(60.0))), [Cmd::Seek(20.0)]);
}

#[test]
fn live_progress_stays_on_the_bar_when_the_window_slides_past_the_playhead() {
    let mut p = live((100.0, 124.0));
    p.dispatch(Event::Saw(Fact::Time(90.0))); // paused while the window slid on
    p.dispatch(Event::Saw(Fact::Buffered(Ranges(vec![(100.0, 110.0)])))); // nothing buffered under the playhead
    let v = p.state.view();
    assert_eq!((v.progress, v.buffered), (0.0, 0.0));
}

#[test]
fn a_new_load_forgets_the_old_seekable_window() {
    let mut p = live((3600.0, 3624.0));
    p.dispatch(Event::Do(Intent::Load("https://cdn.test/other.m3u8".into())));
    // Still live, but the new stream has not reported its window: nothing to map the number keys onto.
    assert_eq!(p.dispatch(Event::Key('0')), []);
}

#[test]
fn the_buffered_bar_ends_with_the_range_under_the_playhead() {
    let mut p = loaded(100.0);
    p.dispatch(Event::Saw(Fact::Buffered(Ranges(vec![(0.0, 10.0), (20.0, 30.0)]))));
    let bar = |p: &mut Player, t| {
        p.dispatch(Event::Saw(Fact::Time(t)));
        p.state.view().buffered
    };
    assert_eq!(bar(&mut p, 5.0), 0.1);
    assert_eq!(bar(&mut p, 10.0), 0.1, "the end of a range is in it");
    assert_eq!(bar(&mut p, 15.0), 0.0, "between ranges: nothing plays from here");
    assert_eq!(bar(&mut p, 25.0), 0.3);
}

#[test]
fn ranges_answer_where_a_time_is_and_what_comes_next() {
    let r = Ranges(vec![(0.066, 2.0), (2.05, 4.0)]);
    assert_eq!(r.end_at(1.0), Some(2.0));
    assert_eq!(r.end_at(0.0), None, "before the first range");
    assert_eq!(r.next_start(0.0), Some(0.066));
    assert_eq!(r.next_start(2.0), Some(2.05));
    assert_eq!(r.next_start(2.05), None, "a range that starts at t is not after it");
    assert_eq!(Ranges::default().end_at(0.0), None);
    assert_eq!(Ranges::default().next_start(0.0), None);
}

#[test]
fn ranges_come_from_the_hosts_bytes_or_not_at_all() {
    let bytes: Vec<u8> = [0.5f64, 2.0, 3.0, 4.5].iter().flat_map(|x| x.to_le_bytes()).collect();
    assert_eq!(Ranges::from_le_bytes(&bytes), Some(Ranges(vec![(0.5, 2.0), (3.0, 4.5)])));
    assert_eq!(Ranges::from_le_bytes(&[]), Some(Ranges::default()), "nothing buffered");
    for len in [8, 15, 17, 24] {
        assert_eq!(Ranges::from_le_bytes(&bytes[..len]), None, "{len} bytes");
    }
}
