//! The wasm bridge: every call carries a code, a number, and optional bytes as (pointer, length).
//! The only `unsafe` in the workspace. Event, command and plugin codes must match `web/player.js`.

use alloc::{borrow::Cow, boxed::Box, string::String, vec, vec::Vec};
use core::cell::RefCell;
use hls::Hls;
use player::plugins::{AbLoop, Gaps, Keyboard, Loop, StartAt};
use player::{Cmd, Event, Fact, Intent, Player, Ranges, View};

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[link(wasm_import_module = "host")]
unsafe extern "C" {
    fn cmd(id: u32, code: u32, value: f64, data: *const u8, len: usize);
    /// The host's answer for the knob called `name`; NaN keeps the default.
    fn tune(id: u32, name: *const u8, name_len: usize, value: f64) -> f64;
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

/// A player and the view the host last painted for it, so an event that changes nothing visible paints nothing.
struct Slot {
    player: Player,
    shown: Option<View>,
}

struct Players(RefCell<Vec<Option<Slot>>>);
// SAFETY: wasm32-unknown-unknown is single-threaded.
unsafe impl Sync for Players {}
static PLAYERS: Players = Players(RefCell::new(Vec::new()));

fn slot<R>(id: u32, f: impl FnOnce(&mut Slot) -> R) -> Option<R> {
    PLAYERS.0.borrow_mut().get_mut(id as usize)?.as_mut().map(f)
}

fn with<R>(id: u32, f: impl FnOnce(&mut Player) -> R) -> Option<R> {
    slot(id, |s| f(&mut s.player))
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
    ps.push(Some(Slot { player: Player::new(), shown: None }));
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
        4 => p.add(Hls::tuned(move |knob| {
            let (name, value) = (knob.name(), knob.target());
            // SAFETY: the host answers synchronously and does not call back into the core.
            let answer = unsafe { tune(id, name.as_ptr(), name.len(), value) };
            (!answer.is_nan()).then_some(answer)
        })),
        5 => p.add(Gaps::default()),
        _ => {}
    });
}

/// Returns how many commands the event produced, so the host knows if it was handled.
#[unsafe(no_mangle)]
pub extern "C" fn player_event(id: u32, code: u32, value: f64, data: *mut u8, len: usize) -> u32 {
    let Some(event) = decode(code, value, take(data, len)) else { return 0 };
    // Borrow ends before calling out, so the host may safely call back in.
    let Some(cmds) = with(id, |p| p.dispatch(event)) else { return 0 };
    for c in &cmds {
        let (code, value, data) = encode(c);
        unsafe { cmd(id, code, value, data.as_ptr(), data.len()) };
    }
    // Painted after the commands: one of them may have reported back (an event within this one), and the
    // view must include it. Gone if a command destroyed the player; skipped if it looks as last painted.
    let fresh = slot(id, |s| Some(s.player.state.view()).filter(|v| s.shown.as_ref() != Some(v))).flatten();
    if let Some(v) = fresh {
        unsafe { render(id, v.playing, v.waiting, v.muted, v.progress, v.buffered, v.volume, v.label.as_ptr(), v.label.len()) };
        slot(id, |s| s.shown = Some(v));
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
        14 => Saw(F::Buffered(Ranges::from_le_bytes(&data)?)),
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
        // The end of the range travels in the bytes, as 8 little-endian bytes (see `encode`).
        26 => Saw(F::Seekable(v, f64::from_le_bytes(data.as_slice().try_into().ok()?))),
        27 => Saw(F::Seeking(v)),
        _ => return None,
    })
}

fn encode(c: &Cmd) -> (u32, f64, Cow<'_, [u8]>) {
    let none = || Cow::Borrowed(&[][..]);
    match c {
        Cmd::Play => (0, 0.0, none()),
        Cmd::Pause => (1, 0.0, none()),
        Cmd::Seek(t) => (2, *t, none()),
        Cmd::Volume(v) => (3, *v, none()),
        Cmd::Muted(m) => (4, f64::from(u8::from(*m)), none()),
        Cmd::Rate(r) => (5, *r, none()),
        Cmd::Fullscreen(f) => (6, f64::from(u8::from(*f)), none()),
        Cmd::Fetch(url, delay) => (7, *delay, Cow::Borrowed(url.as_bytes())),
        Cmd::Open(duration) => (8, *duration, none()),
        Cmd::Codec(mime) => (9, 0.0, Cow::Borrowed(mime.as_bytes())),
        Cmd::Append(data) => (10, 0.0, Cow::Borrowed(data.as_slice())),
        Cmd::Remove(end) => (11, *end, none()),
        Cmd::End => (12, 0.0, none()),
        Cmd::Source(url) => (13, 0.0, Cow::Borrowed(url.as_bytes())),
        Cmd::SeekableRange(start, end) => (14, *start, Cow::Owned(end.to_le_bytes().to_vec())),
        Cmd::Warn(message) => (15, 0.0, Cow::Borrowed(message.as_bytes())),
        Cmd::Fail(message) => (16, 0.0, Cow::Borrowed(message.as_bytes())),
    }
}
