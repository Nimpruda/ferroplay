# shellcheck shell=sh
# A catalog source (see make-catalog.sh): the Sintel trailer from 0:14, past the titles. The trailer
# is downloaded once to .cache/ (make-catalog.sh runs from the repo root); delete it to fetch it again.
# source_clip <seconds> <ffmpeg output options...>
source_clip() {
  seconds=$1
  shift
  trailer=.cache/sintel_trailer-480p.mp4
  if [ ! -f "$trailer" ]; then
    mkdir -p .cache
    # Under another name until it is complete, so an interrupted download is not taken for the trailer.
    ffmpeg -loglevel error -y -i https://download.blender.org/durian/trailer/sintel_trailer-480p.mp4 \
      -c copy -f mp4 "$trailer.part"
    mv "$trailer.part" "$trailer"
  fi
  ffmpeg -loglevel error -y -ss 14 -i "$trailer" -t "$seconds" "$@"
}
