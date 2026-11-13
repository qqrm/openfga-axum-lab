# Rebuild Linux 6.1 with IPC, normal ACPI timer discovery and VirtIO entropy.
# Emulator and application/rootfs bytes are kept from the verified artifact.
FROM ubuntu:22.04 AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc-x86-64-linux-gnu libc6-dev make git curl ca-certificates \
    gperf flex bison bc libelf-dev libssl-dev python3 \
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
    && cp .config /tmp/ipc-baseline.config \
    && scripts/config --enable ACPI --enable X86_PM_TIMER \
    && make ARCH=x86 CROSS_COMPILE=x86_64-linux-gnu- olddefconfig \
    && scripts/diffconfig /tmp/ipc-baseline.config .config > /tmp/acpi.config.diff \
    && cp .config /tmp/acpi-baseline.config \
    && printf '%s\n' 'fbee1ad0bc5fb73f2ca82ba98fa045b76ff19e67a148b40cb93b63916c2ad9b8  .config' | sha256sum --check --status \
    && scripts/config --enable HW_RANDOM_VIRTIO \
    && make ARCH=x86 CROSS_COMPILE=x86_64-linux-gnu- olddefconfig \
    && scripts/diffconfig /tmp/acpi-baseline.config .config > /tmp/rng.config.diff \
    && test "$(cat /tmp/rng.config.diff)" = ' HW_RANDOM_VIRTIO n -> y' \
    && for option in \
         CONFIG_SYSVIPC=y CONFIG_SYSVIPC_SYSCTL=y CONFIG_IPC_NS=y \
         CONFIG_ACPI=y CONFIG_X86_PM_TIMER=y CONFIG_HPET_TIMER=y \
         CONFIG_HW_RANDOM=y CONFIG_HW_RANDOM_VIRTIO=y CONFIG_VIRTIO=y CONFIG_VIRTIO_PCI=y \
         CONFIG_X86_64=y CONFIG_SMP=y CONFIG_X86_MPPARSE=y \
         CONFIG_X86_LOCAL_APIC=y CONFIG_X86_IO_APIC=y \
         CONFIG_HZ_PERIODIC=y CONFIG_HZ_100=y CONFIG_HZ=100 \
         '# CONFIG_HIGH_RES_TIMERS is not set' \
         '# CONFIG_NO_HZ_IDLE is not set' '# CONFIG_NO_HZ_FULL is not set'; do \
         grep -qx "$option" .config || exit 1; \
       done \
    && make ARCH=x86 CROSS_COMPILE=x86_64-linux-gnu- -j2 bzImage \
    && mkdir /out \
    && cp arch/x86/boot/bzImage /out/bzImage \
    && cp .config /out/linux.config \
    && cp /tmp/upstream.config /out/upstream.config \
    && cp /tmp/ipc-baseline.config /out/ipc-baseline.config \
    && cp /tmp/acpi.config.diff /out/acpi.config.diff \
    && cp /tmp/acpi-baseline.config /out/acpi-baseline.config \
    && cp /tmp/rng.config.diff /out/rng.config.diff \
    && cp COPYING /out/COPYING \
    && cp LICENSES/preferred/GPL-2.0 /out/GPL-2.0 \
    && printf '%s\n' 'Linux 6.1, source commit 830b3c68c1fb1e9176028d02ef86f3cf76aa2476; unmodified source, configuration enables SYSVIPC, SYSVIPC_SYSCTL, IPC_NS, ACPI, X86_PM_TIMER and HW_RANDOM_VIRTIO; HPET, SMP and HZ=100 are retained. The checksummed ACPI baseline changes only HW_RANDOM_VIRTIO from n to y. See ipc-baseline.config, acpi.config.diff, acpi-baseline.config and rng.config.diff for resolved changes. Rebuild with browser-single/Kernel.Dockerfile.' > /out/SOURCE.txt \
    && sha256sum /out/bzImage /out/linux.config /out/ipc-baseline.config /out/acpi.config.diff /out/acpi-baseline.config /out/rng.config.diff > /out/build.sha256
FROM scratch
COPY --from=build /out/ /
