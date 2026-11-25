FROM rust:1.88-bookworm AS builder

WORKDIR /build
COPY Cargo.toml clippy.toml ./
COPY src ./src

RUN cargo build --release

FROM debian:bookworm-slim

COPY --from=builder /build/target/release/kbwall /usr/local/bin/kbwall
COPY examples/kb.example.toml /etc/kbwall/kb.example.toml

ENTRYPOINT ["kbwall", "mcp"]
