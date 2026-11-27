# Copyright (C) 2019-2026, Lux Industries, Inc. All rights reserved.
# SPDX-License-Identifier: BSD-3-Clause-Eco
#
# hanzod / luxd / lux-node — the Rust runtime, as an image.
#
# BUILD CONTEXT IS THE ORG DIRECTORY, NOT THIS REPO. The crate names three
# siblings by path — ../evm, ../sdk, and the crypto library ../../lux/crypto —
# so a build that can only see this checkout cannot link. Build it from the
# directory that holds them:
#
#     docker build -f node/Dockerfile -t ghcr.io/luxfi/lux-rs-node .
#
# run from the lux-rs checkout's parent, with lux/crypto beside lux-rs. That is
# the layout the crate already declares; the Dockerfile states it rather than
# discovering it at link time.
#
# THE BUILDER'S DEBIAN RELEASE IS THE RUNTIME'S, AND THAT IS LOAD-BEARING.
# A binary carries the symbol versions of the glibc it was linked against, and
# distroless-*-debian12 provides Debian 12's: glibc 2.36, GLIBCXX_3.4.30. Built
# anywhere newer, it builds, it pushes, and it dies on first exec with
# `version GLIBC_2.38 not found` — a failure that no build log shows. Measured,
# not assumed: a host-built binary was run against this base and did exactly
# that, and one built in bookworm ran.

# ── builder ─────────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

WORKDIR /src

# The siblings this crate names by path, first, so a change to the node does
# not rebuild them. `lux-rs/consensus` is deliberately absent: the node takes
# lux-consensus from crates.io, not from the checkout of the same name.
COPY lux-rs/sdk lux-rs/sdk
COPY lux-rs/evm lux-rs/evm
COPY lux/crypto/dist lux/crypto/dist

COPY lux-rs/node lux-rs/node
WORKDIR /src/lux-rs/node

# Release, stripped, and one codegen unit: this binary is built once and run for
# months, so the build's time is worth the run's.
ENV CARGO_PROFILE_RELEASE_LTO=true \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    CARGO_PROFILE_RELEASE_STRIP=symbols \
    CARGO_PROFILE_RELEASE_PANIC=abort
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/lux-rs/node/target \
    cargo build --release --bins && \
    mkdir -p /out/bin /out/lib && \
    cp target/release/hanzod target/release/luxd target/release/lux-node /out/bin/ && \
    cp /src/lux/crypto/dist/libluxcrypto.so /out/lib/

# The state directory, made here because the runtime has no shell to make one.
RUN mkdir -p /out/data && chown -R 65532:65532 /out

# ── runtime ─────────────────────────────────────────────────────────────────
# cc rather than static: the binary is dynamically linked (measured with `ldd`,
# not assumed — libluxcrypto.so, libgcc_s.so.1 and libc.so.6), and libluxcrypto
# is a shared object this node loads rather than a crate it compiles in. cc
# carries glibc and libgcc and nothing that could execute an attacker's idea:
# no shell, no package manager, no curl.
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

COPY --from=builder /out/lib/libluxcrypto.so /usr/lib/
COPY --from=builder /out/bin/ /usr/local/bin/
COPY --from=builder --chown=65532:65532 /out/data /data

USER nonroot
WORKDIR /data

# RPC, and the stake port the mesh votes over.
EXPOSE 9780 9781

ENTRYPOINT ["/usr/local/bin/hanzod"]
