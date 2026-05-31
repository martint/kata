# syntax=docker/dockerfile:1.7
#
# Single Dockerfile for both local dev and the multi-arch release.
# The release workflow is pure orchestration; everything about how
# the image is built lives here.
#
# Stages:
#   1. frontend  — bun builds the Svelte bundle. Pinned to
#                  $BUILDPLATFORM so bun never runs under QEMU.
#   2. backend   — rustxc cross-compiles the kata binary. $TARGETARCH
#                  selects the rust target triple; rustxc carries the
#                  matching cross-toolchain (gcc-aarch64-linux-gnu /
#                  gcc-x86-64-linux-gnu + cargo config). Always runs
#                  on $BUILDPLATFORM — the cargo + rustc + linker
#                  processes are native amd64; the emitted bytes are
#                  target-arch.
#   3. jj-fetch  — downloads the matching musl-static jj CLI for the
#                  target arch. Used by `runtime-with-jj` only; the
#                  `runtime` target skips this entirely.
#   4. runtime   — minimal debian-slim + the kata binary. Defaults to
#                  $TARGETPLATFORM (so apt-get install ca-certificates
#                  emulates under QEMU for non-host arches — ~5 s).
#                  This is what the release workflow targets.
#   5. runtime-with-jj — extends runtime with the jj CLI on PATH so
#                  `kata demo` (which shells out to jj for repo
#                  seeding) works. This is the default target, so
#                  `docker compose up --build` produces it without
#                  any --target flag. The release workflow overrides
#                  to `--target runtime` to ship a leaner image.
#
# Build via buildx:
#   docker buildx build --platform linux/amd64,linux/arm64 \
#     --target runtime --tag ghcr.io/martint/kata:dev .
# Or locally:
#   docker compose up --build

# ---- Stage 1: frontend bundle ----
FROM --platform=$BUILDPLATFORM oven/bun:1.3 AS frontend
WORKDIR /web
COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile
COPY web/ ./
RUN bun run build

# ---- Stage 2: cross-compile via rustxc ----
FROM --platform=$BUILDPLATFORM ghcr.io/martint/rustxc:latest AS backend
ARG TARGETARCH
WORKDIR /src
# Skip build.rs's bun probe — we hand it a pre-built bundle below.
ENV KATA_SKIP_WEB_BUILD=1
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY --from=frontend /web/dist ./web/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    set -eux; \
    case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-gnu ;; \
      arm64) target=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; \
    esac; \
    cargo build --release --bin kata --target "$target"; \
    cp "/src/target/$target/release/kata" /kata

# ---- Stage 3: fetch the matching jj CLI on the build host ----
FROM --platform=$BUILDPLATFORM curlimages/curl:8.10.1 AS jj-fetch
ARG TARGETARCH
ARG JJ_VERSION=0.41.0
WORKDIR /jj
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) jj_arch=x86_64 ;; \
      arm64) jj_arch=aarch64 ;; \
      *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; \
    esac; \
    curl -fsSL \
      "https://github.com/jj-vcs/jj/releases/download/v${JJ_VERSION}/jj-v${JJ_VERSION}-${jj_arch}-unknown-linux-musl.tar.gz" \
      | tar -xz

# ---- Stage 4: runtime (no jj) — what the release image ships ----
# kata serve, the HTTP API, and MCP all use libjj in-process and
# never spawn jj; this is the lean prod target. Override the default
# (--target runtime-with-jj below) to build this one.
FROM debian:stable-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=backend /kata /usr/local/bin/kata

# Bind to every interface inside the container — loopback would be
# unreachable from outside. Host-side port mapping is what actually
# decides external reachability. Overridable via `--bind` or KATA_BIND.
ENV KATA_BIND=0.0.0.0:7878
# Settle the data dir on a well-known path so a bare `docker run`
# works without operator setup, and the VOLUME below has something
# concrete to track. Persist via `-v kata-data:/data`.
ENV KATA_DATA=/data
VOLUME ["/data"]
EXPOSE 7878
ENTRYPOINT ["kata"]
CMD ["--help"]

# ---- Stage 5: runtime-with-jj — adds the jj CLI for `kata demo` ----
# Default target (Dockerfile builds the last stage). `docker compose
# up --build` and `docker build .` produce this image. The release
# workflow targets `runtime` instead to skip the jj download.
FROM runtime AS runtime-with-jj
COPY --from=jj-fetch /jj/jj /usr/local/bin/jj
