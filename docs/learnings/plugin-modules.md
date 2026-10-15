# One wasm file per plugin: memory sharing is the crux

Separate wasm modules do not share memory. So each one carries its own allocator and runtime, and data
crossing between them is copied through JS. Measured (raw / gzipped):

| Module | Size |
|---|---|
| everything in one (today) | 46.8 / 19.9 KB |
| core alone, no HLS | 16.0 / 6.5 KB |
| HLS, own memory and allocator | 41.0 / 17.6 KB |
| HLS, sharing the core's memory and allocator | 35.0 / 15.4 KB |
| keyboard, no allocator at all | 0.4 / 0.3 KB |

- An MP4-only page would download a third of today's file.
- Sharing memory removes the duplicated allocator (~6 KB per module). What stays duplicated (~4 KB) is
  generic library code: Vec growth, UTF-8, panic paths.
- The shared-memory variant was only measured. Running it needs each module's static data and stack placed
  apart in the shared memory (wasm-ld `--global-base`, stack placement).

## Decide per plugin

Plugins that only turn events into intents (keyboard, loop, A-B loop, start-at) need no allocator and cost a
few hundred bytes. Plugins with unbounded data (playlists, multi-MB segments) borrow the core's allocator
through a shared memory.

Running without any allocator (buffers sized upfront) is possible in safe Rust but brittle: segments range
from 100 KB to 20+ MB, and playlists can be any length. Reusing a few buffers gets most of the benefit.
