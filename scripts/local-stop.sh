#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUN_DIR="${ROOT_DIR}/.local-run"

stop_pid_file() {
  pid_file="$1"
  label="$2"

  if [ -f "$pid_file" ]; then
    pid="$(cat "$pid_file")"
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" || true
      echo "stopped $label ($pid)"
    fi
    rm -f "$pid_file"
  fi
}

stop_pid_file "${RUN_DIR}/satori-api.pid" "satori-api"
if [ -x "${SCRIPT_DIR}/falkor-runtime.sh" ]; then
  "${SCRIPT_DIR}/falkor-runtime.sh" stop
else
  stop_pid_file "${RUN_DIR}/falkordb.pid" "falkordb"
fi
