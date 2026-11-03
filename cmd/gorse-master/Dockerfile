# syntax = docker/dockerfile:1

# STEP 1 build the Rust binary. The Go build of this image is removed (WP-6-1).
FROM rust:1.99.1-bookworm AS build

WORKDIR /src

COPY . ./

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p gorse-cmd --bin gorse-master && \
    cp target/release/gorse-master /usr/local/bin/gorse-master

# STEP 2 build a small image with the binary and the CA certificates.
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=build /usr/local/bin/gorse-master /usr/bin/gorse-master

ENV USER=root

ENTRYPOINT ["/usr/bin/gorse-master", "-c", "/etc/gorse/config.toml"]
