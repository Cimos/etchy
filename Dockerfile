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
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release -p etchy-cli

# --- runtime: distroless static, nonroot. Just the binary. ---
FROM gcr.io/distroless/static:nonroot
COPY --from=build /src/target/release/etchy /etchy
ENTRYPOINT ["/etchy"]
