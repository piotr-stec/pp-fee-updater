# Build stage
FROM rust:1.88-slim AS builder

# Install required system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /app

# Copy manifests first to cache dependencies
COPY Cargo.toml ./

# Create a dummy main.rs to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs

# Build dependencies (this layer will be cached)
RUN cargo build --release && rm -rf src target/release/deps/pp_fee_updater*

# Copy source code
COPY src ./src

# Build the actual application
RUN cargo build --release

# Runtime stage  
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && update-ca-certificates

# Create non-root user
RUN groupadd -r appuser && useradd -r -g appuser appuser

# Create app directory
WORKDIR /app

# Copy binary from builder stage
COPY --from=builder /app/target/release/pp-fee-updater /usr/local/bin/pp-fee-updater

# Set ownership
RUN chown appuser:appuser /usr/local/bin/pp-fee-updater

# Switch to non-root user
USER appuser

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD pgrep pp-fee-updater || exit 1

# Set environment variables
ENV RUST_LOG=info
ENV RUST_BACKTRACE=1

# Expose port (if needed for metrics in future)
# EXPOSE 8080

# Run the application
ENTRYPOINT ["pp-fee-updater"]
CMD []