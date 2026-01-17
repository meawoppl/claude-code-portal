# =============================================================================
# Multi-stage Dockerfile for CC-Proxy Backend
# Downloads pre-built claude-proxy binary from GitHub releases
# =============================================================================

# -----------------------------------------------------------------------------
# Stage 1: Builder (backend only)
# -----------------------------------------------------------------------------
FROM rust:1.92-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && \
    apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files (only what's needed for backend)
COPY Cargo.toml Cargo.lock ./
COPY shared ./shared
COPY backend ./backend
COPY frontend ./frontend

# Remove proxy and cli-tools from workspace (not needed - proxy downloaded from releases)
RUN sed -i 's/, "cli-tools"//' Cargo.toml && \
    sed -i 's/, "proxy"//' Cargo.toml

# Build release binary (backend only)
RUN cargo build --release -p backend

# -----------------------------------------------------------------------------
# Stage 2: Runtime
# -----------------------------------------------------------------------------
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y \
    ca-certificates \
    libpq5 \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy backend binary from builder
COPY --from=builder /app/target/release/backend /app/backend

# Download pre-built claude-proxy binary from GitHub releases
# This ensures consistency with the published binaries and faster builds
RUN mkdir -p /app/bin && \
    curl -fsSL https://github.com/meawoppl/cc-proxy/releases/download/latest/claude-proxy-linux-x86_64 \
    -o /app/bin/claude-proxy && \
    chmod +x /app/bin/claude-proxy

# Copy pre-built frontend dist (built locally with trunk)
COPY frontend/dist /app/frontend/dist

# Set proxy binary path for the download endpoint
ENV PROXY_BINARY_PATH=/app/bin/claude-proxy

# Create non-root user
RUN useradd -m -u 1001 -s /bin/bash appuser && \
    chown -R appuser:appuser /app

USER appuser

# Expose port
EXPOSE 3000

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3000/ || exit 1

# Run the backend (expects environment variables to be passed in)
CMD ["/app/backend"]
