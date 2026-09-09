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
#
# lux-node is a git dependency on a private repository, named by an ssh:// URL
# that Cargo.lock records, so the credential arrives here and the URL is
# rewritten rather than edited — editing it would invalidate the lock. The
# secret is readable only by this command and lands in no layer. Without it
# cargo stops at "failed to authenticate when downloading repository".
# The config is WRITTEN, not set with `git config`: this base image has no git
# binary. Cargo does not need one either — it fetches with libgit2, which reads
# the same url.*.insteadOf rules from this file.
RUN --mount=type=secret,id=gh_token \
    export GIT_CONFIG_GLOBAL=/tmp/gitcred && \
    tok=$(cat /run/secrets/gh_token) && \
    printf '[url "https://x-access-token:%s@github.com/"]\n\tinsteadOf = https://github.com/\n\tinsteadOf = ssh://git@github.com/\n' "$tok" > "$GIT_CONFIG_GLOBAL" && \
    cargo build --release --locked \
 && strip target/release/hanzod \
 && test -s target/release/hanzod \
 && rm -f /tmp/gitcred

# cc, not static: hanzod links libc and libgcc and nothing else, so the image is
# the binary plus the two libraries it names.
FROM gcr.io/distroless/cc-debian12
COPY --from=build /src/target/release/hanzod /usr/local/bin/hanzod
# The RPC and the validator mesh. A validator is named by the key it proves on
# every link, so the mesh port is not an admin surface and needs no gate of its
# own.
EXPOSE 9630 9631
ENTRYPOINT ["/usr/local/bin/hanzod"]
