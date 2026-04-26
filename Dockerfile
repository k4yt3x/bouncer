FROM rust:alpine as builder
COPY . /app
WORKDIR /app
RUN apk add --no-cache --virtual .build-deps \
        make \
        musl-dev \
    && cargo build --release --locked --target x86_64-unknown-linux-musl

FROM gcr.io/distroless/static:nonroot
LABEL maintainer="K4YT3X <i@k4yt3x.com>" \
      org.opencontainers.image.source="https://github.com/k4yt3x/bouncer" \
      org.opencontainers.image.description="Telegram join-request gatekeeper"
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/bouncer \
                    /usr/local/bin/bouncer
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/bouncer", "-c", "/data/bouncer.yaml", "-d", "/data/bouncer.db"]
