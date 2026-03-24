#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
RUN_DIR="${ROOT_DIR}/.local-run"
DATA_DIR="${ROOT_DIR}/.local-data"

PACK_PATH="${PACK_PATH:-${ROOT_DIR}/memory-pack}"
PACK_PATHS="${PACK_PATHS:-$PACK_PATH}"
API_HOST="${API_HOST:-127.0.0.1}"
API_PORT="${API_PORT:-4242}"
AUTH_SECRET="${AUTH_SECRET:-dev-local-secret}"

mkdir -p "$RUN_DIR" "$DATA_DIR"
for p in $(echo "$PACK_PATHS" | tr ',' ' '); do mkdir -p "$p"; done

if [ ! -x "${ROOT_DIR}/target/release/mk" ]; then
  "${SCRIPT_DIR}/local-build.sh"
fi

if [ -f "${RUN_DIR}/memkit-api.pid" ] && kill -0 "$(cat "${RUN_DIR}/memkit-api.pid")" 2>/dev/null; then
  echo "memkit server already running"
else
  nohup env \
    MEMKIT_PACK_PATHS="$PACK_PATHS" \
    API_PORT="$API_PORT" \
    AUTH_SECRET="$AUTH_SECRET" \
    ${MEMKIT_LLM_MODEL:+MEMKIT_LLM_MODEL="$MEMKIT_LLM_MODEL"} \
    ${MEMKIT_ONTOLOGY_MODEL:+MEMKIT_ONTOLOGY_MODEL="$MEMKIT_ONTOLOGY_MODEL"} \
    ${OPENAI_API_KEY:+OPENAI_API_KEY="$OPENAI_API_KEY"} \
    "${ROOT_DIR}/target/release/mk" \
    start \
    --pack "$PACK_PATHS" \
    --host "$API_HOST" \
    --port "$API_PORT" \
    > "${RUN_DIR}/memkit-api.log" 2>&1 &
  echo $! > "${RUN_DIR}/memkit-api.pid"
fi

echo "started local memkit daemon"
echo "api: http://${API_HOST}:${API_PORT}"
