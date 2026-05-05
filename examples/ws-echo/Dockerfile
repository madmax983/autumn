FROM rust:1.88-bookworm AS builder
WORKDIR /workspace

COPY . .
RUN cargo build --release -p ws-echo

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libpq5 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /workspace/target/release/ws-echo /app/ws-echo
COPY --from=builder /workspace/examples/ws-echo /app/examples/ws-echo

ENV AUTUMN_MANIFEST_DIR=/app/examples/ws-echo
ENV AUTUMN_SERVER__HOST=0.0.0.0

CMD ["/app/ws-echo"]
