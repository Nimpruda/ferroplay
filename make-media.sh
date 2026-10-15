#!/bin/sh
# Makes a plain MP4 in web/media/ for the test buttons: a 20 s test pattern with a tone, H.264 + AAC,
# no network needed. Faststart puts the index first so seeking works from the start.
set -e
cd "$(dirname "$0")"
mkdir -p web/media
ffmpeg -loglevel error -y -f lavfi -i testsrc=size=640x360:rate=25 -f lavfi -i sine=frequency=440 \
  -t 20 -c:v libx264 -preset veryfast -pix_fmt yuv420p -g 50 -c:a aac -b:a 96k -movflags +faststart \
  web/media/test.mp4
ls -l web/media
