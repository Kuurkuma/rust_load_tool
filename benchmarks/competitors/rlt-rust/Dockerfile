# ==============================================================================
# rlt (Rust) Ingestion Service Dockerfile for Benchmarks
# Multi-stage optimized release build -> distroless/cc-debian12
# ==============================================================================
FROM rust:1.85-slim-bookworm AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY tests ./tests

RUN cargo build --release --bin rlt

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

WORKDIR /app
COPY --from=builder --chown=nonroot:nonroot /app/target/release/rlt /usr/local/bin/rlt

ENV RUST_LOG=info \
    PORT=8080

EXPOSE 8080
USER nonroot:nonroot

ENTRYPOINT ["/usr/local/bin/rlt"]
CMD ["serve", "--port", "8080", "--destination", "/tmp/rlt_output"]
