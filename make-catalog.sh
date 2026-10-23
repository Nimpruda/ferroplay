#!/bin/sh
# Makes the test media catalog in web/catalog/ for checks by hand on the demo page: a folder per entry,
# and catalog.json saying what each entry is and what it should do in each browser.
# Sources are plug-ins: catalog/sources/<name>.sh defines source_<name>, which takes the seconds then
# the ffmpeg output options and runs ffmpeg on its own inputs. synthetic: a test pattern with a tone (no
# network); clip: a cut of the Sintel trailer, downloaded once to .cache/, where the content matters.
# To add a source: one new file in catalog/sources/, nothing else.
# To add an entry: `start <id>`, set its fields (see `add`), make its media in "$dir" with `h264_aac` or
# `encode` (or `remux`, when only the container differs from media made already; `hls_ts` or `hls_fmp4`
# in front of any makes an HLS VOD), `add`. Another entry for the same media: change the id and the
# fields that differ, `add` again.
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

# Entry $1's names ($dir; $path from web/, for urls) and its fields, all empty.
fields() {
  id=$1 path=catalog/$1 dir=web/catalog/$1
  title='' url='' delivery='' container='' codecs='' axis='' feature='' fault='' source='' plugins='' firefox='' chrome=''
}

# Starts entry $1: its fields, and its folder.
start() {
  fields "$1"
  mkdir -p "$dir"
}

# ffmpeg on the entry's source (catalog/sources/), cut to $1 seconds; the other arguments are the
# output options.
encode() {
  command -v "source_$source" >/dev/null || { echo "$id: unknown source '$source'" >&2 && exit 1; }
  "source_$source" "$@"
}

# $1 seconds of the entry's source in H.264 + AAC, a keyframe every 2 s (what lets HLS cut it in 4 s or
# 2 s segments); the other arguments are more output options.
h264_aac() {
  seconds=$1
  shift
  encode "$seconds" -c:v libx264 -preset veryfast -pix_fmt yuv420p -g 50 -c:a aac -b:a 96k -ac 2 "$@"
}

# The streams of $base (the first entry's MP4) copied, not encoded, into another container: the
# arguments are the output options (-vn or -an first to keep one track).
remux() {
  ffmpeg -loglevel error -y -i "$base" -c copy "$@"
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

# Containers. The faststart MP4 is the one synthetic encode the remuxes copy.

# Progressive MP4, index (moov) first: plays and seeks before the whole file is in. The page's "MP4,
# local" button and its ?core=1 default play it.
start mp4-faststart-h264-aac
title='MP4, faststart, H.264 + AAC' url=$path/video.mp4 delivery=file container=mp4 codecs='h264 aac'
axis=container feature='Progressive, 20 s, index first' source=synthetic firefox=ends chrome=ends
h264_aac 20 -movflags +faststart "$dir/video.mp4"
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
hls_ts remux
add

# HLS with fMP4 segments: an init section (EXT-X-MAP), then segments the browser takes as they are.
start hls-fmp4-h264-aac
title='HLS, fMP4, H.264 + AAC' url=$path/index.m3u8 delivery=hls container=fmp4 codecs='h264 aac'
axis=container feature='VOD, 20 s in 4 s segments' source=synthetic firefox=ends chrome=ends
hls_fmp4 remux
add

# One track only, in each segment type.
start hls-ts-audio
title='HLS, MPEG-TS, audio only (AAC)' url=$path/index.m3u8 delivery=hls container=mpegts codecs=aac
axis=container feature='VOD, 20 s, no video track' source=synthetic firefox=ends chrome=ends
hls_ts remux -vn
add

start hls-fmp4-audio
title='HLS, fMP4, audio only (AAC)' url=$path/index.m3u8 delivery=hls container=fmp4 codecs=aac
axis=container feature='VOD, 20 s, no video track' source=synthetic firefox=ends chrome=ends
hls_fmp4 remux -vn
add

start hls-ts-video
title='HLS, MPEG-TS, video only (H.264)' url=$path/index.m3u8 delivery=hls container=mpegts codecs=h264
axis=container feature='VOD, 20 s, no audio track' source=synthetic firefox=ends chrome=ends
hls_ts remux -an
add

start hls-fmp4-video
title='HLS, fMP4, video only (H.264)' url=$path/index.m3u8 delivery=hls container=fmp4 codecs=h264
axis=container feature='VOD, 20 s, no audio track' source=synthetic firefox=ends chrome=ends
hls_fmp4 remux -an
add

# Three qualities of fMP4 under a master playlist; 2 s keyframes keep their segments aligned.
start hls-fmp4-qualities
title='HLS, fMP4, 3 qualities, H.264 + AAC' url=$path/master.m3u8 delivery=hls container=fmp4 codecs='h264 aac'
axis=container feature='VOD, 12 s, 180p, 270p and 360p' source=synthetic firefox=ends chrome=ends
h264_aac 12 -filter_complex '[0:v]split=3[a][b][c];[a]scale=-2:180[180];[b]scale=-2:270[270];[c]null[360]' \
  -map '[180]' -map '[270]' -map '[360]' -map 1:a -map 1:a -map 1:a -b:v:0 200k -b:v:1 400k -b:v:2 800k \
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
hls_fmp4 encode 12 -c:v libx264 -preset veryfast -bf 3 -g 48 -c:a aac -b:a 96k -ac 2
add

# The same media and fields (no `start`), with the gaps plugin.
id=hls-fmp4-clip-gaps title='HLS, fMP4, Sintel with B-frames, gaps plugin' plugins='hls gaps'
feature='VOD, 12 s, B-frames: the buffer starts 83 ms in, jumped'
add

# Progressive WebM, VP9 + Opus: both browsers play it themselves. A keyframe every 2 s, so the VP9 HLS
# entry below can copy the video.
start webm-vp9-opus
title='WebM, VP9 + Opus' url=$path/video.webm delivery=file container=webm codecs='vp9 opus'
axis=container feature='Progressive, 10 s' source=synthetic firefox=ends chrome=ends
encode 10 -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 500k -g 50 -c:a libopus -b:a 96k "$dir/video.webm"
add
webm=$dir/video.webm

# Codecs our HLS engine does not play (Firefox's case: Chrome plays HLS itself), each to fail naming
# the codec. Outcomes as seen in headless Firefox 130 and Chrome 156. No HE-AAC: ffmpeg's aac encoder
# has no HE profile, and few ffmpeg builds have libfdk_aac. 10 s each; where only the audio codec
# differs, the H.264 of $base is copied and only the audio encoded.

# The browser plays the file itself, or does not: no engine on the way.
start mp4-hevc-aac
title='MP4, faststart, HEVC + AAC' url=$path/video.mp4 delivery=file container=mp4 codecs='hevc aac'
axis=codec feature='Progressive HEVC (hvc1): the browser decodes it or not' source=synthetic
firefox='fails: NS_ERROR_DOM_MEDIA_METADATA_ERR, shown on the player'
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
chrome="fails: DECODER_ERROR_NOT_SUPPORTED (Chrome's HLS, no HEVC decoder), shown on the player"
hls_fmp4 ffmpeg -loglevel error -y -i "$hevc" -c copy
add

# Audio in MPEG-TS that the engine does not take: it drops the track, says so once and plays the picture.
start hls-ts-ac3
title='HLS, MPEG-TS, H.264 + AC-3' url=$path/index.m3u8 delivery=hls container=mpegts codecs='h264 ac3'
axis=codec feature='AC-3 audio' source=synthetic
firefox='ends, picture only: a warning names the AC-3 track'
chrome="ends, picture only: Chrome's HLS has no AC-3 decoder"
hls_ts remux -t 10 -c:a ac3 -b:a 192k -ac 2
add

start hls-ts-eac3
title='HLS, MPEG-TS, H.264 + E-AC-3' url=$path/index.m3u8 delivery=hls container=mpegts codecs='h264 eac3'
axis=codec feature='E-AC-3 audio' source=synthetic
firefox='ends, picture only: a warning names the E-AC-3 track'
chrome="ends, picture only: Chrome's HLS has no E-AC-3 decoder"
hls_ts remux -t 10 -c:a eac3 -b:a 192k -ac 2
add

start hls-ts-mp3
title='HLS, MPEG-TS, H.264 + MP3' url=$path/index.m3u8 delivery=hls container=mpegts codecs='h264 mp3'
axis=codec feature='MP3 audio' source=synthetic
firefox='ends, picture only: a warning names the MPEG audio (MP3) track' chrome=ends
hls_ts remux -t 10 -c:a libmp3lame -b:a 128k -ac 2
add

# Packed audio: raw MP3 segments, no container around them. ffmpeg's hls muxer would wrap MP3 in
# MPEG-TS, so the segment muxer cuts it and writes the playlist.
start hls-mp3-packed
title='HLS, packed audio, MP3' url=$path/index.m3u8 delivery=hls container=mp3 codecs=mp3
axis=codec feature='Audio-only playlist of .mp3 segments' source=synthetic
firefox='fails at once naming it: MP3 packed audio is a format this player cannot play'
chrome="fails: CHUNK_DEMUXER_ERROR_APPEND_FAILED (Chrome's HLS), shown on the player"
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
hls_fmp4 ffmpeg -loglevel error -y -i "$webm" -c:v copy -c:a aac -b:a 96k -ac 2
add

start hls-fmp4-av1
title='HLS, fMP4, AV1 + AAC' url=$path/index.m3u8 delivery=hls container=fmp4 codecs='av1 aac'
axis=codec feature='AV1 video' source=synthetic
firefox='fails: init section: av01 is a codec this player cannot play' chrome=ends
hls_fmp4 encode 10 -c:v libaom-av1 -usage realtime -cpu-used 8 -row-mt 1 -tiles 2x2 -b:v 500k -g 50 -c:a aac -b:a 96k -ac 2
add

# Faults. Container entries' playlists through the server's fault route (crates/server/src/fault.rs), which
# lists their URIs through it too, so the fault reaches the segments. The url is a route, not a file, and
# there is no media of their own, so no folder. The route counts requests
# from the server's start, all paths of a spec together: nth=3 is the third, whichever file that is (the
# segment after the first in Firefox; Chrome fetches the first segment twice), and shows once until the
# server restarts. Chrome's player gives up on any failed download, and the page shows nothing.
# Starts fault entry $1: the faults $2 over the playlist $3 (from catalog/) of a synthetic H.264 + AAC
# container entry in container $4.
fault_start() {
  fields "$1"
  url=fault/$2/catalog/$3 container=$4 delivery=hls codecs='h264 aac' axis=fault source=synthetic
}
TS=hls-ts-h264-aac/index.m3u8 QUALITIES=hls-fmp4-qualities/master.m3u8
CHROME_GIVES_UP='fails: PipelineStatus::DEMUXER_ERROR_COULD_NOT_PARSE'

# A missing segment that comes back: the engine retries it (retries 3).
fault_start fault-segment-404 status=404,nth=3 $TS mpegts
title='HLS, a segment 404 then back' fault='A segment answers 404 once (the third request): retried'
firefox=ends chrome=$CHROME_GIVES_UP
add

# The playlist itself fails first: retried like a segment.
fault_start fault-playlist-503 status=503,nth=1 $TS mpegts
title='HLS, the playlist 503 first' fault='The playlist answers 503 on the first request: retried'
firefox=ends chrome=$CHROME_GIVES_UP
add

# A flaky CDN under three qualities: three in ten requests fail with 503, each path its own way, the same
# every run. Firefox meets one (a 360p segment), Chrome the 180p init section.
fault_start fault-flaky flaky=0.3 $QUALITIES fmp4
title='HLS, a flaky CDN (30% 503)' fault='3 requests in 10 answer 503: each retried'
firefox=ends chrome=$CHROME_GIVES_UP
add

# A network slower than every quality but the lowest: both players drop to 180p and play on.
fault_start fault-slow slow=300kbit $QUALITIES fmp4
title='HLS, a slow network (300 kbit/s)' fault='Every body at 300 kbit/s: down to 180p' firefox=ends chrome=ends
add

# A connection dropped mid-segment (half the body, under the full Content-Length): the download fails
# and is retried, by the engine and by Chrome alike.
fault_start fault-truncated truncate=0.5,nth=3 $TS mpegts
title='HLS, a truncated segment' fault='A segment cut at half its length once (the third request): retried'
firefox=ends chrome=ends
add

# A CDN or captive portal answering 200 with an HTML error page in place of a segment: the engine warns
# and skips it, then moves the playhead past the hole; Chrome drops it and ends 4 s early.
fault_start fault-html-segment body=html,nth=3 $TS mpegts
title='HLS, an HTML page as a segment' fault='A segment is an HTML error page with 200 once (the third request): skipped'
firefox=ends chrome=ends
add

# Pathologies: HLS streams that go wrong in one way each, encoded in 2 s segments, then edited: the
# playlist, or the timestamps of the segments after a point. The comment above each entry says what it
# should do once the engine handles it.

# $1 seconds of H.264 + AAC as an HLS VOD in 2 s segments; the other arguments are more output options,
# the playlist last.
hls_vod() {
  seconds=$1
  shift
  h264_aac "$seconds" -f hls -hls_time 2 -hls_playlist_type vod "$@"
}

# A stream of $2 seconds encoded on its own in "$dir/$1": 000.ts on, and index.m3u8. Its timestamps
# start at 1.4 s, as ffmpeg's always do. The other arguments are more output options.
stream() {
  folder=$dir/$1
  shift
  mkdir -p "$folder"
  hls_vod "$@" -hls_segment_filename "$folder/%03d.ts" "$folder/index.m3u8"
}

# The segments (#EXTINF and URI) of the playlist in "$dir/$1", their URIs from "$dir" (from another
# entry's folder too: ../<id>/<folder>).
segments() {
  sed -n "/^#EXTINF/{p;n;s|^|$1/|;p}" "$dir/$1/index.m3u8"
}

# A media playlist's first lines: 2 s segments from sequence number 0.
head_tags() {
  printf '#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n'
}

# 12 s in "$dir" with a hole of $1 seconds 6 s in: the segments from 003.ts on are moved that much later
# (ffmpeg adds 1.4 s unless told not to), and 002.ts is that much longer in the playlist, so it still says
# where each is. No B-frames: they start each segment's video 0.08 s after its audio, and the holes of
# the two tracks would add up in the buffer.
hole() {
  stream . 12 -bf 0
  for segment in "$dir"/00[3-5].ts; do
    ffmpeg -loglevel error -y -copyts -i "$segment" -c copy -muxdelay 0 -muxpreload 0 -output_ts_offset "$1" \
      -f mpegts "$segment.moved"
    mv "$segment.moved" "$segment"
  done
  length=$(LC_ALL=C awk "BEGIN { print 2 + $1 }")
  sed -i "/^#EXTINF/{N;/\n002\.ts\$/s/:[0-9.]*,/:$length,/}" "$dir/index.m3u8"
}

# Two streams of 6 s encoded apart, in a/ and b/, the second in grey: its timestamps start over, as at
# an ad or after an encoder restart. Both in one playlist, EXT-X-DISCONTINUITY between them. Our engine
# rebases timestamps once, so the second is appended over the first and the duration shrinks to 6 s. Once
# the engine reads the tag and rebases each section, it plays the 12 s through, the counter going back
# to 0 at 0:06.
start hls-ts-discontinuity
title='HLS, MPEG-TS, discontinuity with a timestamp reset' url=$path/index.m3u8 delivery=hls container=mpegts
codecs='h264 aac' axis=feature feature='VOD, 2 × 6 s encoded apart, EXT-X-DISCONTINUITY between them'
source=synthetic firefox='fails: ends at 0:06 of 0:12, the grey half buffered over the first, no message'
chrome=ends
stream a 6
stream b 6 -vf hue=s=0
{
  head_tags
  echo '#EXT-X-PLAYLIST-TYPE:VOD'
  segments a
  echo '#EXT-X-DISCONTINUITY'
  segments b
  echo '#EXT-X-ENDLIST'
} > "$dir/index.m3u8"
add

# An encoder restart in a live stream, which one static playlist cannot show: a reload has to list other
# segments. With the streams above: live.m3u8 starts as before.m3u8, the first as a live window of 6 s
# (no ENDLIST, so the player reloads it and waits at its end); after.m3u8 is the window after the
# restart: MEDIA-SEQUENCE back to 0, the grey stream's segments under the sequence numbers already
# played, then ENDLIST. Copied over live.m3u8 while it plays, the next reload sees the restart. Now our
# engine skips the new segments, as their numbers are loaded already, and ends at 0:06; so does Chrome's
# own player. Once the engine sees the restart, it plays the grey 6 s after the first and ends at 0:12.
start hls-ts-encoder-restart
title='HLS, MPEG-TS, live, encoder restart (copy a file by hand)' url=$path/live.m3u8 delivery=hls
container=mpegts codecs='h264 aac' axis=feature source=synthetic
feature="Live, 6 s, then MEDIA-SEQUENCE back to 0: while it plays, cp after.m3u8 live.m3u8 in $dir (cp before.m3u8 live.m3u8 to start over)"
firefox='fails: ends at 0:06 once after.m3u8 is in, the new segments never fetched, no message' chrome=$firefox
{ head_tags; segments ../hls-ts-discontinuity/a; } > "$dir/before.m3u8"
{ head_tags; segments ../hls-ts-discontinuity/b; echo '#EXT-X-ENDLIST'; } > "$dir/after.m3u8"
cp "$dir/before.m3u8" "$dir/live.m3u8"
add

# A hole of one frame (0.04 s) in the timestamps. Firefox plays over it, gaps plugin or not. Chrome waits at
# it, 0.07 s before the end of the range, with its own player or our engine; the gaps plugin, which measures
# the hole between ranges and runs on either, jumps it.
start hls-ts-hole-small
title='HLS, MPEG-TS, a one-frame hole, gaps plugin' url=$path/index.m3u8 delivery=hls container=mpegts
codecs='h264 aac' axis=feature feature='VOD, 12 s, no media from 6 to 6.04 s' source=synthetic plugins='hls gaps'
firefox=ends chrome=ends
hole 0.04
add

# A hole of 2 s: missing media the gaps plugin leaves alone, so the player waits at it; so does
# Chrome's own player, which now and then stops with an error instead. Once the gaps plugin nudges a stalled
# playhead and gives up after its retries, Firefox stops there with a stall error instead of waiting.
start hls-ts-hole-large
title='HLS, MPEG-TS, a 2 s hole, gaps plugin' url=$path/index.m3u8 delivery=hls container=mpegts
codecs='h264 aac' axis=feature feature='VOD, 14 s, no media from 6 to 8 s' source=synthetic plugins='hls gaps'
firefox='fails: waits at 0:06, no message'
chrome='fails: waits at 0:06, no message, or at times PipelineStatus::DEMUXER_ERROR_COULD_NOT_PARSE'
hole 2
add

# Byte-range segments: one file, each segment a slice of it (EXT-X-BYTERANGE). Our engine ignores the
# ranges: it fetches the whole file for each segment, six times, and appends it all at the same times, so
# it still plays to the end. Once the engine fetches the slices with a Range header, each is fetched once.
start hls-ts-byterange
title='HLS, MPEG-TS, byte-range segments in one file' url=$path/index.m3u8 delivery=hls container=mpegts
codecs='h264 aac' axis=feature feature='VOD, 12 s, EXT-X-BYTERANGE slices of video.ts' source=synthetic
firefox=ends chrome=ends
hls_vod 12 -hls_flags single_file -hls_segment_filename "$dir/video.ts" "$dir/index.m3u8"
add

# The same in fMP4, with the init section a slice of the file too (EXT-X-MAP BYTERANGE), which our engine
# refuses before fetching anything. Once the engine fetches the slices, it plays the 12 s and ends.
start hls-fmp4-byterange
title='HLS, fMP4, byte-range segments and init section in one file' url=$path/index.m3u8 delivery=hls
container=fmp4 codecs='h264 aac' axis=feature feature='VOD, 12 s, EXT-X-MAP and EXT-X-BYTERANGE slices of video.mp4'
source=synthetic chrome=ends
firefox='fails: HLS: giving up: …/video.mp4 is a byte range (EXT-X-MAP BYTERANGE), which this player cannot fetch yet'
hls_vod 12 -hls_segment_type fmp4 -hls_flags single_file -hls_segment_filename "$dir/video.mp4" "$dir/index.m3u8"
add

# The same segment twice in a row in the playlist, without a discontinuity: its timestamps say 4 s both
# times, the playlist 4 s then 6 s. Our engine appends it at its timestamps, over itself, and plays the
# 12 s of media once; Chrome's own player fails as it appends the repeat. No task: the stream is broken.
start hls-ts-segment-twice
title='HLS, MPEG-TS, a segment listed twice' url=$path/index.m3u8 delivery=hls container=mpegts codecs='h264 aac'
axis=feature feature='VOD, 12 s, 002.ts listed twice: 14 s by the playlist' source=synthetic firefox=ends
chrome='fails: PipelineStatus::CHUNK_DEMUXER_ERROR_APPEND_FAILED: Parsed buffers not in DTS sequence'
stream . 12
sed -i '/^#EXTINF/{N;/\n002\.ts$/p}' "$dir/index.m3u8"
add

printf '[\n%s\n]\n' "$entries" > web/catalog/catalog.json
ls web/catalog
