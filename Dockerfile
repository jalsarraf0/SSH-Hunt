# syntax=docker/dockerfile:1.7

FROM rust:bookworm AS builder
WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*

COPY ssh-hunt/Cargo.toml ssh-hunt/Cargo.toml
COPY ssh-hunt/rust-toolchain.toml ssh-hunt/rust-toolchain.toml
COPY ssh-hunt/crates ssh-hunt/crates
COPY ssh-hunt/migrations ssh-hunt/migrations
COPY ssh-hunt/tests ssh-hunt/tests

WORKDIR /build/ssh-hunt
RUN cargo build --workspace --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tzdata && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 --create-home --home-dir /home/sshhunt --shell /usr/sbin/nologin sshhunt

WORKDIR /app
COPY --from=builder /build/ssh-hunt/target/release/ssh-hunt-server /usr/local/bin/ssh-hunt-server
COPY --from=builder /build/ssh-hunt/target/release/admin /usr/local/bin/admin
COPY ssh-hunt/migrations /app/migrations

RUN mkdir -p /data /backups && chown -R 10001:10001 /data /backups /app
USER 10001:10001
EXPOSE 22222
ENTRYPOINT ["/usr/local/bin/ssh-hunt-server"]
