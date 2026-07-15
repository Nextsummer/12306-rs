FROM rust:1.96-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app
COPY . .
RUN cargo build --release -p rs12306-cli

FROM scratch AS binary

COPY --from=builder /app/target/release/12306-rs /12306-rs

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && useradd --system --uid 10001 --home /nonexistent --shell /usr/sbin/nologin rs12306 \
    && mkdir -p /data \
    && chown rs12306:rs12306 /data \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/12306-rs /usr/local/bin/12306-rs

ENV RS12306_HOST=0.0.0.0 \
    RS12306_PORT=12306 \
    RS12306_DATABASE=/data/12306-rs.sqlite \
    RUST_LOG=info

VOLUME ["/data"]
EXPOSE 12306
USER rs12306

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:12306/api/health || exit 1

CMD ["12306-rs", "serve"]
