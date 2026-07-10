# hanzod-operator image — native Kubernetes operator reconciling apps.hanzo.ai
# (kind: App) CRs to Deployment/StatefulSet + Service + Ingress (+ PVC/PDB/HPA).
#
# Build context = the repo ROOT (the cargo workspace):
#   docker build -f hanzo-bin/hanzod-operator/Dockerfile -t ghcr.io/hanzoai/hanzod-operator:<tag> .
#
# Published by .github/workflows/hanzod-operator-image.yml on the native Hanzo ARC
# fleet (NO GitHub-hosted builders). The operator is a self-contained workspace
# leaf (crates.io deps only — no protoc, no inference graph), so `-p hanzod-operator`
# compiles a small subgraph independent of the rest of the node workspace.

FROM rust:bookworm AS builder
# clang + lld satisfy .cargo/config.toml (`linker = clang`, `-fuse-ld=lld`);
# cmake + perl build the aws-lc-rs backend kube pulls via its `rustls-tls` feature.
RUN apt-get update && apt-get install -y --no-install-recommends \
        clang lld cmake perl pkg-config \
    && rm -rf /var/lib/apt/lists/*
RUN rustup component add rustfmt
WORKDIR /build
COPY . .
# Bound parallel codegen/link jobs to avoid OOM on constrained runners (mirrors
# docker/Dockerfile). `--locked` pins the committed Cargo.lock.
ENV CARGO_BUILD_JOBS=4
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --locked --release -p hanzod-operator \
    && cp target/release/hanzod-operator /usr/local/bin/hanzod-operator

FROM debian:bookworm-slim AS runtime
LABEL org.opencontainers.image.title="hanzod-operator" \
      org.opencontainers.image.description="Native Kubernetes operator reconciling apps.hanzo.ai (kind: App)" \
      org.opencontainers.image.vendor="Hanzo AI" \
      org.opencontainers.image.source="https://github.com/hanzoai/node" \
      org.opencontainers.image.licenses="Apache-2.0"
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 -s /usr/sbin/nologin hanzod
COPY --from=builder /usr/local/bin/hanzod-operator /usr/local/bin/hanzod-operator
USER 10001
# Default: run the controller (ambient in-cluster SA). `crd`/`plan` are one-shot.
ENTRYPOINT ["/usr/local/bin/hanzod-operator"]
