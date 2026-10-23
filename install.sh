#!/bin/sh
# Adds the wasm target, then lists each prerequisite as found or missing, with the recipes that
# need it and the command that installs it. Never uses sudo itself; safe to run again.
set -e

if command -v apt-get >/dev/null; then pm=apt
elif command -v brew >/dev/null; then pm=brew
else pm=
fi
apt='sudo apt-get install -y'
missing=0

# check <command> <needed by> <apt command> <brew command>
check() {
  if command -v "$1" >/dev/null; then
    printf 'found    %-17s %s\n' "$1" "$2"
    return
  fi
  missing=1
  case $pm in
    apt) how=$3 ;;
    brew) how=$4 ;;
    *) how="install $1 with your package manager" ;;
  esac
  printf 'missing  %-17s %s: %s\n' "$1" "$2" "$how"
}

if command -v rustup >/dev/null; then
  rustup target add wasm32-unknown-unknown
else
  missing=1
  echo 'missing  rustup            everything: https://rustup.rs'
fi
check cargo 'build, serve, test' 'rustup default stable' 'rustup default stable'
check cc 'build, serve, test (C compiler and linker)' "$apt build-essential" 'xcode-select --install'
check wasm-opt 'build (optional: smaller wasm)' "$apt binaryen" 'brew install binaryen'
check ffmpeg 'build (optional: demo media), hls, catalog, live, e2e' "$apt ffmpeg" 'brew install ffmpeg'
check python3 'e2e' "$apt python3" 'brew install python'
check firefox 'e2e' "$apt firefox" 'brew install --cask firefox'
chrome=google-chrome
for c in chromium chromium-browser; do command -v "$c" >/dev/null && chrome=$c; done
check "$chrome" 'e2e, shot' "$apt chromium" 'brew install --cask google-chrome'
check prek 'lint' 'pipx install prek' 'brew install prek'
check hadolint 'lint' 'https://github.com/hadolint/hadolint/releases' 'brew install hadolint'
check docker 'lint, up, up-live, down, docker-e2e' "$apt docker.io docker-compose-v2" 'brew install --cask docker'

if [ "$missing" = 0 ]; then echo 'all prerequisites found'; fi
