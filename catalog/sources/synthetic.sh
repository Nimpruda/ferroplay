# shellcheck shell=sh
# A catalog source (see make-catalog.sh): a 640x360 test pattern with a 440 Hz tone. No network.
# source_synthetic <seconds> <ffmpeg output options...>
source_synthetic() {
  seconds=$1
  shift
  ffmpeg -loglevel error -y -f lavfi -i testsrc=size=640x360:rate=25 -f lavfi -i sine=frequency=440 -t "$seconds" "$@"
}
