#!/bin/sh
# Build the wasm core into web/. Serve with: cargo run -p server
set -e
cd "$(dirname "$0")"
cargo build --release --target wasm32-unknown-unknown -p player-wasm
cp target/wasm32-unknown-unknown/release/player_wasm.wasm web/player.wasm
# The flags match the wasm features rustc enables by default; wasm-opt rejects the module without them.
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-multivalue --enable-reference-types web/player.wasm -o web/player.wasm
fi
! command -v ffmpeg >/dev/null || ./make-demo.sh
ls -l web/player.wasm
