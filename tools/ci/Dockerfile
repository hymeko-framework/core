# Build on the Docker host, with an SSH mount and trusted known_hosts secret.
# See docs/core-container.md for the bounded Amaterasu build/run commands.
ARG RUST_IMAGE=rust:1.97.1-bookworm@sha256:408fe88047cef61a2087653b0c5255fa51c0f2d6d94ddedd7a2562a9b91a46f6
FROM ${RUST_IMAGE} AS toolchain

RUN rustup component add rustfmt clippy \
    && apt-get update \
    && apt-get install --no-install-recommends -y time libvulkan1 libegl1 \
    && rm -rf /var/lib/apt/lists/*

ENV CARGO_BUILD_JOBS=6 \
    RAYON_NUM_THREADS=6 \
    CARGO_TERM_COLOR=never \
    CARGO_NET_GIT_FETCH_WITH_CLI=true

WORKDIR /workspace/core

# Native compilation for the ARM64 verification runtime. QEMU is invoked
# explicitly inside the runtime container; no host binfmt registration is needed.
FROM toolchain AS cross-toolchain
RUN apt-get update \
    && apt-get install --no-install-recommends -y gcc-aarch64-linux-gnu libc6-dev-arm64-cross qemu-user-static \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add aarch64-unknown-linux-gnu
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CARGO_BUILD_JOBS=4 \
    RAYON_NUM_THREADS=4

FROM toolchain AS dependencies
COPY Cargo.toml Cargo.lock ./
COPY hymeko_core ./hymeko_core
COPY hymeko_wgpu ./hymeko_wgpu
# Authentication is available for this instruction only. Cargo.lock retains
# the canonical SSH URL and exact parser commit. No host Cargo config is copied.
RUN --mount=type=ssh,required=true \
    --mount=type=secret,id=known_hosts,target=/etc/ssh/ssh_known_hosts,required=true \
    GIT_SSH_COMMAND='ssh -o BatchMode=yes -o StrictHostKeyChecking=yes' \
    cargo fetch --locked --target x86_64-unknown-linux-gnu

FROM dependencies AS verify
COPY data ./data
COPY README.md LICENSE ./
COPY tools/ci/core.sh /usr/local/bin/hymeko-core-check
RUN sed -i 's/\r$//' /usr/local/bin/hymeko-core-check \
    && chmod +x /usr/local/bin/hymeko-core-check
ARG SOURCE_REVISION=unspecified
LABEL org.opencontainers.image.title="HyMeKo Core SDK and verification" \
    org.opencontainers.image.revision="${SOURCE_REVISION}"
ENV CARGO_NET_OFFLINE=true
ENTRYPOINT ["/usr/local/bin/hymeko-core-check"]
CMD ["all"]
