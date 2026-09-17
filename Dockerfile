FROM rust:1-bookworm AS builder

WORKDIR /build

COPY Cargo.toml Cargo.lock ./

COPY src ./src
COPY templates ./templates
COPY migrations ./migrations

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked && \
    cp target/release/tg-voting /tmp/tg-voting


FROM debian:bookworm-slim AS runtime

RUN apt-get update && \
    apt-get install --yes --no-install-recommends \
        ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /tmp/tg-voting /usr/local/bin/tg-voting

ENV BIND_HOST=0.0.0.0
ENV BIND_PORT=3000
ENV LOG_LEVEL=info

EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl --fail --silent --show-error "http://127.0.0.1:${BIND_PORT:-3000}/health/live" && \
        curl --fail --silent --show-error "http://127.0.0.1:${BIND_PORT:-3000}/health/ready" || exit 1

ENTRYPOINT ["/usr/local/bin/tg-voting"]
