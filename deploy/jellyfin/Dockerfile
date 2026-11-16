# syntax=docker/dockerfile:1.7
# Jellyfin with the kausway-host transport and the admin plugin, built from this repository's sources.
# Pinned versions and the reasons for them are in docs/operations/versions.md.

FROM rust:1.99.1-slim-bookworm@sha256:24707e70a4f1e23d9a7b25202ec43126d28a52294b96aad5c670df1fd1f1d400 AS rust-build
# Overrides rust/rust-toolchain.toml so the image's compiler is used without fetching clippy/rustfmt.
ENV RUSTUP_TOOLCHAIN=1.99.1 CARGO_TERM_COLOR=never
WORKDIR /src/rust
COPY rust/ ./
# Optional combined CA bundle for builds behind a TLS-inspecting proxy; not stored in layers.
RUN --mount=type=secret,id=proxy_ca,target=/etc/ssl/certs/ca-certificates.crt \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/rust/target \
    cargo build --release --locked -p kausway-host --bin kausway-host \
    && install -D -m 0755 target/release/kausway-host /out/kausway-host

FROM mcr.microsoft.com/dotnet/sdk:10.0.403@sha256:de4fe90a98845d75910b9769662465c84de1a934665c431279d1382b2bddc02b AS plugin-build
ENV DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1
WORKDIR /src/integrations/jellyfin
COPY integrations/jellyfin/ ./
COPY scripts/packaging/package-plugin.sh /usr/local/bin/package-plugin
# Fail the image build if host, plugin metadata and assembly versions have drifted apart.
COPY rust/Cargo.toml rust/Cargo.lock /src/rust/
COPY scripts/ci/check-version.sh /src/scripts/ci/check-version.sh
RUN bash /src/scripts/ci/check-version.sh
RUN --mount=type=secret,id=proxy_ca,target=/etc/ssl/certs/ca-certificates.crt \
    --mount=type=cache,target=/root/.nuget/packages \
    dotnet publish plugin/Jellyfin.Plugin.Kausway.csproj -c Release -o /build \
    && sh /usr/local/bin/package-plugin /build meta.json /out

FROM jellyfin/jellyfin:12.4.20261026-211555@sha256:fea863d0cd9ec445985d9c75fc6ed64e38a1d2766cf0c250a33d553f4fc1b771
COPY --from=rust-build /out/kausway-host /usr/local/bin/kausway-host
COPY --from=plugin-build /out/ /opt/kausway/plugins/
COPY deploy/jellyfin/entrypoint.sh /usr/local/bin/kausway-entrypoint
RUN chmod 0755 /usr/local/bin/kausway-entrypoint
ENV KAUSWAY_DATA_DIR=/kausway-data \
    KAUSWAY_ORIGIN=http://127.0.0.1:8096 \
    KAUSWAY_CONTROL_LISTEN=127.0.0.1:47950 \
    JELLYFIN_KAUSWAY_CONTROL_URL=http://127.0.0.1:47950 \
    JELLYFIN_KAUSWAY_CONTROL_TOKEN_FILE=/kausway-data/control.token
VOLUME ["/kausway-data"]
ENTRYPOINT ["/usr/local/bin/kausway-entrypoint"]
