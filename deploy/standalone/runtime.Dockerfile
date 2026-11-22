# syntax=docker/dockerfile:1.7
# CI stages verified native musl binaries/notices first; no unverified downloads.
# Keep the runtime contract identical to deploy/standalone/Dockerfile.
FROM alpine:3.22@sha256:49340b346798875b5592e5d420241f2f9cfba5f47e62ebb1446a1b34734e898f
ARG VCS_REF=unknown
ARG CONTAINER_VERSION=unknown
ARG SUITE_VERSION=unknown
ARG SUITE_SOURCE_COMMIT=unknown
ARG SUITE_MANIFEST_SHA256=unknown
LABEL org.opencontainers.image.source="https://github.com/Xaque8787/Kausway" \
      org.opencontainers.image.revision="$VCS_REF" \
      org.opencontainers.image.version="$CONTAINER_VERSION" \
      io.kausway.suite.version="$SUITE_VERSION" \
      io.kausway.suite.commit="$SUITE_SOURCE_COMMIT" \
      io.kausway.suite.manifest="$SUITE_MANIFEST_SHA256" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
RUN addgroup -g 10001 kausway && adduser -D -u 10001 -G kausway kausway \
    && mkdir /config /data && chown 10001:10001 /config /data
COPY bin/ /usr/local/bin/
COPY share/ /usr/share/
ENV KAUSWAY_CONFIG=/config/kausway.toml KAUSWAY_DATA_ROOT=/data \
    KAUSWAY_ADMIN_LISTEN=0.0.0.0:47951
USER 10001:10001
EXPOSE 47951/tcp
VOLUME ["/config", "/data"]
CMD ["kausway", "run", "--admin-ui"]
