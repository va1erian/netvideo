# syntax=docker/dockerfile:1
#
# Multi-stage build for netvideo-server. Build from the repository root:
#
#   docker build -f deploy/Dockerfile -t va1erian/netvideo-server .
#
# The bundled SQLite and the rustls provider are compiled from source, so the
# builder image needs a C toolchain and CMake. The runtime image carries
# Debian's ffmpeg for ffprobe; jellyfin-ffmpeg (Intel QSV) replaces it with
# the transcoding milestone.

FROM rust:1-bookworm AS builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake perl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p netvideo-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /var/lib/netvideo netvideo \
    && mkdir -p /var/lib/netvideo /etc/netvideo /media/videos \
    && chown netvideo:netvideo /var/lib/netvideo

COPY --from=builder /src/target/release/netvideo-server /usr/local/bin/netvideo-server

USER netvideo
EXPOSE 8080
VOLUME ["/var/lib/netvideo"]
ENV NETVIDEO_DATA_DIR=/var/lib/netvideo
ENTRYPOINT ["/usr/local/bin/netvideo-server"]
# No default `--config`: the server runs `serve` and reads `NETVIDEO_CONFIG`
# or the `NETVIDEO_*` environment variables. Pass
# `--config /etc/netvideo/server.toml` as a command to use a file.
