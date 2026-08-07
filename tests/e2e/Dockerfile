FROM rust:1.88-bookworm AS builder
WORKDIR /source
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/source/target,sharing=locked \
    e2e_toolchain="$(rustup default | cut -d' ' -f1)" \
    && RUSTUP_TOOLCHAIN="$e2e_toolchain" cargo build --locked --release --bins \
    && install -D -m 0755 target/release/folioharbor /artifacts/folioharbor \
    && install -D -m 0755 target/release/folioharbor-api /artifacts/folioharbor-api \
    && install -D -m 0755 target/release/folioharbor-worker /artifacts/folioharbor-worker

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates curl openssl util-linux wget \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /artifacts/folioharbor /usr/local/bin/folioharbor
COPY --from=builder /artifacts/folioharbor-api /usr/local/bin/folioharbor-api
COPY --from=builder /artifacts/folioharbor-worker /usr/local/bin/folioharbor-worker
USER 10001:10001
