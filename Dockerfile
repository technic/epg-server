# Multi-stage Dockerfile combining build and runner
FROM rust:1.53-bullseye AS builder

RUN apt-get update && apt-get install -y \
    build-essential \
    libssl-dev \
    libsqlite3-dev \
    pkg-config \
    nodejs \
    npm \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY . .

RUN OPENSSL_STATIC=1 \
    OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu \
    OPENSSL_INCLUDE_DIR=/usr/include/openssl \
    SQLITE3_STATIC=1 \
    SQLITE3_LIB_DIR=/usr/lib/x86_64-linux-gnu \
    SQLITE3_INCLUDE_DIR=/usr/include \
    cargo build --release

RUN npm install && npm run-script build

RUN mkdir -p /dist && \
    cp target/release/epg-server /dist/ && \
    cp -a static /dist/ && \
    cp backup-db.sh /dist/ && \
    chmod +x /dist/backup-db.sh

# Runtime stage
FROM debian:bullseye-slim

RUN apt-get update && apt-get install -y \
    libssl1.1 \
    libsqlite3-0 \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

RUN useradd -m -u 10000 appuser

COPY --from=builder /dist/ /app/

RUN mkdir /app/data && chown appuser:appuser /app/data

VOLUME ["/app/data"]

USER appuser

EXPOSE 3000

HEALTHCHECK --interval=5m --timeout=30s --start-period=10m --retries=3 \
    CMD curl -f http://localhost:3000/ || exit 1

CMD ["/app/epg-server", "--address=0.0.0.0", "--db=/app/data/epg.db"]
