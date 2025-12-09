# Use a Rust base image
FROM rust:bookworm as builder
ARG BUILD_TYPE
RUN apt-get update && apt-get install -y libclang-dev cmake libssl-dev libc++-dev libc++abi-dev lld

# Install nvm, npm and node
RUN rm /bin/sh && ln -s /bin/bash /bin/sh
ENV NVM_DIR /usr/local/nvm
ENV NODE_VERSION v22.13.1
RUN mkdir $NVM_DIR

RUN curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.4/install.sh | bash
RUN source $NVM_DIR/nvm.sh \
    && nvm install $NODE_VERSION \
    && nvm alias default $NODE_VERSION \
    && nvm use default

ENV NODE_PATH $NVM_DIR/v$NODE_VERSION/lib/node_modules
ENV PATH $NVM_DIR/versions/node/$NODE_VERSION/bin:$PATH
RUN node -v

# Create a new directory for your app
WORKDIR /app

# Clone the repository
COPY . .

# Build the dependencies (cached)
RUN cargo clean
RUN rustup default 1.88
RUN rustup component add rustfmt

# Build main code
RUN CARGO_BUILD_RERUN_IF_CHANGED=1 cargo build $([ "$BUILD_TYPE" = "release" ] && echo "--release")

# Pre-compile test binaries to avoid recompilation in CI test jobs
# This prevents disk space exhaustion during cargo test
RUN cargo test --no-run 2>/dev/null || true

# Aggressively clean up build artifacts to reduce image size
# This must happen AFTER test pre-compilation to ensure all artifacts are cleaned
# Use direct rm -rf on known paths to avoid find issues with nested dirs
RUN rm -rf /app/target/debug/incremental && \
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
