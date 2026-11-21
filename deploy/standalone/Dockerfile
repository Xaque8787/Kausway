# syntax=docker/dockerfile:1.7
# Native per-platform build. Buildx selects the target platform for both stages.
FROM rust:1.99.1-slim-bookworm@sha256:24707e70a4f1e23d9a7b25202ec43126d28a52294b96aad5c670df1fd1f1d400 AS build
ARG TARGETARCH
ARG VCS_REF=unknown
ARG CARGO_BUILD_JOBS=4
ENV RUSTUP_TOOLCHAIN=1.99.1 CARGO_TERM_COLOR=never CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools binutils python3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY rust/ rust/
RUN --mount=type=secret,id=proxy_ca,target=/etc/ssl/certs/ca-certificates.crt \
    case "$TARGETARCH" in amd64) target=x86_64-unknown-linux-musl ;; arm64) target=aarch64-unknown-linux-musl ;; *) exit 1 ;; esac \
    && rustup target add "$target"
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/rust/target \
    --mount=type=secret,id=proxy_ca,target=/etc/ssl/certs/ca-certificates.crt \
    case "$TARGETARCH" in amd64) target=x86_64-unknown-linux-musl ;; arm64) target=aarch64-unknown-linux-musl ;; *) exit 1 ;; esac \
    && CC=musl-gcc cargo build --manifest-path rust/Cargo.toml --release --locked --target "$target" \
       -p kausway-manager -p kausway-host -p kausway-admin --bins \
    && mkdir /binaries \
    && for binary in kausway kausway-host kausway-admin kausway-test-client; do cp "rust/target/$target/release/$binary" /binaries/; done \
    && cargo metadata --manifest-path rust/Cargo.toml --locked --format-version 1 --filter-platform "$target" > /metadata.json
COPY scripts/packaging/ scripts/packaging/
COPY scripts/ci/provenance.py scripts/ci/provenance.py
COPY scripts/releases/ scripts/releases/
COPY releases.toml ./
COPY LICENSE-MIT LICENSE-APACHE ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    case "$TARGETARCH" in amd64) target=x86_64-unknown-linux-musl ;; arm64) target=aarch64-unknown-linux-musl ;; *) exit 1 ;; esac \
    && python3 scripts/packaging/stage-container.py --binary-dir /binaries --metadata /metadata.json \
       --target "$target" --revision "$VCS_REF" --output /out

FROM alpine:3.22@sha256:49340b346798875b5592e5d420241f2f9cfba5f47e62ebb1446a1b34734e898f AS runtime
ARG VCS_REF=unknown
LABEL org.opencontainers.image.source="https://github.com/Xaque8787/Kausway" \
      org.opencontainers.image.revision="$VCS_REF" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
RUN addgroup -g 10001 kausway && adduser -D -u 10001 -G kausway kausway \
    && mkdir /config /data && chown 10001:10001 /config /data
COPY --from=build /out/bin/ /usr/local/bin/
COPY --from=build /out/share/ /usr/share/
ENV KAUSWAY_CONFIG=/config/kausway.toml KAUSWAY_DATA_ROOT=/data \
    KAUSWAY_ADMIN_LISTEN=0.0.0.0:47951
USER 10001:10001
EXPOSE 47951/tcp
VOLUME ["/config", "/data"]
CMD ["kausway", "run", "--admin-ui"]
