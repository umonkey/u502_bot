# syntax=docker/dockerfile:1

FROM rust:1-slim-trixie AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
 && echo 'fn main() {}' > src/main.rs \
 && cargo build --release \
 && rm -rf src
COPY src ./src
RUN touch src/main.rs \
 && cargo build --release

FROM debian:trixie-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends supervisor cron ca-certificates \
 && rm -rf /var/lib/apt/lists/*
RUN useradd --system --create-home --home-dir /home/u502 --shell /usr/sbin/nologin u502 \
 && mkdir -p /var/log/supervisor
COPY --from=builder /app/target/release/u502 /usr/local/bin/u502
COPY docker/supervisord.conf /etc/supervisor/supervisord.conf
COPY docker/cron/u502 /etc/cron.d/u502
RUN chmod 0644 /etc/cron.d/u502
ENV RUST_LOG=info
CMD ["supervisord", "-c", "/etc/supervisor/supervisord.conf"]
