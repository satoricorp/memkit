#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUN_DIR="${ROOT_DIR}/.local-run"
DATA_DIR="${ROOT_DIR}/.local-data"

SOCKET_PATH="${FALKORDB_SOCKET:-/tmp/falkordb.sock}"
GRAPH_PATH="${GRAPH_PATH:-${DATA_DIR}/graph}"
RUNTIME_CURRENT="${FALKOR_RUNTIME_ROOT:-${ROOT_DIR}/.local-runtime/falkor}/current"
REDIS_SERVER_BIN="${FALKOR_REDIS_SERVER:-${RUNTIME_CURRENT}/bin/redis-server}"
MODULE_PATH="${FALKOR_MODULE:-}"
LOG_PATH="${RUN_DIR}/falkordb.log"
PID_FILE="${RUN_DIR}/falkordb.pid"

if [ -z "$MODULE_PATH" ]; then
  if [ -f "${RUNTIME_CURRENT}/bin/falkordb.dylib" ]; then
    MODULE_PATH="${RUNTIME_CURRENT}/bin/falkordb.dylib"
  else
    MODULE_PATH="${RUNTIME_CURRENT}/bin/falkordb.so"
  fi
fi

mkdir -p "$RUN_DIR" "$GRAPH_PATH"

is_running() {
  [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null
}

wait_for_socket() {
  i=0
  while [ ! -S "$SOCKET_PATH" ] && [ "$i" -lt 20 ]; do
    sleep 0.5
    i=$((i + 1))
  done
}

start() {
  if is_running; then
    echo "falkordb already running"
    return 0
  fi
  rm -f "$PID_FILE" "$SOCKET_PATH"

  if [ ! -x "$REDIS_SERVER_BIN" ]; then
    echo "missing redis-server sidecar binary at ${REDIS_SERVER_BIN}" >&2
    exit 1
  fi
  if [ ! -f "$MODULE_PATH" ]; then
    echo "missing Falkor module at ${MODULE_PATH}" >&2
    exit 1
  fi

  nohup "$REDIS_SERVER_BIN" \
    --loadmodule "$MODULE_PATH" \
    --unixsocket "$SOCKET_PATH" \
    --unixsocketperm 777 \
    --save 60 1 \
    --dir "$GRAPH_PATH" \
    > "$LOG_PATH" 2>&1 &
  echo $! > "$PID_FILE"

  wait_for_socket
  if [ ! -S "$SOCKET_PATH" ]; then
    echo "timed out waiting for FalkorDB socket at $SOCKET_PATH" >&2
    echo "see log: ${LOG_PATH}" >&2
    if is_running; then
      kill "$(cat "$PID_FILE")" || true
    fi
    rm -f "$PID_FILE"
    exit 1
  fi

  echo "falkordb started (pid $(cat "$PID_FILE"))"
}

stop() {
  if is_running; then
    pid="$(cat "$PID_FILE")"
    kill "$pid" || true
    echo "stopped falkordb ($pid)"
  fi
  rm -f "$PID_FILE"
}

status() {
  if is_running; then
    echo "falkordb: running (pid $(cat "$PID_FILE"))"
  else
    echo "falkordb: stopped"
  fi
}

cmd="${1:-}"
case "$cmd" in
  start) start ;;
  stop) stop ;;
  status) status ;;
  *)
    echo "usage: $0 {start|stop|status}" >&2
    exit 1
    ;;
esac
