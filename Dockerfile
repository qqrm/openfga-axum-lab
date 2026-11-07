FROM rust:1.99-slim-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tools ./tools
COPY assets ./assets
COPY db ./db
COPY migrations ./migrations
RUN cargo build --locked --release --bins
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/openfga-axum-lab /usr/local/bin/app
COPY --from=build /app/target/release/lab-tool /usr/local/bin/lab-tool
COPY migrations /app/migrations
COPY fga /app/fga
WORKDIR /app
USER 65534:65534
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/app"]
