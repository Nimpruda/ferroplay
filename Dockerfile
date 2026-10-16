# syntax=docker/dockerfile:1
# Stages: chef (toolchain) -> planner (dependency recipe) -> deps-* (dependencies only, cached until a
# Cargo.toml or Cargo.lock changes) -> native / wasm (our crates) -> slim images. Default target: server.

FROM lukemathwalker/cargo-chef:0.1.78-rust-1.99-trixie AS chef
RUN rustup target add wasm32-unknown-unknown \
 && apt-get update && apt-get install -y --no-install-recommends binaryen && rm -rf /var/lib/apt/lists/*
WORKDIR /src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS deps-native
COPY --from=planner /src/recipe.json .
RUN cargo chef cook --release --recipe-path recipe.json -p server -p ts2mp4

FROM chef AS deps-wasm
COPY --from=planner /src/recipe.json .
RUN cargo chef cook --release --recipe-path recipe.json --target wasm32-unknown-unknown -p player-wasm

FROM deps-native AS native
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --release -p server -p ts2mp4

# build.sh skips the demo media without ffmpeg; the media service makes it instead.
FROM deps-wasm AS wasm
COPY Cargo.toml Cargo.lock build.sh ./
COPY crates crates
RUN mkdir web && ./build.sh

# ffmpeg and the make-*.sh scripts; compose mounts ./web over /src/web so they write into the repo.
FROM debian:trixie-slim AS media
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY make-demo.sh make-hls.sh make-media.sh make-live.sh ./

# e2e/run.py builds and runs our crates itself, so this keeps the toolchain, with them prebuilt.
FROM chef AS e2e
RUN apt-get update && apt-get install -y --no-install-recommends python3 ffmpeg firefox-esr chromium \
 && rm -rf /var/lib/apt/lists/*
ENV FIREFOX=firefox-esr CHROME=chromium
COPY --from=planner /src/recipe.json .
RUN cargo chef cook --recipe-path recipe.json -p server -p ts2mp4
COPY . .
COPY --from=wasm /src/web/player.wasm web/
RUN cargo build -p server -p ts2mp4
CMD ["python3", "e2e/run.py"]

FROM gcr.io/distroless/cc-debian13:nonroot AS ts2mp4
COPY --from=native /src/target/release/ts2mp4 /usr/local/bin/
ENTRYPOINT ["ts2mp4"]

FROM gcr.io/distroless/cc-debian13:nonroot AS server
WORKDIR /app
COPY --from=native /src/target/release/server /usr/local/bin/
COPY web web
COPY --from=wasm /src/web/player.wasm web/
ENV HOST=0.0.0.0
EXPOSE 8080
ENTRYPOINT ["server"]
