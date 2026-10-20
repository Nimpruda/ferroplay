#!/bin/sh
# Build the wasm core into web/: player.wasm (with HLS) and player-core.wasm (without). Serve with: cargo run -p server
set -e
cd "$(dirname "$0")"

# wasm <file in web/> [cargo flags]
wasm() {
  out="web/$1"
  shift
  cargo build --release --target wasm32-unknown-unknown -p player-wasm "$@"
  cp target/wasm32-unknown-unknown/release/player_wasm.wasm "$out"
  # The flags match the wasm features rustc enables by default; wasm-opt rejects the module without them.
  if command -v wasm-opt >/dev/null; then
    wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
      --enable-multivalue --enable-reference-types "$out" -o "$out"
  fi
}
wasm player.wasm
wasm player-core.wasm --no-default-features

! command -v ffmpeg >/dev/null || ./make-demo.sh
ls -l web/player.wasm web/player-core.wasm
