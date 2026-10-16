# Small wasm

| Change | Effect |
|---|---|
| `no_std` + `panic = "abort"` + a one-line panic handler | removes std's panic and formatting machinery |
| `f64::clamp` → `x.max(lo).min(hi)` | 46 → 18 KB: clamp's panic message formats floats |
| hand-rolled decimal parser for `#EXTINF` | avoids `core`'s float parser |
| moving to the workspace (every library crate `no_std`) | 66 → 47 KB, same exports and tests (exact cause not pinned down) |
| talc instead of dlmalloc | −4.5 KB (−10 %) |
| `wasm-opt -Oz` | −13 % more |
| `[a, b].concat()` instead of `format!` for messages to the host | `format!` cost +5 KB (49.8 → 54.8 KB raw), concat +0.9 KB |

Any remaining bounds check keeps a little `core::fmt` alive; chasing the last few KB was not worth `unsafe`.

## Allocators

During real playback with seeks:

| Allocator | Wasm memory | Size (raw / gzipped) |
|---|---|---|
| dlmalloc (today) | 8 MB, flat | 46.8 / 19.9 KB |
| talc | 9 MB | 42.2 / 18.4 KB |
| lol_alloc | grew to 11 MB (fragmentation) | 43.1 / 18.4 KB |

talc + wasm-opt: 36.7 KB / 17.4 KB gzipped, and plays to the end in Chrome and Firefox.
