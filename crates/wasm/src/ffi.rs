//! The wasm bridge: every call carries a code, a number, and optional bytes as (pointer, length).
//! The only `unsafe` in the workspace. Event, command and plugin codes must match `web/player.js`.

use alloc::{boxed::Box, string::String, vec, vec::Vec};
use core::cell::RefCell;
use hls::Hls;
use player::plugins::{AbLoop, Keyboard, Loop, StartAt};
use player::{Cmd, Event, Fact, Intent, Player};

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[link(wasm_import_module = "host")]
unsafe extern "C" {
    fn cmd(id: u32, code: u32, value: f64, data: *const u8, len: usize);
    fn render(
        id: u32,
        playing: bool,
        waiting: bool,
        muted: bool,
        progress: f64,
        buffered: f64,
        volume: f64,
        label: *const u8,
        len: usize,
    );
}

struct Players(RefCell<Vec<Option<Player>>>);
// SAFETY: wasm32-unknown-unknown is single-threaded.
unsafe impl Sync for Players {}
static PLAYERS: Players = Players(RefCell::new(Vec::new()));

fn with<R>(id: u32, f: impl FnOnce(&mut Player) -> R) -> Option<R> {
    PLAYERS.0.borrow_mut().get_mut(id as usize)?.as_mut().map(f)
}

/// A buffer for the host to fill. Ownership comes back with `player_event`.
#[unsafe(no_mangle)]
pub extern "C" fn player_alloc(len: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; len].into_boxed_slice()).cast()
}

fn take(data: *mut u8, len: usize) -> Vec<u8> {
    if len == 0 {
        return Vec::new();
    }
    // SAFETY: the host got `data` from `player_alloc(len)` and hands it back exactly once.
    unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(data, len)) }.into_vec()
}

#[unsafe(no_mangle)]
pub extern "C" fn player_new() -> u32 {
    let mut ps = PLAYERS.0.borrow_mut();
    ps.push(Some(Player::new()));
    ps.len() as u32 - 1
}

#[unsafe(no_mangle)]
pub extern "C" fn player_free(id: u32) {
    if let Some(slot) = PLAYERS.0.borrow_mut().get_mut(id as usize) {
        *slot = None;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn player_use(id: u32, plugin: u32, arg: f64) {
    with(id, |p| match plugin {
        0 => p.add(Keyboard { step: arg }),
        1 => p.add(Loop),
        2 => p.add(StartAt::new(arg)),
        3 => p.add(AbLoop::default()),
        4 => p.add(Hls::default()),
        _ => {}
    });
}

/// Returns how many commands the event produced, so the host knows if it was handled.
#[unsafe(no_mangle)]
pub extern "C" fn player_event(id: u32, code: u32, value: f64, data: *mut u8, len: usize) -> u32 {
    let Some(event) = decode(code, value, take(data, len)) else { return 0 };
    // Borrow ends before calling out, so the host may safely call back in.
    let Some((cmds, v)) = with(id, |p| (p.dispatch(event), p.state.view())) else { return 0 };
    unsafe {
        for c in &cmds {
            let (code, value, data) = encode(c);
            cmd(id, code, value, data.as_ptr(), data.len());
        }
        render(id, v.playing, v.waiting, v.muted, v.progress, v.buffered, v.volume, v.label.as_ptr(), v.label.len());
    }
    cmds.len() as u32
}

fn decode(code: u32, v: f64, data: Vec<u8>) -> Option<Event> {
    use {Event::*, Fact as F, Intent as I};
    Some(match code {
        0 => Do(I::Toggle),
        1 => Do(I::Play),
        2 => Do(I::Pause),
        3 => Do(I::Seek(v)),
        4 => Do(I::SeekBy(v)),
        5 => Do(I::SeekFrac(v)),
        6 => Do(I::Volume(v)),
        7 => Do(I::VolumeBy(v)),
        8 => Do(I::ToggleMute),
        9 => Do(I::ToggleFullscreen),
        10 => Do(I::Rate(v)),
        11 => Key(char::from_u32(v as u32)?),
        12 => Saw(F::Time(v)),
        13 => Saw(F::Duration(v)),
        14 => Saw(F::Buffered(v)),
        15 => Saw(F::Playing),
        16 => Saw(F::Paused),
        17 => Saw(F::Ended),
        18 => Saw(F::Waiting),
        19 => Saw(F::Volume(v)),
        20 => Saw(F::Muted(v != 0.0)),
        21 => Saw(F::Rate(v)),
        22 => Saw(F::Fullscreen(v != 0.0)),
        23 => Saw(F::Loaded { ms: v, data }),
        24 => Saw(F::LoadFailed),
        25 => Do(I::Load(String::from_utf8(data).ok()?)),
        _ => return None,
    })
}

fn encode(c: &Cmd) -> (u32, f64, &[u8]) {
    match c {
        Cmd::Play => (0, 0.0, &[]),
        Cmd::Pause => (1, 0.0, &[]),
        Cmd::Seek(t) => (2, *t, &[]),
        Cmd::Volume(v) => (3, *v, &[]),
        Cmd::Muted(m) => (4, f64::from(u8::from(*m)), &[]),
        Cmd::Rate(r) => (5, *r, &[]),
        Cmd::Fullscreen(f) => (6, f64::from(u8::from(*f)), &[]),
        Cmd::Fetch(url) => (7, 0.0, url.as_bytes()),
        Cmd::Open(duration) => (8, *duration, &[]),
        Cmd::Codec(mime) => (9, 0.0, mime.as_bytes()),
        Cmd::Append(data) => (10, 0.0, data),
        Cmd::Remove(end) => (11, *end, &[]),
        Cmd::End => (12, 0.0, &[]),
        Cmd::Source(url) => (13, 0.0, url.as_bytes()),
    }
}
