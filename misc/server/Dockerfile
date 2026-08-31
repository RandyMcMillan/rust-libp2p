# syntax=docker/dockerfile:1.5-labs
FROM rust:1.88.0 as chef
RUN cargo install cargo-chef --version 0.1.78 --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --release --package libp2p-server --recipe-path recipe.json
# Build application
COPY . .
RUN cargo build --release --package libp2p-server

FROM gcr.io/distroless/cc
COPY --from=builder /app/target/release/libp2p-server /bin/libp2p-server
CMD ["libp2p-server"]
