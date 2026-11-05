FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml .
COPY src ./src
RUN cargo build --release && strip target/release/mm2-relay

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --no-create-home relay
WORKDIR /app
ENV HOST=0.0.0.0
COPY --from=build /src/target/release/mm2-relay /usr/local/bin/mm2-relay
USER relay
EXPOSE 2108
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD ["mm2-relay", "--healthcheck"]
CMD ["mm2-relay", "--port", "2108"]
