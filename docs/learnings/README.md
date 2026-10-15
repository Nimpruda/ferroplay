# What we learned

Design notes, measurements and the traps we hit. Measurements are from 2026-10-15 (Chrome 155 and
Firefox 130 on Linux, headless) unless noted.

- [Decoding is not our job](decoding.md): what browsers play, and why HLS means repackaging, not decoding.
- [Small wasm](wasm-size.md): what made the binary small, and allocators compared.
- [One wasm file per plugin](plugin-modules.md): memory sharing is the crux; decide per plugin.
- [Media Source traps](media-source.md): the bugs we hit and their fixes.
- [Live streams in Media Source](live-media-source.md): Infinity duration, the seekable window, and what Firefox reports.
- [HLS and the network](hls-network.md): segments, content types, CORS, URIs, adaptive bitrate.
- [Testing in real browsers without Node](testing.md): the stdlib Python drivers and their gotchas.
