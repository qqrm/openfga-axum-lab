# Experimental single-process-namespace image for container2wasm.
# Three real services; no Docker daemon or Compose inside the image.
FROM rust:1.99.1-slim-bookworm AS rust-build
WORKDIR /src
ENV CARGO_PROFILE_RELEASE_STRIP=symbols
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tools ./tools
COPY assets ./assets
COPY db ./db
COPY migrations ./migrations
COPY fga ./fga
COPY benches ./benches
RUN cargo build --locked --release --bins --no-default-features --features http

# Reuse the repository's tested OpenFGA release checksum instead of a
# mutable 'latest' tag or an unverified binary from another image.
FROM debian:bookworm-slim AS openfga-download
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
ARG OPENFGA_VERSION=1.10.3
ARG OPENFGA_ARCHIVE_SHA256=cbf0ddaddd9580dc0b9d6af70265853c53b4847ff1ac91a9fd06b97ed6f97efb
RUN set -eu; \
    curl --fail --silent --show-error --location --retry 3 \
      "https://github.com/openfga/openfga/releases/download/v${OPENFGA_VERSION}/openfga_${OPENFGA_VERSION}_linux_amd64.tar.gz" \
      --output /tmp/openfga.tar.gz; \
    printf '%s  %s\n' "$OPENFGA_ARCHIVE_SHA256" /tmp/openfga.tar.gz | sha256sum --check --status; \
    tar -xzf /tmp/openfga.tar.gz -C /tmp openfga; \
    chmod 0555 /tmp/openfga

# PostgreSQL's official runtime already contains initdb/pg_ctl. The Rust
# toolchain, compiler caches, archives and package manager never enter this image.
FROM postgres:17-bookworm
LABEL org.opencontainers.image.source="https://github.com/qqrm/openfga-axum-lab" \
      org.opencontainers.image.description="Experimental ephemeral one-container browser laboratory; not production"
COPY --from=rust-build /src/target/release/openfga-axum-lab /usr/local/bin/app
COPY --from=rust-build /src/target/release/lab-tool /usr/local/bin/lab-tool
COPY --from=openfga-download /tmp/openfga /usr/local/bin/openfga
COPY migrations /app/migrations
COPY fga /app/fga
COPY NOTICE /app/NOTICE
COPY --chmod=0555 browser-single/entrypoint.sh /usr/local/bin/browser-lab-entrypoint
RUN install -d -o postgres -g postgres -m 0700 /tmp/browser-lab
WORKDIR /app
USER postgres:postgres
# Do not write to the base image's automatically declared PostgreSQL VOLUME.
# No persistent data or application credentials are baked into the image.
ENV PGDATA=/tmp/browser-lab/postgres \
    DEMO_PASSWORD_FILE=/tmp/browser-lab/demo-password \
    FGA_CONFIG=/tmp/browser-lab/config.json \
    FGA_MODEL=/app/fga/model.json
EXPOSE 3000
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=10s --timeout=6s --start-period=90s \
    CMD ["/usr/local/bin/lab-tool", "probe"]
ENTRYPOINT ["/usr/local/bin/browser-lab-entrypoint"]
