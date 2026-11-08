FROM rust:1.99.1-slim-bookworm
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git jq procps \
    && rm -rf /var/lib/apt/lists/*
