# =============================================================================
# Dockerfile for Claude Code Portal Backend
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

# Copy pre-built backend binary from CI (frontend assets are embedded)
COPY build-output/backend /app/backend

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
