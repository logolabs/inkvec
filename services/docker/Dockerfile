# syntax=docker/dockerfile:1
#
# inkvec-server: Inkvec's HTTP service (crates/inkvec-server). Build from the repository root
# so the whole workspace is in the build context (see .dockerignore there):
#
#     docker build -f services/docker/Dockerfile -t inkvec-server .
#
# Two stages: a `rust:1.98-bookworm` builder (matches the workspace's pinned toolchain,
# rust-version = "1.98" in the root Cargo.toml) with cargo registry and target-dir cache
# mounts, and a `distroless/cc-debian12:nonroot` runtime -- glibc and libgcc (what a default
# `x86_64-unknown-linux-gnu` build links against) and nothing else: no shell, no package
# manager, no curl. `inkvec-server --healthcheck` stands in for the curl/wget a HEALTHCHECK
# would normally shell out to (see crates/inkvec-server/src/lib.rs::healthcheck).
#
# Placed under services/docker/ rather than a repository-root Dockerfile.server: this is the
# first of what the roadmap (docs/svgify-release-surface, in the maintainer's notes) expects
# to grow into more than one deployable artifact (a ComfyUI node is next), and services/ keeps
# each one's build context and compose file together instead of crowding the repository root,
# where four other language bindings already keep their own top-level directories
# (packages/npm, crates/inkvec-py, crates/inkvec-ffi).

ARG RUST_VERSION=1.98-bookworm

FROM rust:${RUST_VERSION} AS builder
WORKDIR /app

# Every workspace member's Cargo.toml must be present for `cargo` to resolve the workspace,
# even the ones this image never builds (inkvec-wasm, inkvec-ffi, inkvec-py); `bindings/` is
# needed too, for bindings/openapi.json (embedded with include_str! in inkvec-server) and
# bindings/options.schema.json (compared against the schema inkvec::Options generates, in
# inkvec's own tests, which this build does not run but which cargo still type-checks the
# source of). See .dockerignore for what is left out of this COPY.
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY bindings bindings

ARG TARGETARCH
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry \
    --mount=type=cache,target=/app/target,id=cargo-target-${TARGETARCH} \
    cargo build --release --locked -p inkvec-server \
    && install -Dm755 target/release/inkvec-server /out/inkvec-server
    # `install` copies the binary out of the cache-mounted target/ before the mount is
    # dropped at the end of this RUN step; anything left only under target/ would not exist
    # in the next stage.

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

ARG INKVEC_VERSION=0.1.3
LABEL org.opencontainers.image.title="inkvec-server" \
      org.opencontainers.image.description="Inkvec's HTTP service: trace raster images to SVG over HTTP" \
      org.opencontainers.image.source="https://github.com/logolabs/inkvec" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.version="${INKVEC_VERSION}"
      # INKVEC_VERSION tracks [workspace.package].version in the root Cargo.toml; pass
      # --build-arg INKVEC_VERSION=$(value) to keep a built image's label in sync without
      # editing this file.

COPY --from=builder /out/inkvec-server /usr/local/bin/inkvec-server

ENV INKVEC_PORT=8080
EXPOSE 8080

# distroless/cc's `nonroot` tag already runs as the non-root `nonroot` user (uid 65532); no
# USER instruction needed (and no shell to run one with).
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD ["/usr/local/bin/inkvec-server", "--healthcheck"]

ENTRYPOINT ["/usr/local/bin/inkvec-server"]
