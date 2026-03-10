#!/usr/bin/env sh
set -eu

mkdir -p /data/graph /data/lance /data/memory-pack

if [ -z "${AUTH_SECRET:-}" ]; then
  echo "warning: AUTH_SECRET is not set" >&2
fi

/usr/bin/supervisord -c /etc/supervisor/conf.d/memkit.conf &
supervisord_pid=$!

i=0
while [ ! -S /tmp/falkordb.sock ] && [ "$i" -lt 20 ]; do
  sleep 0.5
  i=$((i + 1))
done

if [ ! -S /tmp/falkordb.sock ]; then
  echo "timed out waiting for FalkorDB socket" >&2
  kill "$supervisord_pid" || true
  exit 1
fi

wait "$supervisord_pid"
