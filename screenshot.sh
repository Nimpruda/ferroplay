#!/usr/bin/env bash
# Screenshots of the demo page in headless Chrome, small enough to read cheaply (an agent pays
# per pixel). Writes <dir>/<name>-desktop.png (1280x900 at half scale: 640x450) and
# <dir>/<name>-mobile.png (390x844 at half scale), of the page's top, or of the whole page with
# FULL=1. Also a text dump: <dir>/<name>.txt holds the page's text content, which reads cheaper
# than any picture. The server must be up (cargo run -p server).
#
#   ./screenshot.sh [path] [dir] [name]     # path defaults to /, dir to shots/, name to the path's slug
#   FULL=1 SCALE=1 ./screenshot.sh '/?core=1'
#
# SCALE is the device scale factor (0.5 default: half the pixels each way, a quarter of the
# tokens; still legible). WAIT is the virtual-time budget in ms the page gets to load (6000).
set -euo pipefail

path=${1:-/}
dir=${2:-shots}
name=${3:-$(printf '%s' "$path" | sed 's#^/##; s#[^A-Za-z0-9]\+#-#g; s#^-*##; s#-*$##')}
name=${name:-index}
url="${URL:-http://127.0.0.1:8080}$path"
scale=${SCALE:-0.5}
wait=${WAIT:-6000}

for b in google-chrome chromium chromium-browser; do
  if command -v "$b" > /dev/null; then chrome=$b; break; fi
done
: "${chrome:?no Chrome or Chromium on PATH}"

mkdir -p "$dir"
curl -fsS -o /dev/null "$url" || { echo "nothing at $url: run cargo run -p server first" >&2; exit 1; }

shot() { # name width height
  local size="$2,$3"
  # FULL=1: a tall window, so the whole page lands in the picture (the page is never that tall).
  [ "${FULL:-0}" = 1 ] && size="$2,6000"
  "$chrome" --headless=new --no-sandbox --disable-gpu --hide-scrollbars --mute-audio \
    --window-size="$size" --force-device-scale-factor="$scale" --virtual-time-budget="$wait" \
    --screenshot="$dir/$name-$1.png" "$url" 2> /dev/null
  # Chrome pads a full-page shot to the window; cut the empty bottom off so the file stays small.
  if [ "${FULL:-0}" = 1 ] && command -v convert > /dev/null; then
    convert "$dir/$name-$1.png" -trim +repage -bordercolor "#111" -border 8 "$dir/$name-$1.png"
  fi
  echo "$dir/$name-$1.png"
}

shot desktop 1280 900
shot mobile 390 844

# The page as text: what an agent should read first, and only look at the picture for layout.
"$chrome" --headless=new --no-sandbox --disable-gpu --virtual-time-budget="$wait" --dump-dom "$url" 2> /dev/null \
  | python3 -c '
import html, re, sys
s = sys.stdin.read()
s = re.sub(r"(?s)<(script|style)[^>]*>.*?</\1>", "", s)
s = re.sub(r"<(?:br|/p|/li|/tr|/h\d|/div|/section|/fieldset|/label|/button)[^>]*>", "\n", s)
s = re.sub(r"</t[dh]>", "\t", s)
s = html.unescape(re.sub(r"<[^>]+>", "", s))
print("\n".join(line.strip() for line in s.splitlines() if line.strip()))
' > "$dir/$name.txt"
echo "$dir/$name.txt"
