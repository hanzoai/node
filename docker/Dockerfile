# ============================================================================
# Hanzo Gateway Node - Multi-stage Docker Build
# ============================================================================
# This Dockerfile builds the Hanzo Node from source with pure environment
# variable configuration. No config files required at runtime.
#
# Build Arguments:
#   BUILD_TYPE: 'release' or 'debug' (default: debug)
#   HANZO_VERSION: Semantic version (e.g., 1.0.0)
#
# Runtime Environment Variables:
#   See docker/.env.example or run_node.sh for full list
# ============================================================================

# ============================================================================
# Stage 1: Builder
# ============================================================================
FROM rust:bookworm as builder

# Build configuration
ARG BUILD_TYPE=debug
ARG HANZO_VERSION=dev
ARG BUILD_DATE
ARG VCS_REF

# Install build dependencies
RUN apt-get update && apt-get install -y \
    libclang-dev \
    cmake \
    libssl-dev \
    libc++-dev \
    libc++abi-dev \
    lld \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /build

# Copy source code
# Note: Build context should be the hanzo-node repository root
COPY . .

# Add rustfmt for build
RUN rustup component add rustfmt

# Build the project
# Build the entire workspace to properly resolve all dependencies
# Then extract just the hanzo_node binary we need
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --locked --bin hanzo-node $([ "$BUILD_TYPE" = "release" ] && echo "--release") && \
    cp target/${BUILD_TYPE}/hanzo-node /build/hanzo-node && \
    cp -r target/${BUILD_TYPE}/hanzo-tools-runner-resources /build/hanzo-tools-runner-resources || true

# ============================================================================
# Stage 2: Runtime
# ============================================================================
FROM debian:bookworm-slim as runner

# Metadata
ARG BUILD_TYPE=debug
ARG HANZO_VERSION=dev
ARG BUILD_DATE
ARG VCS_REF

LABEL org.opencontainers.image.title="Hanzo Gateway" \
      org.opencontainers.image.description="Multi-provider AI inference platform for Hanzo ecosystem" \
      org.opencontainers.image.version="${HANZO_VERSION}" \
      org.opencontainers.image.created="${BUILD_DATE}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.vendor="Hanzo AI" \
      org.opencontainers.image.url="https://hanzo.ai" \
      org.opencontainers.image.source="https://github.com/hanzoai/gateway" \
      org.opencontainers.image.licenses="MIT"

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    libssl3 \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN useradd -m -u 1000 -s /bin/bash hanzo

# Set working directory
WORKDIR /app

# Copy runtime script
COPY docker/run_node.sh /app/

# Copy built binaries and resources from builder
COPY --from=builder /build/hanzo-node /app/
COPY --from=builder /build/hanzo-tools-runner-resources /app/hanzo-tools-runner-resources

# Make script executable
RUN chmod +x /app/run_node.sh /app/hanzo-node

# Set ownership
RUN chown -R hanzo:hanzo /app

# Switch to non-root user
USER hanzo

# Environment variable defaults (can be overridden at runtime)
ENV BUILD_TYPE=${BUILD_TYPE} \
    HANZO_VERSION=${HANZO_VERSION} \
    NODE_API_IP=0.0.0.0 \
    NODE_IP=0.0.0.0 \
    NODE_API_PORT=9550 \
    NODE_WS_PORT=9551 \
    NODE_PORT=9552 \
    NODE_HTTPS_PORT=9553 \
    GLOBAL_IDENTITY_NAME=@@my_local_ai.sep-hanzo \
    RUST_LOG=debug,error,info \
    LOG_SIMPLE=true \
    NO_SECRET_FILE=true \
    STARTING_NUM_QR_PROFILES=1 \
    STARTING_NUM_QR_DEVICES=1 \
    FIRST_DEVICE_NEEDS_REGISTRATION_CODE=false \
    NODE_STORAGE_PATH=hanzo-storage \
    INITIAL_AGENT_NAMES=do_qwen32b \
    INITIAL_AGENT_URLS=https://inference.do-ai.run \
    INITIAL_AGENT_MODELS=openai:alibaba-qwen3-32b

# Expose ports
EXPOSE 9550 9551 9552 9553

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=40s --retries=3 \
    CMD curl -f http://localhost:${NODE_API_PORT}/v2/health_check || exit 1

# Volume for persistent storage
VOLUME ["/app/hanzo-storage"]

# Set entrypoint
ENTRYPOINT ["/app/run_node.sh"]
