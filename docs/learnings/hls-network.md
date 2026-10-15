# HLS and the network

- **Segments must be aligned across qualities** for switching. With ffmpeg: fixed keyframe interval
  (`-g`, `-keyint_min`, `-sc_threshold 0`), see `make-hls.sh`.
- **Content types:** `application/vnd.apple.mpegurl` and `video/mp2t`. Default static servers send others,
  which Safari can reject.
- **CORS:** the Rust engine downloads with `fetch()`, so other sites' links need CORS headers. The server's
  `/proxy/<url>` fetches for the page and rewrites playlists so every URI goes through it too.
- **Resolve URIs against an absolute URL**, and only treat `scheme://` as absolute: `seg.ts?next=http://…`
  and `/proxy/https://…` are paths.
- **An HTML error page instead of a playlist** must count as a failed download, or the engine re-asks forever.

## Adaptive bitrate

Throughput-based: measure each segment download, drop the estimate at once after a slow one, climb
gradually, and pick the best quality within 80 % of the estimate. One noisy slow download costs a
lower-quality segment, and it ignores the buffer level.
