FROM rust:1.93 AS build

WORKDIR /api

RUN apt-get update && apt-get install -y \
    libudev-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY api ./api

WORKDIR /api/api

ENV SQLX_OFFLINE=true
RUN cargo build --release

RUN cargo install solana-verify --git https://github.com/solana-foundation/solana-verifiable-build --tag v0.4.15

FROM debian:stable-slim AS final

WORKDIR /api

COPY --from=build /api/api/target/release/verified_programs_api .
COPY --from=build /usr/local/cargo/bin/solana-verify /usr/local/bin/solana-verify

RUN apt-get update && apt-get install -y docker.io ca-certificates && rm -rf /var/lib/apt/lists/*

CMD ["./verified_programs_api"]
