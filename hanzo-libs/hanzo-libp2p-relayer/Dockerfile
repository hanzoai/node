# Use a Rust base image for building
FROM rust:bookworm AS builder
ARG BUILD_TYPE=release

# Install build dependencies
RUN apt-get update && apt-get install -y libclang-dev cmake libssl-dev libc++-dev libc++abi-dev lld

# Create a new directory for the app
WORKDIR /app

# Copy the workspace files (build context is workspace root)
COPY Cargo.toml Cargo.lock ./
COPY hanzo-libs ./hanzo-libs
COPY hanzo-bin ./hanzo-bin
COPY hanzo-test-framework ./hanzo-test-framework
COPY hanzo-test-macro ./hanzo-test-macro

# Build the hanzo-libp2p-relayer specifically
RUN rustup component add rustfmt
RUN cargo build --package hanzo_libp2p_relayer --bin hanzo_libp2p_relayer $([ "$BUILD_TYPE" = "release" ] && echo "--release")

# Runtime stage
FROM debian:bookworm-slim AS runner
ARG BUILD_TYPE=release

# Install runtime dependencies including curl for Deno installation
RUN apt-get update && apt-get install -y \
    libssl3 \
    ca-certificates \
    curl \
    unzip \
    && rm -rf /var/lib/apt/lists/*

# Install Deno
RUN curl -fsSL https://deno.land/install.sh | sh && \
    mv /root/.deno/bin/deno /usr/local/bin/deno && \
    chmod +x /usr/local/bin/deno

# Create a non-root user for security
RUN useradd -r -s /bin/false -m -d /app hanzo

# Create necessary directories and set permissions for both root and hanzo user
RUN mkdir -p /app/storage /app/internal_tools_storage /app/.cache/deno && \
    chown -R hanzo:hanzo /app && \
    chmod -R 755 /app

# Pre-cache ethers dependency to avoid runtime network issues
RUN echo 'import { ethers } from "npm:ethers@6.14.1"; console.log("Cached ethers");' > /tmp/cache_ethers.ts && \
    /usr/local/bin/deno run --allow-net --allow-read --allow-write --allow-env /tmp/cache_ethers.ts || true && \
    rm /tmp/cache_ethers.ts

# Copy the cache to the hanzo user's directory
RUN cp -r /root/.cache/deno /app/.cache/ && \
    chown -R hanzo:hanzo /app/.cache

# Copy only the necessary binary from builder
WORKDIR /app
COPY --from=builder /app/target/${BUILD_TYPE}/hanzo_libp2p_relayer /app/

# Change ownership to the hanzo user
RUN chown -R hanzo:hanzo /app

# Switch to the non-root user
USER hanzo

# Set environment variables with defaults
ENV PORT=9901
ENV IDENTITY_SECRET_KEY=""
ENV ENCRYPTION_SECRET_KEY=""
ENV NODE_NAME=""
ENV RPC_URL="https://sepolia.base.org"
ENV CONTRACT_ADDRESS="0x425fb20ba3874e887336aaa7f3fab32d08135ba9"
ENV MAX_CONNECTIONS=20
ENV HANZO_TOOLS_RUNNER_DENO_BINARY_PATH="/usr/local/bin/deno"
ENV NODE_STORAGE_PATH="/app/storage"
ENV DENO_DIR="/app/.cache/deno"

# Expose the port
EXPOSE ${PORT}

# Set entrypoint
ENTRYPOINT ["./hanzo_libp2p_relayer"] 