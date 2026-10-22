#!/bin/sh
# Makes the demo media that is missing: the HLS stream (make-hls.sh) and the test media catalog
# (make-catalog.sh). Delete web/hls or web/catalog to remake it. The catalog is also remade when
# make-catalog.sh or a file under catalog/ is newer, since entries and sources are added over time.
set -e
cd "$(dirname "$0")"
[ -f web/hls/master.m3u8 ] || ./make-hls.sh
if [ ! -f web/catalog/catalog.json ] || [ -n "$(find make-catalog.sh catalog -newer web/catalog/catalog.json)" ]; then
  ./make-catalog.sh
fi
