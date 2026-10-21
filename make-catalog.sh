#!/bin/sh
# Makes the test media catalog in web/catalog/ for checks by hand on the demo page: a folder per entry,
# and catalog.json saying what each entry is and what it should do in each browser.
# Sources are plug-ins: catalog/sources/<name>.sh defines source_<name>, which takes the seconds then
# the ffmpeg output options and runs ffmpeg on its own inputs. synthetic: a test pattern with a tone (no
# network); clip: a cut of the Sintel trailer, downloaded once to .cache/, where the content matters.
# To add a source: one new file in catalog/sources/, nothing else.
# To add an entry: `start <id>`, set its fields (see `add`), make its media in "$dir" with `encode`, `add`.
set -e
cd "$(dirname "$0")"
for file in catalog/sources/*.sh; do
  # shellcheck source=/dev/null
  . "$file"
done
rm -rf web/catalog
mkdir -p web/catalog
entries=

# A JSON string, or null when empty. The fields are ours, so quotes and backslashes are all to escape.
str() {
  if [ -n "$1" ]; then printf '"%s"' "$(printf %s "$1" | sed 's/[\\"]/\\&/g')"; else printf null; fi
}

# A JSON array of the space-separated words in $1, or null when there are none.
array() {
  [ -n "$1" ] || { printf null; return; }
  list=
  for word in $1; do list="${list:+$list, }$(str "$word")"; done
  printf '[%s]' "$list"
}

# Starts entry $1: its folder ($dir; $path from web/, for urls) and its fields, all empty.
start() {
  id=$1 path=catalog/$1 dir=web/catalog/$1
  title='' url='' delivery='' container='' codecs='' axis='' feature='' fault='' source='' plugins='' firefox='' chrome=''
  mkdir -p "$dir"
}

# ffmpeg on the entry's source (catalog/sources/), cut to $1 seconds; the other arguments are the
# output options.
encode() {
  command -v "source_$source" >/dev/null || { echo "$id: unknown source '$source'" >&2 && exit 1; }
  "source_$source" "$@"
}

# Lists the entry in catalog.json. Fields: title; url (from web/); delivery (file, hls); container;
# codecs (space-separated); axis (container, codec, feature, fault: the page groups by it); feature or
# fault (what the entry shows); source (a file in catalog/sources/); plugins (space-separated): the
# player plugins the entry needs, which the page ticks, exactly those, before playing it (say, hls
# without gaps to show a stall), or empty to play it with whatever is ticked; firefox and chrome: the
# expected outcome, `ends` (plays to the end) or `fails: <the message>`.
add() {
  : "${title:?}" "${url:?}" "${delivery:?}" "${container:?}" "${codecs:?}" "${axis:?}" "${source:?}" "${firefox:?}" "${chrome:?}"
  entries="${entries:+$entries,
}  {
    \"id\": $(str "$id"), \"title\": $(str "$title"), \"url\": $(str "$url"),
    \"delivery\": $(str "$delivery"), \"container\": $(str "$container"), \"codecs\": $(array "$codecs"),
    \"axis\": $(str "$axis"), \"feature\": $(str "$feature"), \"fault\": $(str "$fault"), \"source\": $(str "$source"),
    \"plugins\": $(array "$plugins"), \"expect\": {\"firefox\": $(str "$firefox"), \"chrome\": $(str "$chrome")}
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
