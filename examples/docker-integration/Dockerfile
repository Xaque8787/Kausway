# syntax=docker/dockerfile:1.7
ARG KAUSWAY_IMAGE=ghcr.io/xaque8787/kausway:latest
FROM ${KAUSWAY_IMAGE} AS kausway

FROM python:3.12-alpine
RUN addgroup -g 10001 application && adduser -D -u 10001 -G application application \
    && mkdir /config /data && chown 10001:10001 /config /data
COPY --from=kausway /usr/local/bin/kausway /usr/local/bin/kausway-host /usr/local/bin/kausway-admin /usr/local/bin/
COPY --from=kausway /usr/share/kausway/ /usr/share/kausway/
# Build with examples/docker-integration as the context.
COPY entrypoint.sh supervise.py /opt/kausway-example/
RUN chmod 0755 /opt/kausway-example /opt/kausway-example/entrypoint.sh \
    && chmod 0644 /opt/kausway-example/supervise.py
ENV KAUSWAY_CONFIG=/config/config.toml KAUSWAY_DATA_ROOT=/data \
    KAUSWAY_ADMIN_LISTEN=0.0.0.0:47951 KAUSWAY_MODE=manager KAUSWAY_WITH_ADMIN=true
USER 10001:10001
ENTRYPOINT ["/opt/kausway-example/entrypoint.sh"]
# Replace this demonstration origin with the developer's real application.
CMD ["python3", "-m", "http.server", "8096", "--bind", "127.0.0.1", "--directory", "/tmp"]
