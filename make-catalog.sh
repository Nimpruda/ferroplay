#!/bin/sh
# Makes the test media catalog in web/catalog/ for checks by hand on the demo page: a folder per entry,
# and catalog.json saying what each entry is and what it should do in each browser.
# Sources are plug-ins: catalog/sources/<name>.sh defines source_<name>, which takes the seconds then
# the ffmpeg output options and runs ffmpeg on its own inputs. synthetic: a test pattern with a tone (no
# network); clip: a cut of the Sintel trailer, downloaded once to .cache/, where the content matters.
# To add a source: one new file in catalog/sources/, nothing else.
# To add an entry: `start <id>`, set its fields (see `add`), make its media in "$dir" with `encode` (or
# `remux`, when only the container differs from media made already; `hls_ts` or `hls_fmp4` in front of
# either makes an HLS VOD), `add`. Another entry for the same
# media: change the id and the fields that differ, `add` again.
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

# The streams of $base (the first entry's MP4) copied, not encoded, into another container: the
# arguments are the output options (-vn or -an first to keep one track). remux_ts and remux_fmp4 make
# an HLS VOD in 4 s segments in "$dir".
remux() {
  ffmpeg -loglevel error -y -i "$base" -c copy "$@"
}
remux_ts() {
  hls_ts remux "$@"
}
remux_fmp4() {
  hls_fmp4 remux "$@"
}

# Runs a command that ends in ffmpeg output options (`encode <seconds> ...`, `remux ...`) into an HLS
# VOD in 4 s segments in "$dir": MPEG-TS or fMP4.
hls_ts() {
  "$@" -f hls -hls_time 4 -hls_playlist_type vod -hls_segment_filename "$dir/%03d.ts" "$dir/index.m3u8"
}
hls_fmp4() {
  "$@" -f hls -hls_time 4 -hls_playlist_type vod -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4 \
    -hls_segment_filename "$dir/%03d.m4s" "$dir/index.m3u8"
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

# Containers. The faststart MP4 is the one synthetic encode the remuxes copy; its keyframes every 2 s
# let HLS cut it in 4 s segments.

# Progressive MP4, index (moov) first: plays and seeks before the whole file is in. The page's "MP4,
# local" button and its ?core=1 default play it.
start mp4-faststart-h264-aac
title='MP4, faststart, H.264 + AAC' url=$path/video.mp4 delivery=file container=mp4 codecs='h264 aac'
axis=container feature='Progressive, 20 s, index first' source=synthetic firefox=ends chrome=ends
encode 20 -c:v libx264 -preset veryfast -pix_fmt yuv420p -g 50 -c:a aac -b:a 96k -ac 2 -movflags +faststart \
  "$dir/video.mp4"
add
base=$dir/video.mp4

# The same, index (moov) last: the browser reads the end of the file before it can start.
start mp4-h264-aac
title='MP4, index last, H.264 + AAC' url=$path/video.mp4 delivery=file container=mp4 codecs='h264 aac'
axis=container feature='Progressive, 20 s, index at the end' source=synthetic firefox=ends chrome=ends
remux "$dir/video.mp4"
add

# HLS with MPEG-TS segments: what the Rust engine was first built for.
start hls-ts-h264-aac
title='HLS, MPEG-TS, H.264 + AAC' url=$path/index.m3u8 delivery=hls container=mpegts codecs='h264 aac'
axis=container feature='VOD, 20 s in 4 s segments' source=synthetic firefox=ends chrome=ends
remux_ts
add

# HLS with fMP4 segments: an init section (EXT-X-MAP), then segments the browser takes as they are.
start hls-fmp4-h264-aac
title='HLS, fMP4, H.264 + AAC' url=$path/index.m3u8 delivery=hls container=fmp4 codecs='h264 aac'
axis=container feature='VOD, 20 s in 4 s segments' source=synthetic firefox=ends chrome=ends
remux_fmp4
add

# One track only, in each segment type.
start hls-ts-audio
title='HLS, MPEG-TS, audio only (AAC)' url=$path/index.m3u8 delivery=hls container=mpegts codecs=aac
axis=container feature='VOD, 20 s, no video track' source=synthetic firefox=ends chrome=ends
remux_ts -vn
add

start hls-fmp4-audio
title='HLS, fMP4, audio only (AAC)' url=$path/index.m3u8 delivery=hls container=fmp4 codecs=aac
axis=container feature='VOD, 20 s, no video track' source=synthetic firefox=ends chrome=ends
remux_fmp4 -vn
add

start hls-ts-video
title='HLS, MPEG-TS, video only (H.264)' url=$path/index.m3u8 delivery=hls container=mpegts codecs=h264
axis=container feature='VOD, 20 s, no audio track' source=synthetic firefox=ends chrome=ends
remux_ts -an
add

start hls-fmp4-video
title='HLS, fMP4, video only (H.264)' url=$path/index.m3u8 delivery=hls container=fmp4 codecs=h264
axis=container feature='VOD, 20 s, no audio track' source=synthetic firefox=ends chrome=ends
remux_fmp4 -an
add

# Three qualities of fMP4 under a master playlist; 2 s keyframes keep their segments aligned.
start hls-fmp4-qualities
title='HLS, fMP4, 3 qualities, H.264 + AAC' url=$path/master.m3u8 delivery=hls container=fmp4 codecs='h264 aac'
axis=container feature='VOD, 12 s, 180p, 270p and 360p' source=synthetic firefox=ends chrome=ends
encode 12 -filter_complex '[0:v]split=3[a][b][c];[a]scale=-2:180[180];[b]scale=-2:270[270];[c]null[360]' \
  -map '[180]' -map '[270]' -map '[360]' -map 1:a -map 1:a -map 1:a \
  -c:v libx264 -preset veryfast -pix_fmt yuv420p -g 50 -b:v:0 200k -b:v:1 400k -b:v:2 800k -c:a aac -b:a 96k -ac 2 \
  -f hls -hls_time 4 -hls_playlist_type vod -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4 \
  -hls_segment_filename "$dir/%v/%03d.m4s" -master_pl_name master.m3u8 \
  -var_stream_map 'v:0,a:0,name:180p v:1,a:1,name:270p v:2,a:2,name:360p' "$dir/%v/index.m3u8"
add

# Real content in fMP4 with B-frames: Media Source ignores the edit list that hides the B-frame delay,
# so the buffer starts 83 ms in. Firefox was seen waiting at that gap without the gaps plugin;
# headless Firefox 130 plays on, so here it is, with the gaps plugin left out, to try by hand.
start hls-fmp4-clip
title='HLS, fMP4, Sintel with B-frames, no gaps plugin' url=$path/index.m3u8 delivery=hls container=fmp4
codecs='h264 aac' axis=container feature='VOD, 12 s, B-frames: the buffer starts 83 ms in' source=clip plugins=hls
firefox=ends chrome=ends
encode 12 -c:v libx264 -preset veryfast -bf 3 -g 48 -c:a aac -b:a 96k -ac 2 \
  -f hls -hls_time 4 -hls_playlist_type vod -hls_segment_type fmp4 -hls_fmp4_init_filename init.mp4 \
  -hls_segment_filename "$dir/%03d.m4s" "$dir/index.m3u8"
add

# The same media and fields (no `start`), with the gaps plugin.
id=hls-fmp4-clip-gaps title='HLS, fMP4, Sintel with B-frames, gaps plugin' plugins='hls gaps'
feature='VOD, 12 s, B-frames: the buffer starts 83 ms in, jumped'
add

# Progressive WebM, VP9 + Opus: both browsers play it themselves.
start webm-vp9-opus
title='WebM, VP9 + Opus' url=$path/video.webm delivery=file container=webm codecs='vp9 opus'
axis=container feature='Progressive, 10 s' source=synthetic firefox=ends chrome=ends
encode 10 -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 500k -c:a libopus -b:a 96k "$dir/video.webm"
add

# Codecs our HLS engine does not play (Firefox's case: Chrome plays HLS itself), each to fail naming
# the codec. Outcomes as seen in headless Firefox 130 and Chrome 156. No HE-AAC: ffmpeg's aac encoder
# has no HE profile, and few ffmpeg builds have libfdk_aac. 10 s each; where only the audio codec
# differs, the H.264 of $base is copied and only the audio encoded.

# The browser plays the file itself, or does not: no engine on the way.
start mp4-hevc-aac
title='MP4, faststart, HEVC + AAC' url=$path/video.mp4 delivery=file container=mp4 codecs='hevc aac'
axis=codec feature='Progressive HEVC (hvc1): the browser decodes it or not' source=synthetic
firefox='fails: NS_ERROR_DOM_MEDIA_METADATA_ERR, nothing on the player'
chrome='ends, sound only: no picture (no HEVC decoder) and no error'
encode 10 -c:v libx265 -preset ultrafast -x265-params log-level=error -tag:v hvc1 -pix_fmt yuv420p -g 50 \
  -c:a aac -b:a 96k -ac 2 -movflags +faststart "$dir/video.mp4"
add
hevc=$dir/video.mp4

# A codec in an fMP4 init section: the engine gives up on it at once, naming its sample entry. The
# MP4's streams, copied.
start hls-fmp4-hevc
title='HLS, fMP4, HEVC + AAC' url=$path/index.m3u8 delivery=hls container=fmp4 codecs='hevc aac'
axis=codec feature='HEVC video (hvc1)' source=synthetic
firefox='fails: init section: hvc1 is a codec this player cannot play'
chrome="fails: DECODER_ERROR_NOT_SUPPORTED (Chrome's HLS, no HEVC decoder), nothing on the player"
hls_fmp4 ffmpeg -loglevel error -y -i "$hevc" -c copy
add

# Audio in MPEG-TS that the engine does not take: it drops the track and plays the picture.
start hls-ts-ac3
title='HLS, MPEG-TS, H.264 + AC-3' url=$path/index.m3u8 delivery=hls container=mpegts codecs='h264 ac3'
axis=codec feature='AC-3 audio' source=synthetic
firefox='ends, picture only: the AC-3 track is dropped without a message'
chrome="ends, picture only: Chrome's HLS has no AC-3 decoder"
hls_ts remux -t 10 -c:a ac3 -b:a 192k -ac 2
add

start hls-ts-eac3
title='HLS, MPEG-TS, H.264 + E-AC-3' url=$path/index.m3u8 delivery=hls container=mpegts codecs='h264 eac3'
axis=codec feature='E-AC-3 audio' source=synthetic
firefox='ends, picture only: the E-AC-3 track is dropped without a message'
chrome="ends, picture only: Chrome's HLS has no E-AC-3 decoder"
hls_ts remux -t 10 -c:a eac3 -b:a 192k -ac 2
add

start hls-ts-mp3
title='HLS, MPEG-TS, H.264 + MP3' url=$path/index.m3u8 delivery=hls container=mpegts codecs='h264 mp3'
axis=codec feature='MP3 audio' source=synthetic
firefox='ends, picture only: the MP3 track is dropped without a message' chrome=ends
hls_ts remux -t 10 -c:a libmp3lame -b:a 128k -ac 2
add

# Packed audio: raw MP3 segments, no container around them. ffmpeg's hls muxer would wrap MP3 in
# MPEG-TS, so the segment muxer cuts it and writes the playlist.
start hls-mp3-packed
title='HLS, packed audio, MP3' url=$path/index.m3u8 delivery=hls container=mp3 codecs=mp3
axis=codec feature='Audio-only playlist of .mp3 segments' source=synthetic
firefox='each segment skipped as not MPEG-TS; sits at the end, not ended, nothing on the player'
chrome="fails: CHUNK_DEMUXER_ERROR_APPEND_FAILED (Chrome's HLS), nothing on the player"
encode 10 -vn -c:a libmp3lame -b:a 128k -ac 2 -f segment -segment_time 4 -segment_format mp3 \
  -segment_list "$dir/index.m3u8" -segment_list_type m3u8 "$dir/%03d.mp3"
add

start hls-fmp4-opus
title='HLS, fMP4, H.264 + Opus' url=$path/index.m3u8 delivery=hls container=fmp4 codecs='h264 opus'
axis=codec feature='Opus audio' source=synthetic
firefox='fails: init section: Opus is a codec this player cannot play' chrome=ends
hls_fmp4 remux -t 10 -c:a libopus -b:a 96k
add

start hls-fmp4-flac
title='HLS, fMP4, H.264 + FLAC' url=$path/index.m3u8 delivery=hls container=fmp4 codecs='h264 flac'
axis=codec feature='FLAC audio' source=synthetic
firefox='fails: init section: fLaC is a codec this player cannot play' chrome=ends
hls_fmp4 remux -t 10 -c:a flac -ac 2
add

start hls-fmp4-vp9
title='HLS, fMP4, VP9 + AAC' url=$path/index.m3u8 delivery=hls container=fmp4 codecs='vp9 aac'
axis=codec feature='VP9 video' source=synthetic
firefox='fails: init section: vp09 is a codec this player cannot play' chrome=ends
hls_fmp4 encode 10 -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 500k -g 50 -c:a aac -b:a 96k -ac 2
add

start hls-fmp4-av1
title='HLS, fMP4, AV1 + AAC' url=$path/index.m3u8 delivery=hls container=fmp4 codecs='av1 aac'
axis=codec feature='AV1 video' source=synthetic
firefox='fails: init section: av01 is a codec this player cannot play' chrome=ends
hls_fmp4 encode 10 -c:v libaom-av1 -usage realtime -cpu-used 8 -row-mt 1 -tiles 2x2 -b:v 500k -g 50 -c:a aac -b:a 96k -ac 2
add

printf '[\n%s\n]\n' "$entries" > web/catalog/catalog.json
ls web/catalog
