# Stage 1: Build Rust backend  
FROM rust:latest as backend-builder

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y \
    clang \
    libclang-dev \
    binutils \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy backend source
COPY pipeline-backend/ ./backend/
WORKDIR /app/backend

# Build backend
RUN cargo build --release

# Stage 2: Build Solid.js frontend
FROM node:20-slim as frontend-builder

WORKDIR /app

# Copy frontend source
COPY pipeline-frontend/ ./frontend/
WORKDIR /app/frontend

# Install dependencies
RUN npm ci

# Build frontend
RUN npm run build

# Stage 3: Final runtime image
FROM debian:trixie-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    clang \
    binutils \
    libssl3 \
    ca-certificates \
    curl \
    tini \
    && rm -rf /var/lib/apt/lists/*

# Create app user
RUN useradd -m -u 1000 pipeline

WORKDIR /app

# Copy from builder stages
COPY --from=backend-builder /app/backend/target/release/pipeline-backend ./pipeline-backend
COPY --from=frontend-builder /app/frontend/dist ./frontend

# Set permissions
RUN chown -R pipeline:pipeline /app

# Switch to non-root user
USER pipeline

# Expose ports
EXPOSE 8080

# Environment variables
ENV BIND_ADDRESS=0.0.0.0:8080
ENV LOG_LEVEL=info
# Serve the built SPA from the image (single-container deploy).
ENV FRONTEND_DIR=/app/frontend
# WORKERS and MAX_CONCURRENT_COMPILES are intentionally NOT set here: the
# backend detects the container memory limit at startup and picks 1/1 below
# 1024 MB (and 2/2 otherwise). Pin them explicitly only to override that, or set
# LOW_MEMORY=1 to force the smallest profile.
ENV RATE_LIMIT_PER_MINUTE=30
ENV MAX_AST_BYTES=2097152

# Memory: run this container with an explicit limit, e.g.
#   docker run --memory 1g --memory-swap 1g -p 8080:8080 pipeline-backend
# 512 MB is the practical floor; 1 GB is recommended. Memory is enforced by the
# container limit, not by RLIMIT_AS (see CompilerOrchestrator docs).

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/api/health || exit 1

# Run backend under tini as PID 1.
#
# The server is PID 1 in the container. When a run's process group is killed,
# any children (e.g. from a fork bomb) are reparented to PID 1; without an init
# that wait()s for them they linger as zombies and permanently exhaust the
# container's PID budget. tini reaps them, so the container recovers.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["./pipeline-backend"]
