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

# Create a non-root user for security
RUN useradd -r -s /bin/false -m -d /app hanzo

# Create necessary directories and set permissions for both root and hanzo user
RUN mkdir -p /app/storage && \
    chown -R hanzo:hanzo /app && \
    chmod -R 755 /app

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
ENV NODE_STORAGE_PATH="/app/storage"

# Expose the port
EXPOSE ${PORT}

# Set entrypoint
ENTRYPOINT ["./hanzo_libp2p_relayer"] 