# Dev tasks. Native recipes need rustup (stable, wasm32-unknown-unknown) and a C compiler; ffmpeg
# and wasm-opt are optional; e2e and shot need python3 and Firefox/Chrome; lint needs prek, hadolint
# and docker. Docker recipes need only Docker. `just install` checks all of it (README: Prerequisites).

set positional-arguments

# Media made in containers stays owned by you (compose.yaml).
export HOST_UID := `id -u`
export HOST_GID := `id -g`

[private]
default:
    @just --list

# Add the wasm target; list each prerequisite as found or missing, with its install command
[group('native')]
install:
    ./install.sh

# Build the wasm core into web/, with and without HLS (and the demo media, if ffmpeg is installed)
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

# The pre-merge subset of the e2e tests (about a minute)
[group('native')]
e2e-quick: build
    python3 e2e/run.py --quick

# Remake the demo HLS stream in web/hls (downloads the Sintel trailer)
[group('native')]
hls:
    ./make-hls.sh

# Remake the test media catalog in web/catalog (listed on the demo page)
[group('native')]
catalog:
    ./make-catalog.sh

# Live stream in web/hls/live until Ctrl-C: a test pattern, or a clip on a loop
[group('native')]
live *clip:
    ./make-live.sh "$@"

# Small desktop and mobile screenshots of a page of the running server, plus its text, into shots/
[group('native')]
shot path='/' *rest:
    ./screenshot.sh "$@"

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

# End-to-end tests in a container
[group('docker')]
docker-e2e:
    docker compose --profile e2e run --rm --build e2e
