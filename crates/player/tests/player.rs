use player::plugins::{AbLoop, Keyboard, Loop, StartAt};
use player::{Cmd, Event, Fact, Intent, Player};

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
    let mut p = Player::new().with(Keyboard { step: 5.0 });
    p.dispatch(Event::Saw(Fact::Duration(f64::INFINITY)));
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
    p.dispatch(Event::Saw(Fact::Buffered(45.0)));
    let v = p.state.view();
    assert_eq!((v.progress, v.buffered), (0.5, 0.7));
    assert_eq!(v.label, "0:35");
}
