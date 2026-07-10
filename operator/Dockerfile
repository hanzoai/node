# Canonical Rust implementation of the Hanzo operator.
#
# Build with this directory as context: `docker build operator/`
# (the COPY paths below are relative to operator/, not the repo root — a
#  repo-root context would copy node's workspace Cargo.toml and break).
# Image: ghcr.io/hanzoai/operator:vX.Y.Z (Go sibling: ghcr.io/luxfi/operator).
#
# Builder — Rust 1.95: deps (clap_lex 1.1.0 via clap 4.6) require edition2024
# (stabilized in 1.85); pinned to the toolchain that builds the workspace.
FROM rust:1.95-bookworm AS builder

WORKDIR /build

# Copy Cargo manifest and lock first for layer caching.
COPY Cargo.toml Cargo.lock ./

# Pre-build a dummy binary so dependencies cache between builds.
RUN mkdir -p src src/bin && \
    echo 'fn main(){}' > src/main.rs && \
    echo 'fn main(){}' > src/bin/generate_crd_yaml.rs && \
    echo '' > src/lib.rs && \
    cargo build --release 2>/dev/null || true

# Now copy real source and build.
COPY src/ src/
RUN touch src/main.rs src/lib.rs src/bin/generate_crd_yaml.rs && cargo build --release

# Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

RUN useradd -r -u 65532 -g nogroup -s /sbin/nologin operator

COPY --from=builder /build/target/release/operator /usr/local/bin/operator
COPY --from=builder /build/target/release/generate-crd-yaml /usr/local/bin/generate-crd-yaml

USER 65532:65532
ENTRYPOINT ["/usr/local/bin/operator"]
