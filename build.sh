#!/bin/sh
# Build the wasm core into web/. Serve with: cargo run -p server
set -e
cd "$(dirname "$0")"
cargo build --release --target wasm32-unknown-unknown -p player-wasm
cp target/wasm32-unknown-unknown/release/player_wasm.wasm web/player.wasm
if command -v wasm-opt >/dev/null; then wasm-opt -Oz web/player.wasm -o web/player.wasm; fi
[ -d web/hls ] || ! command -v ffmpeg >/dev/null || ./make-hls.sh
[ -f web/media/test.mp4 ] || ! command -v ffmpeg >/dev/null || ./make-media.sh
ls -l web/player.wasm
