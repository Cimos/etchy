# syntax=docker/dockerfile:1
#
# etchy CLI as a tiny, self-contained container: a static musl binary in a
# distroless base (no shell, no package manager, no libc to patch). Only the
# `etchy` CLI ships here — the GUI is a separate desktop/web surface and is never
# containerized.
#
#   docker build -t etchy .
#   docker run --rm -v "$PWD:/work" etchy --format summary -- /work/old /work/new
#
# Exit codes are etchy's CI contract: 0 = no diff, 1 = diff, 2 = error.

# --- build: Alpine's Rust toolchain is musl-native, so a plain release build is
#     already a static x86_64-unknown-linux-musl binary (no cross setup needed). ---
FROM rust:1-alpine@sha256:557a94c82b485a14b569120e00867267108c734efe33f10cc31ca0cd5dd1eca9 AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
# --features pdf so the container carries the schematic-PDF diff too (CLI-6);
# hayro is pure-Rust and builds static on musl.
RUN cargo build --locked --release -p etchy-cli --features pdf

# --- runtime: distroless static, nonroot. Just the binary. ---
FROM gcr.io/distroless/static:nonroot@sha256:e2e927ec666bae08560abb3c55d0659eceabb657f56b6782ab500a9fc7f555e3
COPY --from=build /src/target/release/etchy /etchy
ENTRYPOINT ["/etchy"]
