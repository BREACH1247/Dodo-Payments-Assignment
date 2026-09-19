FROM rust:1-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY migrations ./migrations
COPY tests ./tests
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --locked --release --bins && mkdir -p /artifacts && cp target/release/api target/release/mock-psp /artifacts/

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /artifacts/api /usr/local/bin/api
COPY --from=builder /artifacts/mock-psp /usr/local/bin/mock-psp
EXPOSE 3000 3001
