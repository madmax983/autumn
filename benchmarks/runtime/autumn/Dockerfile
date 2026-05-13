FROM rust:1.88-bookworm AS builder
WORKDIR /workspace

# Copy workspace manifests first for layer caching.
COPY ../../Cargo.toml ../../Cargo.lock /workspace/
COPY ../../autumn /workspace/autumn
COPY ../../autumn-macros /workspace/autumn-macros

COPY . /workspace/benchmarks/runtime/autumn/

RUN cargo build --release -p bench-autumn

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libpq5 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /workspace/target/release/bench-autumn /app/bench-autumn
COPY --from=builder /workspace/benchmarks/runtime/autumn /app/bench-autumn-src

ENV AUTUMN_MANIFEST_DIR=/app/bench-autumn-src
ENV AUTUMN_SERVER__HOST=0.0.0.0
ENV AUTUMN_SERVER__PORT=8080

EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=5s --start-period=30s \
  CMD curl -sf http://localhost:8080/health || exit 1

CMD ["/app/bench-autumn"]
