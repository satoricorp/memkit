# Helix-only image: build `mk` on the host, then copy the release binary.
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

COPY target/release/mk /usr/local/bin/mk
RUN chmod +x /usr/local/bin/mk
COPY docker/entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

ENV API_HOST=0.0.0.0
ENV API_PORT=4242

EXPOSE 4242

ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["mk", "start", "--foreground"]
