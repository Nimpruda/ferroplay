# Dev tasks. Native recipes need Rust, ffmpeg and the browsers; docker ones need only Docker.

set positional-arguments

# Media made in containers stays owned by you (compose.yaml).
export HOST_UID := `id -u`
export HOST_GID := `id -g`

[private]
default:
    @just --list

# Build the wasm core into web/ (and the demo media, if ffmpeg is installed)
[group('native')]
build:
    ./build.sh

# Serve web/ on http://127.0.0.1:8080
[group('native')]
serve: build
    cargo run -p server

# Every crate's tests
[group('native')]
test:
    cargo test

# The pre-commit hooks on every file
[group('native')]
lint:
    prek run --all-files

# End-to-end tests in headless Firefox and Chrome
[group('native')]
e2e: build
    python3 e2e/run.py

# Remake the demo HLS stream in web/hls (downloads the Sintel trailer)
[group('native')]
hls:
    ./make-hls.sh

# Remake the test MP4 in web/media
[group('native')]
media:
    ./make-media.sh

# Live stream in web/hls/live until Ctrl-C: a test pattern, or a clip on a loop
[group('native')]
live *clip:
    ./make-live.sh "$@"

# Server and demo media on http://127.0.0.1:8080, rebuilt from the sources
[group('docker')]
up:
    docker compose up --build

# The same, plus a live stream at /hls/live/live.m3u8
[group('docker')]
up-live:
    docker compose --profile live up --build

# Stop and remove the containers
[group('docker')]
down:
    docker compose --profile live --profile e2e down

# Make the demo media that is missing from web/
[group('docker')]
docker-media:
    docker compose run --rm --build media

# End-to-end tests in a container (amd64)
[group('docker')]
docker-e2e:
    docker compose --profile e2e run --rm --build e2e
