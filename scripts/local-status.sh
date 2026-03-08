#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUN_DIR="${ROOT_DIR}/.local-run"
API_HOST="${API_HOST:-127.0.0.1}"
API_PORT="${API_PORT:-4242}"

check_pid_file() {
  pid_file="$1"
  label="$2"

  if [ -f "$pid_file" ] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
    echo "$label: running (pid $(cat "$pid_file"))"
  else
    echo "$label: stopped"
  fi
}

if [ -x "${SCRIPT_DIR}/falkor-runtime.sh" ]; then
  "${SCRIPT_DIR}/falkor-runtime.sh" status
else
  check_pid_file "${RUN_DIR}/falkordb.pid" "falkordb"
fi
check_pid_file "${RUN_DIR}/satori-api.pid" "satori-api"

if command -v curl >/dev/null 2>&1; then
  echo "health:"
  curl -s "http://${API_HOST}:${API_PORT}/health" || true
  echo
fi
