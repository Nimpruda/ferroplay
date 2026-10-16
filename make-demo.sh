#!/bin/sh
# Makes the demo media that is missing: the HLS stream (make-hls.sh) and the test MP4 (make-media.sh).
# Delete web/hls or web/media to remake it.
set -e
cd "$(dirname "$0")"
[ -f web/hls/master.m3u8 ] || ./make-hls.sh
[ -f web/media/test.mp4 ] || ./make-media.sh
