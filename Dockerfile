FROM rust:1.99-slim-bookworm AS build
WORKDIR /app
ARG APP_REVISION=unknown
ENV APP_REVISION=$APP_REVISION
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tools ./tools
COPY assets ./assets
COPY db ./db
COPY migrations ./migrations
COPY benches ./benches
ARG FGA_TRANSPORT=grpc
RUN cargo build --locked --release --bins --no-default-features --features "$FGA_TRANSPORT"
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
ARG APP_REVISION=unknown
LABEL org.opencontainers.image.source="https://github.com/qqrm/openfga-axum-lab" \
      org.opencontainers.image.revision=$APP_REVISION
COPY --from=build /app/target/release/openfga-axum-lab /usr/local/bin/app
COPY --from=build /app/target/release/lab-tool /usr/local/bin/lab-tool
COPY migrations /app/migrations
COPY fga /app/fga
COPY LICENSES /app/LICENSES
COPY NOTICE /app/NOTICE
WORKDIR /app
USER 65534:65534
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/app"]
