# Use a Rust base image
FROM rust:bookworm AS builder
ARG BUILD_TYPE
RUN apt-get update && apt-get install -y libclang-dev cmake libssl-dev libc++-dev libc++abi-dev lld

# Install nvm, npm and node
RUN rm /bin/sh && ln -s /bin/bash /bin/sh
ENV NVM_DIR=/usr/local/nvm
ENV NODE_VERSION=v22.13.1
RUN mkdir $NVM_DIR

RUN curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.4/install.sh | bash
RUN source $NVM_DIR/nvm.sh \
    && nvm install $NODE_VERSION \
    && nvm alias default $NODE_VERSION \
    && nvm use default

ENV NODE_PATH=$NVM_DIR/v$NODE_VERSION/lib/node_modules
ENV PATH=$NVM_DIR/versions/node/$NODE_VERSION/bin:/root/.local/bin:$PATH
RUN node -v

# Pre-install MCP test server dependencies so npx doesn't fail with missing modules
RUN npx -y @modelcontextprotocol/server-everything@2025.9.12 --help 2>/dev/null || true

# Create a new directory for your app
WORKDIR /app

# Clone the repository
COPY . .

# Set up Rust toolchain
RUN rustup default 1.88
RUN rustup component add rustfmt

# Build main code, pre-compile tests, and cleanup in SINGLE layer
# This is critical - separate RUN commands create Docker layers that preserve old data
# Even if we delete files in a later layer, the image size includes all layers
RUN CARGO_BUILD_RERUN_IF_CHANGED=1 cargo build $([ "$BUILD_TYPE" = "release" ] && echo "--release") && \
    cargo test --no-run 2>/dev/null || true && \
    rm -rf /app/target/debug/incremental && \
    rm -rf /app/target/debug/.fingerprint && \
    rm -rf /app/target/debug/build && \
    rm -rf /app/target/debug/examples && \
    rm -rf /app/target/debug/deps/*.d && \
    rm -rf /app/target/.rustc_info.json && \
    rm -rf /root/.cargo/registry/cache && \
    rm -rf /root/.cargo/git/checkouts && \
    rm -rf /root/.cargo/registry/index && \
    echo "=== Target size after cleanup ===" && \
    du -sh /app/target/ && \
    du -sh /app/target/debug/ && \
    ls -la /app/target/debug/ | head -20

COPY .github/run-main*.sh /entrypoints/
RUN chmod 755 /entrypoints/*.sh
