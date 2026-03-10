FROM falkordb/falkordb:latest

RUN apt-get update \
    && apt-get install -y --no-install-recommends supervisor curl \
    && rm -rf /var/lib/apt/lists/*

# Build artifact from this repository.
COPY target/release/mk /usr/local/bin/memkit-api
RUN chmod +x /usr/local/bin/memkit-api

COPY docker/supervisord.conf /etc/supervisor/conf.d/memkit.conf
COPY docker/scripts/start.sh /start.sh
RUN chmod +x /start.sh

EXPOSE 4242 6379
VOLUME ["/data"]

CMD ["/start.sh"]
