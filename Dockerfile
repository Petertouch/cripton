# Stage 1: Build
FROM rust:1.94-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY src/ src/

RUN cargo build --release --bin cripton

# Stage 2: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

RUN groupadd -r cripton && useradd -r -g cripton cripton

COPY --from=builder /app/target/release/cripton /usr/local/bin/cripton

USER cripton

ENV RUST_LOG=info
ENV PAPER_MODE=true

EXPOSE 3001

ENTRYPOINT ["cripton"]
