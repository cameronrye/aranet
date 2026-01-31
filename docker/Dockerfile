# Aranet Service Dockerfile
# Build: docker build -t aranet-service -f docker/Dockerfile .
# Run: docker run -d --name aranet -p 8080:8080 -v aranet-data:/data aranet-service

FROM rust:1.90-slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libdbus-1-dev \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy manifests
COPY Cargo.toml Cargo.lock ./
COPY crates/aranet-types/Cargo.toml crates/aranet-types/
COPY crates/aranet-core/Cargo.toml crates/aranet-core/
COPY crates/aranet-store/Cargo.toml crates/aranet-store/
COPY crates/aranet-service/Cargo.toml crates/aranet-service/
COPY crates/aranet-cli/Cargo.toml crates/aranet-cli/
COPY crates/aranet-tui/Cargo.toml crates/aranet-tui/
COPY crates/aranet-gui/Cargo.toml crates/aranet-gui/

# Create dummy files for dependency caching
RUN mkdir -p crates/aranet-types/src && echo "pub fn dummy() {}" > crates/aranet-types/src/lib.rs && \
    mkdir -p crates/aranet-core/src && echo "pub fn dummy() {}" > crates/aranet-core/src/lib.rs && \
    mkdir -p crates/aranet-store/src && echo "pub fn dummy() {}" > crates/aranet-store/src/lib.rs && \
    mkdir -p crates/aranet-service/src && echo "fn main() {}" > crates/aranet-service/src/main.rs && echo "pub fn dummy() {}" > crates/aranet-service/src/lib.rs && \
    mkdir -p crates/aranet-cli/src && echo "fn main() {}" > crates/aranet-cli/src/main.rs && echo "pub fn dummy() {}" > crates/aranet-cli/src/lib.rs && \
    mkdir -p crates/aranet-tui/src && echo "fn main() {}" > crates/aranet-tui/src/main.rs && \
    mkdir -p crates/aranet-gui/src && echo "fn main() {}" > crates/aranet-gui/src/main.rs

# Build dependencies only
RUN cargo build --release -p aranet-service --features full || true

# Copy actual source code
COPY crates/aranet-types/src crates/aranet-types/src
COPY crates/aranet-core/src crates/aranet-core/src
COPY crates/aranet-store/src crates/aranet-store/src
COPY crates/aranet-service/src crates/aranet-service/src

# Touch source files to invalidate cache
RUN touch crates/aranet-types/src/lib.rs && \
    touch crates/aranet-core/src/lib.rs && \
    touch crates/aranet-store/src/lib.rs && \
    touch crates/aranet-service/src/lib.rs && \
    touch crates/aranet-service/src/main.rs

# Build the actual service
RUN cargo build --release -p aranet-service --features full

# Runtime image
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    libdbus-1-3 \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -r -s /bin/false aranet

# Create data directory
RUN mkdir -p /data && chown aranet:aranet /data

WORKDIR /app

# Copy binary
COPY --from=builder /app/target/release/aranet-service /app/aranet-service

# Copy default config (optional)
COPY docker/server.toml.example /app/server.toml.example

# Switch to non-root user
USER aranet

# Expose port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/api/health || exit 1

# Default environment
ENV RUST_LOG=aranet_service=info

# Run the service
# Note: In Docker, Bluetooth is not available. Use --no-collector for API-only mode
# or mount the host's Bluetooth socket for BLE access.
ENTRYPOINT ["/app/aranet-service"]
CMD ["--bind", "0.0.0.0:8080", "--database", "/data/aranet.db"]
