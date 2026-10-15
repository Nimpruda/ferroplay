# Decoding is not our job

The browser decodes H.264/AAC, in hardware where it can, through `<video>`, Media Source and WebCodecs alike.
JS/WASM cannot reach the GPU's video decoder, and WebGPU has no decode API. Writing H.264 as shaders is not
realistic (its entropy decoding is sequential).

So playing HLS is about *repackaging*, not decoding: turn MPEG-TS into fragmented MP4 and hand it to
Media Source. WASM decoding (ffmpeg) only makes sense for codecs browsers lack (MPEG-2 video, AC-3), and
costs CPU and battery.

| | Built-in HLS | Media Source accepts TS | Media Source accepts fMP4 |
|---|---|---|---|
| Chrome 155 | yes (played 1080p) | yes | yes |
| Firefox 130 | no | no | yes |

Safari and iOS have built-in HLS (iPhone: Media Source only as ManagedMediaSource, iOS 17.1+).
Neither browser here played HEVC, AC-3 or MPEG-2 video.

Hence the design: built-in HLS first, our Rust engine elsewhere.
