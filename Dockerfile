# syntax=docker/dockerfile:1
# Stages: chef (toolchain) -> planner (dependency recipe) -> deps (dependencies only, cached until a
# Cargo.toml or Cargo.lock changes) -> build (our crates) -> slim images. Default target: server.

FROM lukemathwalker/cargo-chef:0.1.78-rust-1.99-trixie AS chef
# Debian's binaryen (120) rejects what this rustc emits, so wasm-opt comes from upstream.
ARG BINARYEN=133
RUN rustup target add wasm32-unknown-unknown \
 && arch=$(uname -m) && url=https://github.com/WebAssembly/binaryen/releases/download/version_$BINARYEN \
 && curl -fsSLO $url/binaryen-version_$BINARYEN-$arch-linux.tar.gz \
 && curl -fsSL $url/binaryen-version_$BINARYEN-$arch-linux.tar.gz.sha256 | sha256sum -c \
 && tar -xzf binaryen-*.tar.gz -C /usr/local --strip-components=1 && rm binaryen-*.tar.gz
WORKDIR /src

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS deps
COPY --from=planner /src/recipe.json .
RUN cargo chef cook --release --recipe-path recipe.json -p server -p ts2mp4 \
 && cargo chef cook --release --recipe-path recipe.json --target wasm32-unknown-unknown -p player-wasm

FROM deps AS build
COPY . .
# build.sh skips the demo media without ffmpeg; the media service makes it instead.
RUN cargo build --release -p server -p ts2mp4 && ./build.sh

# ffmpeg and the make-*.sh scripts; compose mounts ./web over /src/web so they write into the repo.
FROM debian:trixie-slim AS media
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY make-hls.sh make-media.sh make-live.sh ./

FROM gcr.io/distroless/cc-debian13:nonroot AS ts2mp4
COPY --from=build /src/target/release/ts2mp4 /usr/local/bin/
ENTRYPOINT ["ts2mp4"]

FROM gcr.io/distroless/cc-debian13:nonroot AS server
WORKDIR /app
COPY --from=build /src/target/release/server /usr/local/bin/
COPY --from=build /src/web web
ENV HOST=0.0.0.0 PORT=8080
EXPOSE 8080
ENTRYPOINT ["server"]
CMD ["web"]
