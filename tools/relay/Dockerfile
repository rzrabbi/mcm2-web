FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml .
COPY src ./src
RUN cargo build --release && strip target/release/mm2-relay

FROM debian:bookworm-slim
WORKDIR /app
ENV HOST=0.0.0.0
COPY --from=build /src/target/release/mm2-relay /usr/local/bin/mm2-relay
EXPOSE 2108
CMD ["mm2-relay", "--port", "2108"]
