FROM rust:1.78-slim-bookworm AS builder

WORKDIR /app

COPY rust/Cargo.toml rust/Cargo.lock ./rust/
RUN mkdir -p rust/src && printf 'fn main() {}\n' > rust/src/main.rs
RUN cargo build --manifest-path rust/Cargo.toml --release

COPY rust ./rust
RUN cargo build --manifest-path rust/Cargo.toml --release

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -r -u 10001 -g root mcpway

COPY --from=builder /app/rust/target/release/mcpway /usr/local/bin/mcpway

USER 10001
EXPOSE 8000
ENTRYPOINT ["mcpway"]
CMD ["--help"]
