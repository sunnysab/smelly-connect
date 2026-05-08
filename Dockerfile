FROM rust:1.95-alpine AS builder
WORKDIR /build

RUN apk add --no-cache \
    build-base \
    musl-dev \
    pkgconfig

COPY Cargo.toml Cargo.lock ./
COPY smelly-connect ./smelly-connect
COPY smelly-connect-cli ./smelly-connect-cli
COPY smelly-tls ./smelly-tls

RUN set -eux; \
    rust_target="$(rustc -vV | sed -n 's/^host: //p')"; \
    case "$rust_target" in \
        *-unknown-linux-musl) ;; \
        *) echo "expected a musl host target, got: $rust_target" >&2; exit 1 ;; \
    esac; \
    cargo build -p smelly-connect-cli --release --features management-api --target "$rust_target"; \
    install -Dm755 "target/$rust_target/release/smelly-connect-cli" /out/smelly-connect-cli

FROM alpine:3.23

RUN addgroup -S vpn \
    && adduser -S -D -h /var/lib/vpn -G vpn vpn \
    && install -d -o vpn -g vpn /var/lib/vpn /run/smelly-connect

COPY --from=builder /out/smelly-connect-cli /usr/local/bin/smelly-connect-cli
COPY config.toml.example /etc/smelly-connect/config.toml.example
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh

RUN chmod 755 /usr/local/bin/docker-entrypoint.sh

WORKDIR /var/lib/vpn

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["--config", "/etc/smelly-connect/config.toml", "proxy"]
