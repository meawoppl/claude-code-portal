# =============================================================================
# Dockerfile for CC-Proxy Backend
# Uses pre-built binaries from CI (faster builds with caching)
# =============================================================================

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

# Copy pre-built backend binary from CI
COPY build-output/backend /app/backend

# Download pre-built claude-proxy binary from GitHub releases
RUN mkdir -p /app/bin && \
    curl -fsSL https://github.com/meawoppl/cc-proxy/releases/download/latest/claude-proxy-linux-x86_64 \
    -o /app/bin/claude-proxy && \
    chmod +x /app/bin/claude-proxy

# Copy pre-built frontend dist from CI
COPY build-output/frontend-dist /app/frontend/dist

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
