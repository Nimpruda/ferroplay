// DOM bridge for the Rust core: forwards events in, runs commands out, paints renders.
// Code tables must match src/ffi.rs.
const EVENTS = ['toggle', 'play', 'pause', 'seek', 'seekBy', 'seekFrac', 'volume', 'volumeBy',
  'toggleMute', 'toggleFullscreen', 'rate', 'key', 'time', 'duration', 'buffered', 'playing',
  'paused', 'ended', 'waiting', 'volumeChanged', 'muted', 'rateChanged', 'fullscreen',
  'loaded', 'loadFailed', 'load', 'seekable', 'seeking'];
const CMDS = ['play', 'pause', 'seek', 'volume', 'muted', 'rate', 'fullscreen',
  'fetch', 'open', 'codec', 'append', 'remove', 'end', 'source', 'seekableRange', 'warn', 'fail'];
const PLUGINS = ['keyboard', 'loop', 'startAt', 'abLoop', 'hls', 'gaps'];
// Tuning knobs, by name (the names are `Knob::name` in crates/hls). A knob's function gets the target
// duration (seconds) or nothing; it returns the value, or nothing to keep the default. Names not listed here
// throw when registered: a typo must not silently keep the default.
const KNOBS = ['ahead', 'behind', 'holdback', 'safety', 'retries', 'liveRetries', 'initRetries'];
const ARROWS = { ArrowLeft: '←', ArrowUp: '↑', ArrowRight: '→', ArrowDown: '↓' };

const icon = (cls, d) => `<svg class="${cls}" viewBox="0 0 24 24"><path d="${d}"/></svg>`;
const SPEAKER = 'M3 9v6h4l5 5V4L7 9z';
const BAR = `
  <button data-do="toggle" aria-label="Play/pause">
    ${icon('vp-i-play', 'M8 5v14l11-7z')}${icon('vp-i-pause', 'M6 5h4v14H6zm8 0h4v14h-4z')}
  </button>
  <input class="vp-seek" type="range" min="0" max="1" step="any" value="0" aria-label="Seek">
  <span class="vp-time"></span>
  <button data-do="toggleMute" aria-label="Mute">
    ${icon('vp-i-vol', SPEAKER + 'm13.5 3A4.5 4.5 0 0 0 14 8v8a4.5 4.5 0 0 0 2.5-4z')}
    ${icon('vp-i-mute', SPEAKER + 'm11.6 0 2.4 2.4 2.4-2.4 1.4 1.4-2.4 2.4 2.4 2.4-1.4 1.4-2.4-2.4-2.4 2.4-1.4-1.4 2.4-2.4-2.4-2.4z')}
  </button>
  <input class="vp-vol" type="range" min="0" max="1" step="any" value="1" aria-label="Volume">
  <button data-do="toggleFullscreen" aria-label="Fullscreen">
    ${icon('', 'M4 4h6v2H6v4H4zm10 0h6v6h-2V6h-4zM4 14h2v4h4v2H4zm14 0h2v6h-6v-2h4z')}
  </button>`;

const players = new Map();
const utf8 = new TextDecoder();
let core;

const lookup = (table, name) => {
  const i = table.indexOf(name);
  if (i < 0) throw new Error(`unknown name "${name}", expected one of: ${table.join(', ')}`);
  return i;
};
const settle = promise => promise?.catch(() => {});
// Numbers as 8 little-endian bytes each, read as `f64::from_le_bytes` in Rust.
const toBytes = numbers => {
  const bytes = new DataView(new ArrayBuffer(8 * numbers.length));
  numbers.forEach((x, i) => bytes.setFloat64(8 * i, x, true));
  return new Uint8Array(bytes.buffer);
};

// Bytes into wasm: Rust allocates, JS fills, Rust takes them back with the call.
const pass = bytes => {
  if (!bytes?.length) return [0, 0];
  const ptr = core.player_alloc(bytes.length);
  new Uint8Array(core.memory.buffer, ptr, bytes.length).set(bytes);
  return [ptr, bytes.length];
};

/** Loads the wasm core; call once before creating players. */
export async function init(url = new URL('player.wasm', import.meta.url)) {
  const host = {
    cmd: (id, code, value, ptr, len) => players.get(id).run(code, value, new Uint8Array(core.memory.buffer, ptr, len).slice()),
    render: (id, ...args) => players.get(id).render(...args),
    tune: (id, ptr, len, value) => players.get(id).tune(utf8.decode(new Uint8Array(core.memory.buffer, ptr, len)), value),
  };
  ({ instance: { exports: core } } = await WebAssembly.instantiateStreaming(fetch(url), { host }));
}

/** The plugins most pages want: `player(video, { ...defaults, hls })`, or `{ ...defaults, gaps: false }` without one. */
export const defaults = { keyboard: 5, gaps: true };

/**
 * Wraps a <video> with controls; play something with `.play(url)`. `plugins` maps a Rust plugin
 * name to its numeric argument, or any name to a JS plugin: `({ video, send, use, handle }) => cleanup?`.
 * `false` leaves a plugin out. None are implied: see `defaults`.
 */
export function player(video, plugins = {}) {
  const id = core.player_new();
  const send = (name, value = 0, bytes) => core.player_event(id, lookup(EVENTS, name), +value, ...pass(bytes));
  const use = (name, arg = 0) => core.player_use(id, lookup(PLUGINS, name), +arg);
  const jsPlugins = [];
  const tuners = new Map(); // knob name -> function
  for (const [name, arg] of Object.entries(plugins)) {
    if (arg === false) continue;
    if (typeof arg === 'function') jsPlugins.push(arg);
    else use(name, arg);
  }

  const root = document.createElement('div');
  root.className = 'vp';
  root.tabIndex = 0;
  video.replaceWith(root);
  video.controls = false;
  root.append(video);
  root.insertAdjacentHTML('beforeend', `<div class="vp-bar">${BAR}</div>`);
  const [seek, time, vol] = ['.vp-seek', '.vp-time', '.vp-vol'].map(s => root.querySelector(s));

  // Host commands by name. JS plugins add their own with `handle`.
  const handlers = {
    play: () => settle(video.play()),
    pause: () => video.pause(),
    seek: t => { video.currentTime = t; },
    volume: v => { video.volume = v; },
    muted: m => { video.muted = !!m; },
    rate: r => { video.playbackRate = r; },
    fullscreen: f => settle(f ? root.requestFullscreen?.() : document.exitFullscreen?.()),
    source: (_, bytes) => {
      const url = utf8.decode(bytes); // absolute: see `load`
      if (video.src !== url) {
        video.src = url;
        return;
      }
      // The same source (the one given in HTML, say) is not reloaded, so the element will not report again what
      // the core forgot on the load: report it now.
      lastBuffered = [];
      ['emptied', 'durationchange', 'timeupdate'].forEach(type => media[type]());
    },
    warn: (_, bytes) => console.warn(utf8.decode(bytes)),
    // Playback has stopped: say why on the player, and to the page as an `error` event (detail: the message).
    fail: (_, bytes) => {
      const message = utf8.decode(bytes);
      console.error(message);
      root.dataset.error = message;
      root.dispatchEvent(new CustomEvent('error', { detail: message }));
    },
  };
  const run = (code, value, bytes) => {
    const handler = handlers[CMDS[code]];
    if (handler) handler(value, bytes);
    else console.error(`no handler for command "${CMDS[code]}"`);
  };

  const render = (playing, waiting, muted, progress, buffered, volume, ptr, len) => {
    root.classList.toggle('vp-playing', playing);
    root.classList.toggle('vp-waiting', waiting);
    root.classList.toggle('vp-muted', muted);
    paint(seek, progress, buffered);
    paint(vol, volume, volume);
    time.textContent = utf8.decode(new Uint8Array(core.memory.buffer, ptr, len));
  };

  // Asked by the Rust core, synchronously. Giving nothing keeps the default; nonsense throws, here, where the
  // message can be seen (the core would only trap with no message).
  const tune = (name, value) => {
    const knob = tuners.get(name);
    if (!knob) return NaN;
    const answer = knob(value);
    if (answer === undefined) return NaN;
    if (!(typeof answer === 'number' && Number.isFinite(answer) && answer >= 0)) {
      throw new RangeError(`tuning knob "${name}" gave ${answer}, not a finite, non-negative number`);
    }
    return answer;
  };

  players.set(id, { run, render, tune });

  // While playing, the controls hide 2 s after the last pointer move or key press (see CSS).
  let idle;
  const wake = () => {
    root.classList.add('vp-active');
    clearTimeout(idle);
    idle = setTimeout(() => root.classList.remove('vp-active'), 2000);
  };

  // UI -> core
  root.addEventListener('pointermove', wake);
  root.addEventListener('click', e => {
    const action = e.target.closest('[data-do]')?.dataset.do;
    if (action) send(action);
  });
  video.addEventListener('click', () => send('toggle'));
  video.addEventListener('dblclick', () => send('toggleFullscreen'));
  seek.addEventListener('input', () => send('seekFrac', seek.value));
  vol.addEventListener('input', () => send('volume', vol.value));
  root.addEventListener('keydown', e => {
    wake();
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    // Sliders keep their arrow keys, buttons their space bar.
    if (e.target.matches('input') && e.key in ARROWS || e.target.matches('button') && e.key === ' ') return;
    const key = ARROWS[e.key] ?? (e.key.length === 1 ? e.key.toLowerCase() : '');
    if (key && send('key', key.codePointAt(0))) e.preventDefault();
  });

  // media -> core
  // Every buffered range, sent only when they changed. Media Source may not fire `progress`, so this also runs
  // on `timeupdate`.
  let lastBuffered = [];
  const buffered = () => {
    const b = video.buffered;
    const pairs = [];
    for (let i = 0; i < b.length; i++) pairs.push(b.start(i), b.end(i));
    if (pairs.length === lastBuffered.length && pairs.every((x, i) => x === lastBuffered[i])) return;
    send('buffered', 0, toBytes(lastBuffered = pairs));
  };
  // The seekable range, for any source (a live stream's window, native HLS included). Media Source fires no
  // event when it changes, so it is read on the media events below, and sent only when it moved. The end
  // travels in the bytes.
  let lastSeekable;
  const seekable = () => {
    const r = video.seekable;
    if (!r.length) return;
    const [start, end] = [r.start(0), r.end(r.length - 1)];
    if (lastSeekable?.[0] === start && lastSeekable[1] === end) return;
    lastSeekable = [start, end];
    send('seekable', start, toBytes([end]));
  };
  const media = {
    timeupdate: () => { send('time', video.currentTime); buffered(); seekable(); },
    seeking: () => send('seeking', video.currentTime), // every seek, whoever asked; no timeupdate until it has data
    durationchange: () => { send('duration', video.duration); seekable(); },
    progress: () => { buffered(); seekable(); },
    // A new source: report its window even if it matches, and that the element paused (it fires no `pause` for that).
    emptied: () => { lastSeekable = undefined; buffered(); send(video.paused ? 'paused' : 'playing'); },
    play: () => send('playing'),
    playing: () => send('playing'),
    pause: () => send('paused'),
    ended: () => send('ended'),
    waiting: () => send('waiting'),
    volumechange: () => { send('volumeChanged', video.volume); send('muted', video.muted); },
    ratechange: () => send('rateChanged', video.playbackRate),
  };
  for (const [type, fn] of Object.entries(media)) video.addEventListener(type, fn);
  const onFullscreen = () => send('fullscreen', document.fullscreenElement === root);
  document.addEventListener('fullscreenchange', onFullscreen);

  // Absolute, so playlists resolve their URIs against a real base.
  const load = url => {
    delete root.dataset.error; // a new source, a new chance
    send('load', 0, new TextEncoder().encode(new URL(url, document.baseURI).href));
  };
  const api = {
    load,
    play: url => { if (url) load(url); send('play'); },
    pause: () => send('pause'),
    root, video, send, use,
    tune: (name, knob) => {
      lookup(KNOBS, name); // throws on a name that is not a knob: a typo must not silently keep the default
      if (typeof knob !== 'function') throw new TypeError(`tuning knob "${name}" must be a function, not ${typeof knob}`);
      tuners.set(name, knob);
    },
    // Reports the seekable range now, for a plugin that has just changed it.
    seekable,
    handle: more => Object.assign(handlers, more), destroy,
  };
  const cleanups = jsPlugins.map(plugin => plugin(api)).filter(Boolean);

  // Catch up with whatever the video already knows.
  ['durationchange', 'volumechange', 'ratechange', 'progress', 'timeupdate'].forEach(type => media[type]());
  send(video.paused ? 'paused' : 'playing');

  // A source given in HTML goes through the plugins too, so `<video src="….m3u8">` works with `hls`.
  const html = video.getAttribute('src') ? video.src : video.querySelector('source')?.src;
  if (html) load(html);

  function destroy() {
    cleanups.forEach(cleanup => cleanup());
    clearTimeout(idle);
    for (const [type, fn] of Object.entries(media)) video.removeEventListener(type, fn);
    document.removeEventListener('fullscreenchange', onFullscreen);
    core.player_free(id);
    players.delete(id);
    root.replaceWith(video);
  }

  return api;
}

function paint(input, pos, buf) {
  input.value = pos;
  input.style.setProperty('--pos', pos);
  input.style.setProperty('--buf', buf);
}
