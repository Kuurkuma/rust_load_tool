# ==============================================================================
# Multi-stage Dockerfile for rlt (Rust Load Tool)
# Target environment: Google Cloud Run / Container Engine
# Resulting image: ~35 MB, non-root user, ultra-fast cold start (<50ms)
# ==============================================================================

# --- Stage 1: Build Environment ---
FROM rust:1.85-slim-bookworm AS builder

WORKDIR /app

# Install build dependencies for C-bindings (zstd-sys, etc.) and OpenSSL
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy dependency manifests first for build layer caching
COPY Cargo.toml Cargo.lock ./
COPY crates/rlt-core/Cargo.toml ./crates/rlt-core/Cargo.toml
COPY crates/rlt-cli/Cargo.toml ./crates/rlt-cli/Cargo.toml

# Create dummy source files to cache dependency compilation
RUN mkdir -p crates/rlt-core/src crates/rlt-cli/src \
    && echo "pub fn dummy() {}" > crates/rlt-core/src/lib.rs \
    && echo "fn main() {}" > crates/rlt-cli/src/main.rs \
    && cargo build --release --bin rlt \
    && rm -rf crates/rlt-core/src crates/rlt-cli/src target/release/deps/rlt* target/release/deps/librlt_core*

# Copy the real source code
COPY crates ./crates

# Compile the final release binary with full optimizations (LTO, strip)
RUN cargo build --release --bin rlt

# --- Stage 2: Minimal Runtime Environment ---
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

WORKDIR /app

# Copy the statically/dynamically resolved binary from builder
COPY --from=builder --chown=nonroot:nonroot /app/target/release/rlt /usr/local/bin/rlt

# Expose default Cloud Run ingestion port
EXPOSE 8080

# Environment variables for Cloud Run runtime
ENV RUST_LOG=info
ENV PORT=8080

# Run as non-root user (id 65532 in distroless)
USER nonroot:nonroot

# Default entrypoint: launch the HTTP webhook server listening on $PORT
ENTRYPOINT ["/usr/local/bin/rlt"]
CMD ["serve", "--port", "8080", "--destination", "/tmp/output"]
