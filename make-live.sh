#!/bin/sh
# Runs a live HLS stream in web/hls/live/ until you stop it (Ctrl-C). Real time (-re), 4 s MPEG-TS
# segments, a sliding window of 6 (old segments are deleted), and no EXT-X-ENDLIST while it runs.
# Stopping it (Ctrl-C) ends the playlist, so it turns into a finished VOD stream, which is a test too.
# Input: a file to loop (./make-live.sh clip.mp4), or a test pattern with a tone by default.
set -e
cd "$(dirname "$0")"
input="${1:-}"
if [ -n "$input" ]; then
  source_args="-stream_loop -1 -i $input"
else
  source_args="-f lavfi -i testsrc=size=640x360:rate=25 -f lavfi -i sine=frequency=440"
fi
mkdir -p web/hls/live
# shellcheck disable=SC2086
exec ffmpeg -loglevel error -re $source_args \
  -c:v libx264 -preset veryfast -pix_fmt yuv420p -g 100 -keyint_min 100 -sc_threshold 0 \
  -c:a aac -b:a 96k -ac 2 \
  -f hls -hls_time 4 -hls_list_size 6 -hls_flags delete_segments \
  -hls_segment_type mpegts -hls_segment_filename 'web/hls/live/seg%05d.ts' \
  web/hls/live/live.m3u8
