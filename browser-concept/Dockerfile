ARG RUST_IMAGE=rust:1.99.1-slim-bookworm
FROM ${RUST_IMAGE} AS build
SHELL ["/bin/bash", "-euo", "pipefail", "-c"]
WORKDIR /src
ENV CARGO_PROFILE_RELEASE_STRIP=symbols
RUN rustup component add rustfmt clippy
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
COPY tools ./tools
COPY assets ./assets
COPY db ./db
COPY migrations ./migrations
COPY fga ./fga
COPY benches ./benches
COPY browser-concept/*.json ./browser-concept/
RUN mkdir /out && rustc -vV > /out/rustc-version.txt && cargo -V > /out/cargo-version.txt
RUN rustfmt --edition 2024 --check src/bin/browser-concept.rs 2>&1 | tee /out/rustfmt.log
RUN cargo clippy --locked --release --bin browser-concept -- -D warnings 2>&1 | tee /out/clippy.log
RUN cargo test --locked --release --bin browser-concept -- --nocapture 2>&1 | tee /out/unit-tests.log
RUN cargo build --locked --release --bin browser-concept 2>&1 | tee /out/cargo-build.log && \
    install -m 0555 target/release/browser-concept /out/concept && \
    readelf -h /out/concept > /out/elf.txt && \
    readelf --version-info /out/concept >> /out/elf.txt

FROM scratch AS output
COPY --from=build /out/ /
