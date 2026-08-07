FROM node:24-bookworm-slim AS web-builder
WORKDIR /source
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY web/package.json web/package.json
RUN --mount=type=cache,id=folioharbor-corepack,target=/root/.cache/node/corepack,sharing=locked \
    corepack enable \
    && corepack_attempt=1 \
    && until corepack install --global pnpm@11.9.0; do \
      if [ "$corepack_attempt" -ge 5 ]; then exit 1; fi; \
      corepack_attempt=$((corepack_attempt + 1)); \
    done
RUN --mount=type=cache,id=folioharbor-corepack,target=/root/.cache/node/corepack,sharing=locked \
    --mount=type=cache,id=folioharbor-pnpm,target=/pnpm/store,sharing=locked \
    pnpm config set store-dir /pnpm/store \
    && pnpm config set fetch-timeout 60000 \
    && pnpm config set fetch-retries 5 \
    && pnpm install --frozen-lockfile --prod --trust-lockfile
COPY web web
RUN --mount=type=cache,id=folioharbor-corepack,target=/root/.cache/node/corepack,sharing=locked \
    pnpm --dir web run build

FROM rust:1.88-bookworm AS rust-builder
WORKDIR /source
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/source/target,sharing=locked \
    build_toolchain="$(rustup default | cut -d' ' -f1)" \
    && RUSTUP_TOOLCHAIN="$build_toolchain" cargo build --locked --release --bins \
    && install -D -m 0755 target/release/folioharbor /artifacts/folioharbor \
    && install -D -m 0755 target/release/folioharbor-api /artifacts/folioharbor-api \
    && install -D -m 0755 target/release/folioharbor-worker /artifacts/folioharbor-worker

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates curl nginx openssl util-linux wget \
    && rm -rf /var/lib/apt/lists/* \
    && install -d -m 0755 /etc/folioharbor /usr/share/folioharbor/web
COPY --from=rust-builder /artifacts/folioharbor /usr/local/bin/folioharbor
COPY --from=rust-builder /artifacts/folioharbor-api /usr/local/bin/folioharbor-api
COPY --from=rust-builder /artifacts/folioharbor-worker /usr/local/bin/folioharbor-worker
COPY --from=web-builder /source/web/dist/ /usr/share/folioharbor/web/
COPY deploy/nginx.conf /etc/folioharbor/nginx.conf
USER 10001:10001
