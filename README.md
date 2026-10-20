# player

A video.js replacement: a Rust core compiled to WebAssembly plus a thin DOM shim, with HLS.
About 31 KB gzipped in total (wasm 25 KB, `player.js` 4.9 KB, CSS 1 KB), or 13 KB with the core-only wasm (6.9 KB).

```sh
rustup target add wasm32-unknown-unknown   # once
prek install                               # once: the git hooks in prek.toml (also need hadolint, docker)
./build.sh            # wasm -> web/player.wasm, web/player-core.wasm (and the demo media, if ffmpeg is installed)
cargo run -p server   # http://127.0.0.1:8080
cargo test            # every crate's tests, run natively
python3 e2e/run.py    # real HLS and links through our server, in headless Firefox and Chrome
```

The `justfile` wraps these (`just` lists the recipes). With only Docker installed:

```sh
just up          # server + demo media on http://127.0.0.1:8080, rebuilt from the sources
just up-live     # the same, plus a live stream at /hls/live/live.m3u8
just docker-e2e  # the e2e tests in a container with Firefox and Chromium
just down
docker build --target ts2mp4 -t ts2mp4 . && docker run --rm -i ts2mp4 < seg.ts > seg.mp4
```

The `Dockerfile` compiles dependencies in their own layer (cargo-chef), so a source change only
rebuilds our crates. Media made by the containers lands in `web/hls`, `web/media` and `web/catalog`,
as natively.

Design notes, measurements and the traps we hit: [docs/learnings/](docs/learnings/README.md).

## The page

`http://127.0.0.1:8080` is a playground: paste a link (`.mp4` or `.m3u8`) and tick plugins on
and off; the player is rebuilt in place and keeps its position. Both live in the page URL, so a
setup can be shared: `/?play=https://…/master.m3u8&plugins=keyboard,gaps,hls,speed`. `/?core=1` runs it on
`player-core.wasm`, without the hls checkbox.

Playlists from other sites go through the server's proxy (`/proxy/<url>`), which rewrites them
so every segment does too. So links play even when their site sends no CORS headers.

## Test media catalog

`./make-catalog.sh` (`just catalog`) writes test media to `web/catalog/`, a folder per entry, made
from a synthetic test pattern with a tone or from a cut of the Sintel trailer. `build.sh` and the
docker media service make it when it is missing or older than the script.
`web/catalog/catalog.json` says what each entry is, what it should do in Firefox and Chrome (play
to the end, or fail with a message); `make-catalog.sh` says
how to add an entry and documents the fields.

The page lists the entries under the player, grouped by axis; a click plays one with the ticked
plugins. It is for checks by hand: the e2e tests keep their own.

## Crates

| Crate | Role | Depends on |
|---|---|---|
| `player` | The core: state, plugins, what to paint. No I/O. | – |
| `ts2mp4` | MPEG-TS (H.264 + AAC) to fragmented MP4. Also a command: `cargo run -p ts2mp4 < seg.ts > seg.mp4`. | – |
| `hls` | HLS playback as a player plugin, plus M3U8 helpers (`playlist`). | `player`, `ts2mp4` |
| `player-wasm` | The browser build: a code, a number and optional bytes each way. The only `unsafe`. | `player`, `hls` (feature `hls`, default) |
| `server` | Serves `web/` (with HLS content types) and the proxy. | `hls` |

The libraries are `no_std` and dependency-free, so they run (and are tested) anywhere.

## How it works

The browser's `<video>` element still decodes video. Rust owns everything else:

```
Event ──▶ State.apply(fact) ──▶ plugins ──▶ Intents ──▶ State.resolve ──▶ Cmds ──▶ host
```

- **Facts** (`Fact`): what the host reported. The only thing that changes state.
- **Intents** (`Intent`): what someone wants, relative ("seek +5 s"), or an exact `Cmd`.
- **Commands** (`Cmd`): what the host must do, absolute ("seek to 42 s", "fetch this URL").
- **Plugins**: `fn on(&mut self, &State, &Event, &mut Vec<Intent>)`. That's the whole API. A plugin
  sees the intents asked for so far, so it can add, change or drop them (`Hls` takes over `Load`
  for `.m3u8` URLs).

`web/player.js` forwards DOM events in, runs commands, paints what the core says.

## Usage

```html
<video id="v" src="https://…/master.m3u8"></video>
<script type="module">
  import { init, player, defaults } from './player.js';
  import { speed } from './plugins/speed.js';
  import { hls } from './plugins/hls.js';
  await init();
  const p = player(document.getElementById('v'), { ...defaults, abLoop: true, speed, hls });
  p.play('https://…/movie.mp4'); // switch source and play; also p.load(url), p.play(), p.pause()
  p.destroy();
</script>
```

A source set in HTML goes through the plugins like `p.load(url)` does, so `hls` picks up `.m3u8`.

`build.sh` writes two cores. `player.wasm` has everything. `player-core.wasm` has no Rust HLS engine (a
quarter of the download): pick it for pages that play MP4, or HLS only in browsers that play it themselves,
with `await init(new URL('player-core.wasm', import.meta.url))`. `use('hls')` on it throws
`plugin "hls" is not in this build`.
No plugin is implied: `defaults` is `{ keyboard: 5, gaps: true }`, and `false` leaves one out
(`{ ...defaults, gaps: false }`).

Rust plugins: `Keyboard { step }`, `Gaps { start, hole }` (jumps the small gaps in the buffer that
the element would wait at), `Loop`, `StartAt::new(t)`, `AbLoop` (`player::plugins`), `Hls`.
JS plugins are functions `({ root, video, send, use, handle, load, play, pause }) => cleanup?`:

- `send(event, value, bytes?)` reports to the core,
- `use(name, arg)` turns on a Rust plugin,
- `handle({ command: (value, bytes) => … })` carries out host commands a Rust plugin issues.

From Rust, any closure is a plugin:

```rust
let p = Player::new()
    .with(Keyboard { step: 5.0 })
    .with(Loop)
    .with(|_: &State, e: &Event, out: &mut Vec<Intent>| {
        if *e == Event::Key('s') { out.push(Intent::Seek(90.0)) } // skip intro
    });
```

To expose a new Rust plugin to JS, add a code to `player_use` in `crates/wasm/src/ffi.rs` and its
name to `PLUGINS` in `web/player.js`.

## HLS

With `hls` installed, browsers with built-in HLS (Safari, iOS, Android, recent Chrome/Edge) get
the `.m3u8` URL as `video.src`. Elsewhere (Firefox, older browsers) the Rust `Hls` plugin takes over:

- loads the playlists, keeps 30 s ahead of the playhead and drops what is 30 s behind,
- converts each segment with `ts2mp4` and feeds it to Media Source,
- picks the best quality that fits 80 % of the measured bandwidth: drops at once after a slow
  download, climbs gradually,
- after a seek, abandons a download that is no longer needed.

The JS side only downloads and calls Media Source; it does what the Rust side asks.

Not supported yet: encryption (`#EXT-X-KEY`), separate audio renditions (`#EXT-X-MEDIA`), discontinuities, HEVC.

## Tests

`cargo test` covers the core, the converter (a real ffmpeg-made TS file in `fixtures/`),
playlists, and the HLS plugin's decisions with a fake host.

`./make-live.sh [clip]` runs a live stream in `web/hls/live/` (or `$OUT`) until you stop it (Ctrl-C). It is a sliding
window with no `ENDLIST`, and stopping it ends the playlist, so it can also test the switch to VOD.

`./make-hls.sh` makes the demo stream in `web/hls/`: the Sintel trailer, read straight from
blender.org, as HLS with MPEG-TS segments in 240p, 360p and 480p. `e2e/run.py` (Python stdlib
only) then checks, with our server:

- every segment converts and ffmpeg decodes it,
- HLS content types, and the proxy rewriting playlists,
- in Firefox and Chrome (Rust engine and Chrome's own HLS): start, quality climb, seek, end, no errors,
- in Firefox and Chrome, the fMP4 fixtures (a buffer that starts at 66 ms) play to the end,
- a cross-origin playlist link without CORS plays via the proxy, plugin checkboxes rebuild the
  player and keep the position, the paste box plays an MP4 link,
- in Chrome, the core-only build plays a pasted MP4 link and throws for `hls`,
- at a throttled 0.6 Mbit/s, playback stays on 240p and keeps going.
