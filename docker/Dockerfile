# ECS image: build `memkit-server` (and optionally `mk`) on the host, then copy the release binaries.
# Run with a mount for Helix data and pack paths, e.g.:
#   docker build -f docker/Dockerfile -t memkit .
#   docker run --rm -p 4242:4242 -v "$HOME/memory-pack:/data/pack" \
#     -e MEMKIT_HELIX_ROOT=/data/helix -e MEMKIT_PACK_PATH=/data/pack \
#     -e OPENAI_API_KEY=... memkit
#
# For deployed environments, you can pass MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON
# as a secret and the entrypoint will materialize it into
# GOOGLE_APPLICATION_CREDENTIALS before starting the server.

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY target/release/memkit-server /usr/local/bin/memkit-server
COPY target/release/mk /usr/local/bin/mk
RUN chmod +x /usr/local/bin/memkit-server /usr/local/bin/mk
COPY docker/entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

ENV MEMKIT_HOST=0.0.0.0
ENV MEMKIT_PORT=8080
ENV PORT=8080
ENV MEMKIT_CLOUD_ROOT=/mnt/memkit-cloud

EXPOSE 8080

ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["memkit-server"]
