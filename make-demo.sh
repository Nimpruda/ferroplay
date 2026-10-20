#!/bin/sh
# Makes the demo media that is missing: the HLS stream (make-hls.sh), the test MP4 (make-media.sh) and
# the test media catalog (make-catalog.sh). Delete web/hls or web/media to remake it. The catalog is
# also remade when make-catalog.sh is newer, since entries are added to it over time.
set -e
cd "$(dirname "$0")"
[ -f web/hls/master.m3u8 ] || ./make-hls.sh
[ -f web/media/test.mp4 ] || ./make-media.sh
[ web/catalog/catalog.json -nt make-catalog.sh ] || ./make-catalog.sh
