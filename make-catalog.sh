#!/bin/sh
# Makes the test media catalog in web/catalog/ for checks by hand on the demo page: a folder per entry,
# and catalog.json saying what each entry is and what it should do in each browser.
# Sources: a synthetic test pattern with a tone (no network), or a cut of the Sintel trailer (read
# straight from blender.org, like make-hls.sh) where the content matters.
# To add an entry: `start <id>`, set its fields (see `add`), make its media in "$dir" with `encode`, `add`.
set -e
cd "$(dirname "$0")"
rm -rf web/catalog
mkdir -p web/catalog
entries=

# A JSON string, or null when empty. The fields are ours, so quotes and backslashes are all to escape.
str() {
  if [ -n "$1" ]; then printf '"%s"' "$(printf %s "$1" | sed 's/[\\"]/\\&/g')"; else printf null; fi
}

# Starts entry $1: its folder ($dir; $path from web/, for urls) and its fields, all empty.
start() {
  id=$1 path=catalog/$1 dir=web/catalog/$1
  title='' url='' delivery='' container='' codecs='' axis='' feature='' fault='' source='' firefox='' chrome=''
  mkdir -p "$dir"
}

# ffmpeg on the entry's source, cut to $1 seconds; the other arguments are the output options.
# synthetic: a 640x360 test pattern with a 440 Hz tone. clip: the Sintel trailer from 0:14, past the titles.
encode() {
  seconds=$1
  shift
  case $source in
    synthetic) set -- -f lavfi -i testsrc=size=640x360:rate=25 -f lavfi -i sine=frequency=440 -t "$seconds" "$@" ;;
    clip) set -- -ss 14 -i https://download.blender.org/durian/trailer/sintel_trailer-480p.mp4 -t "$seconds" "$@" ;;
    *) echo "$id: unknown source '$source'" >&2 && exit 1 ;;
  esac
  ffmpeg -loglevel error -y "$@"
}

# Lists the entry in catalog.json. Fields: title; url (from web/); delivery (file, hls); container;
# codecs (space-separated); axis (container, codec, feature, fault: the page groups by it); feature or
# fault (what the entry shows); source (synthetic, clip); firefox and chrome: the expected outcome,
# `ends` (plays to the end) or `fails: <the message>`.
add() {
  : "${title:?}" "${url:?}" "${delivery:?}" "${container:?}" "${codecs:?}" "${axis:?}" "${source:?}" "${firefox:?}" "${chrome:?}"
  list=
  for codec in $codecs; do list="${list:+$list, }$(str "$codec")"; done
  entries="${entries:+$entries,
}  {
    \"id\": $(str "$id"), \"title\": $(str "$title"), \"url\": $(str "$url"),
    \"delivery\": $(str "$delivery"), \"container\": $(str "$container"), \"codecs\": [$list],
    \"axis\": $(str "$axis"), \"feature\": $(str "$feature"), \"fault\": $(str "$fault"), \"source\": $(str "$source"),
    \"expect\": {\"firefox\": $(str "$firefox"), \"chrome\": $(str "$chrome")}
  }"
}

# HLS with MPEG-TS segments: what the Rust engine was first built for.
start hls-ts-h264-aac
title='HLS, MPEG-TS, H.264 + AAC' url=$path/index.m3u8 delivery=hls container=mpegts codecs='h264 aac'
axis=container feature='VOD, 12 s in 4 s segments' source=synthetic firefox=ends chrome=ends
encode 12 -c:v libx264 -preset veryfast -pix_fmt yuv420p -g 50 -c:a aac -b:a 96k -ac 2 \
  -f hls -hls_time 4 -hls_playlist_type vod -hls_segment_type mpegts \
  -hls_segment_filename "$dir/%03d.ts" "$dir/index.m3u8"
add

printf '[\n%s\n]\n' "$entries" > web/catalog/catalog.json
ls web/catalog
