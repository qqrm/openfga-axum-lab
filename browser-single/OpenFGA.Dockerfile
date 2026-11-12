# Build only OpenFGA; reuse the verified emulator and application/rootfs image.
# Go's exact version is also asserted by build.sh; module bytes use go.sum.
FROM golang:1.24.6-bookworm AS build
ENV GOTOOLCHAIN=local CGO_ENABLED=0 GOOS=linux GOARCH=amd64
WORKDIR /src
RUN git init -q && git remote add origin https://github.com/openfga/openfga.git \
    && git fetch --depth=1 origin 293893b22a53d8a130a5f7c97cab4bc8e91bcaa4 \
    && git checkout --detach FETCH_HEAD \
    && test "$(git rev-parse HEAD)" = 293893b22a53d8a130a5f7c97cab4bc8e91bcaa4
COPY browser-single/openfga /patch
COPY fga/model.json /test-fixtures/model.json
RUN bash /patch/build.sh /out /test-fixtures/model.json
FROM scratch
COPY --from=build /out/ /
