FROM rust:1.94-bookworm AS builder
WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY smelly-connect ./smelly-connect
COPY smelly-connect-cli ./smelly-connect-cli
COPY smelly-tls ./smelly-tls

RUN cargo build -p smelly-connect-cli --release --features management-api

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --home-dir /var/lib/smelly-connect --create-home smelly-connect

COPY --from=builder /build/target/release/smelly-connect-cli /usr/local/bin/smelly-connect-cli
COPY config.toml.example /etc/smelly-connect/config.toml.example

USER smelly-connect
WORKDIR /var/lib/smelly-connect

ENTRYPOINT ["/usr/local/bin/smelly-connect-cli"]
CMD ["--config", "/etc/smelly-connect/config.toml", "proxy"]
