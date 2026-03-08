#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUN_DIR="${ROOT_DIR}/.local-run"
DATA_DIR="${ROOT_DIR}/.local-data"

SOCKET_PATH="${FALKORDB_SOCKET:-/tmp/falkordb.sock}"
LANCEDB_PATH="${LANCEDB_PATH:-${DATA_DIR}/lance}"
GRAPH_PATH="${GRAPH_PATH:-${DATA_DIR}/graph}"
PACK_PATH="${PACK_PATH:-${ROOT_DIR}/memory-pack}"
API_HOST="${API_HOST:-127.0.0.1}"
API_PORT="${API_PORT:-4242}"
AUTH_SECRET="${AUTH_SECRET:-dev-local-secret}"

mkdir -p "$RUN_DIR" "$LANCEDB_PATH" "$GRAPH_PATH" "$PACK_PATH"

if [ ! -x "${ROOT_DIR}/target/release/satori" ]; then
  "${SCRIPT_DIR}/local-build.sh"
fi

"${SCRIPT_DIR}/falkor-ensure.sh"
FALKORDB_SOCKET="$SOCKET_PATH" GRAPH_PATH="$GRAPH_PATH" "${SCRIPT_DIR}/falkor-runtime.sh" start

if [ -f "${RUN_DIR}/satori-api.pid" ] && kill -0 "$(cat "${RUN_DIR}/satori-api.pid")" 2>/dev/null; then
  echo "satori api already running"
else
  nohup env \
    FALKORDB_SOCKET="$SOCKET_PATH" \
    LANCEDB_PATH="$LANCEDB_PATH" \
    API_PORT="$API_PORT" \
    AUTH_SECRET="$AUTH_SECRET" \
    "${ROOT_DIR}/target/release/satori" \
    --headless-serve \
    --pack "$PACK_PATH" \
    --host "$API_HOST" \
    --port "$API_PORT" \
    > "${RUN_DIR}/satori-api.log" 2>&1 &
  echo $! > "${RUN_DIR}/satori-api.pid"
fi

echo "started local stack"
echo "socket: $SOCKET_PATH"
echo "api: http://${API_HOST}:${API_PORT}"
