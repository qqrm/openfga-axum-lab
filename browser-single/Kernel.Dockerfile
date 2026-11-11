# Rebuild only the converter's Linux 6.1 kernel with PostgreSQL's required IPC.
# Emulator and application/rootfs bytes are kept from the verified artifact.
FROM ubuntu:22.04 AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc-x86-64-linux-gnu libc6-dev make git curl ca-certificates \
    gperf flex bison bc libelf-dev libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /kernel
RUN git init && git remote add origin https://github.com/torvalds/linux.git \
    && git fetch --depth=1 origin 830b3c68c1fb1e9176028d02ef86f3cf76aa2476 \
    && git checkout --detach FETCH_HEAD \
    && test "$(git rev-parse HEAD)" = 830b3c68c1fb1e9176028d02ef86f3cf76aa2476
RUN curl --fail --silent --show-error --location --retry 3 \
      https://raw.githubusercontent.com/container2wasm/container2wasm/087c313479d51cdf255e4f28ded6ae7d8141297b/config/qemu/linux_x86_config \
      --output .config \
    && test "$(git hash-object .config)" = 48dd1bf6191dd8d0d220358167f52dce36c69c45 \
    && cp .config /tmp/upstream.config
ENV KBUILD_BUILD_USER=authzlab KBUILD_BUILD_HOST=browser \
    KBUILD_BUILD_TIMESTAMP="2022-12-11 22:15:18 UTC"
RUN scripts/config --enable SYSVIPC --enable SYSVIPC_SYSCTL --enable IPC_NS \
      --set-str LOCALVERSION '-authz-ipc' --disable LOCALVERSION_AUTO \
    && make ARCH=x86 CROSS_COMPILE=x86_64-linux-gnu- olddefconfig \
    && grep -qx 'CONFIG_SYSVIPC=y' .config \
    && grep -qx 'CONFIG_IPC_NS=y' .config \
    && make ARCH=x86 CROSS_COMPILE=x86_64-linux-gnu- -j2 bzImage \
    && mkdir /out \
    && cp arch/x86/boot/bzImage /out/bzImage \
    && cp .config /out/linux.config \
    && cp /tmp/upstream.config /out/upstream.config \
    && cp COPYING /out/COPYING \
    && cp LICENSES/preferred/GPL-2.0 /out/GPL-2.0 \
    && printf '%s\n' 'Linux 6.1, source commit 830b3c68c1fb1e9176028d02ef86f3cf76aa2476; unmodified source, configuration enables SYSVIPC and IPC_NS. Rebuild with browser-single/Kernel.Dockerfile.' > /out/SOURCE.txt \
    && sha256sum /out/bzImage /out/linux.config > /out/build.sha256
FROM scratch
COPY --from=build /out/ /
