#!/bin/sh
# Makes a real HLS stream in web/hls/: the Sintel trailer (read straight from blender.org) in
# three qualities, H.264 + AAC in 4 s MPEG-TS segments. Fixed 2 s keyframes keep the segments
# of all qualities aligned, which quality switching needs.
set -e
cd "$(dirname "$0")"
rm -rf web/hls
mkdir -p web/hls
ffmpeg -loglevel error -i https://download.blender.org/durian/trailer/sintel_trailer-480p.mp4 \
  -filter_complex '[0:v]split=3[a][b][c];[a]scale=-2:240[240];[b]scale=-2:360[360];[c]null[480]' \
  -map '[240]' -map '[360]' -map '[480]' -map 0:a -map 0:a -map 0:a \
  -c:v libx264 -preset veryfast -profile:v main -g 48 -keyint_min 48 -sc_threshold 0 \
  -b:v:0 300k -b:v:1 700k -b:v:2 1400k -c:a aac -b:a 96k -ac 2 \
  -f hls -hls_time 4 -hls_playlist_type vod -hls_segment_type mpegts \
  -hls_segment_filename 'web/hls/%v/%03d.ts' -master_pl_name master.m3u8 \
  -var_stream_map 'v:0,a:0,name:240p v:1,a:1,name:360p v:2,a:2,name:480p' 'web/hls/%v/index.m3u8'
ls web/hls
