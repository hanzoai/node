# hanzod — built for the architecture it is built on, and run on almost nothing.
#
# NO CROSS-COMPILE HERE. Each architecture is built on a runner of that
# architecture, so the binary that ships for an arch was compiled by a native
# toolchain rather than emulated. The lab runs both pools.
FROM rust:1-slim-bookworm AS build
WORKDIR /src

# Manifests first, so a source edit re-runs the compile and not the resolve.
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY genesis genesis

# Locked, and the lockfile ships: a resolver left free to drift builds a tree
# nobody ran.
RUN cargo build --release --locked \
 && strip target/release/hanzod \
 && test -s target/release/hanzod

# cc, not static: hanzod links libc and libgcc and nothing else, so the image is
# the binary plus the two libraries it names.
FROM gcr.io/distroless/cc-debian12
COPY --from=build /src/target/release/hanzod /usr/local/bin/hanzod
# The RPC and the validator mesh. A validator is named by the key it proves on
# every link, so the mesh port is not an admin surface and needs no gate of its
# own.
EXPOSE 9630 9631
ENTRYPOINT ["/usr/local/bin/hanzod"]
