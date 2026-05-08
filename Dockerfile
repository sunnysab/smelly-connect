FROM rust:1.94-alpine AS builder
WORKDIR /build

RUN apk add --no-cache \
    build-base \
    musl-dev \
    openssl-dev \
    openssl-libs-static \
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
    export OPENSSL_STATIC=1 PKG_CONFIG_ALL_STATIC=1; \
    cargo build -p smelly-connect-cli --release --features management-api --target "$rust_target"; \
    install -Dm755 "target/$rust_target/release/smelly-connect-cli" /out/smelly-connect-cli

FROM alpine:3.22

RUN addgroup -S smelly-connect \
    && adduser -S -D -h /var/lib/smelly-connect -G smelly-connect smelly-connect \
    && install -d -o smelly-connect -g smelly-connect /var/lib/smelly-connect

COPY --from=builder /out/smelly-connect-cli /usr/local/bin/smelly-connect-cli
COPY config.toml.example /etc/smelly-connect/config.toml.example

USER smelly-connect
WORKDIR /var/lib/smelly-connect

ENTRYPOINT ["/usr/local/bin/smelly-connect-cli"]
CMD ["--config", "/etc/smelly-connect/config.toml", "proxy"]
