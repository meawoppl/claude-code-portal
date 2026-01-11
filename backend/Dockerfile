# =============================================================================
# Multi-stage Dockerfile for CC-Proxy Backend
# Uses 1Password CLI for secret injection at runtime
# =============================================================================

# -----------------------------------------------------------------------------
# Stage 1: Builder
# -----------------------------------------------------------------------------
FROM rust:1.85-slim AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && \
    apt-get install -y \
    pkg-config \
    libssl-dev \
    libpq-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY shared ./shared
COPY backend ./backend
COPY frontend ./frontend
COPY proxy ./proxy

# Remove cli-tools from workspace (not needed for backend build)
RUN sed -i 's/, "cli-tools"//' Cargo.toml

# Build release binaries (backend and proxy)
RUN cargo build --release -p backend -p proxy

# -----------------------------------------------------------------------------
# Stage 2: Runtime
# -----------------------------------------------------------------------------
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies and 1Password CLI
RUN apt-get update && \
    apt-get install -y \
    ca-certificates \
    libpq5 \
    libssl3 \
    curl \
    gnupg \
    && rm -rf /var/lib/apt/lists/*

# Install 1Password CLI
RUN curl -sS https://downloads.1password.com/linux/keys/1password.asc | \
    gpg --dearmor --output /usr/share/keyrings/1password-archive-keyring.gpg && \
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/1password-archive-keyring.gpg] https://downloads.1password.com/linux/debian/$(dpkg --print-architecture) stable main" | \
    tee /etc/apt/sources.list.d/1password.list && \
    apt-get update && \
    apt-get install -y 1password-cli && \
    rm -rf /var/lib/apt/lists/*

# Create bin directory and copy binaries from builder
RUN mkdir -p /app/bin
COPY --from=builder /app/target/release/backend /app/backend
COPY --from=builder /app/target/release/claude-proxy /app/bin/claude-proxy

# Copy .env template with 1Password references
COPY .env.example /app/.env

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

# Use 1Password CLI to inject secrets and run the backend
# Note: Requires 1Password Connect or Service Account token at runtime
ENTRYPOINT ["op", "run", "--env-file=/app/.env", "--"]
CMD ["/app/backend"]
