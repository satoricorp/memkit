#!/usr/bin/env sh
# Downloads a small GGUF model for query synthesis. Run before first query if you don't have a model.
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
MODELS_DIR="${MODELS_DIR:-${ROOT_DIR}/.local-runtime/models}"
MODEL_FILE="tinyllama-1.1b-chat-v1.0.Q4_K_M.gguf"
URL="https://huggingface.co/TheBloke/TinyLlama-1.1B-Chat-v1.0-GGUF/resolve/main/${MODEL_FILE}"

mkdir -p "$MODELS_DIR"
OUT="${MODELS_DIR}/${MODEL_FILE}"

if [ -f "$OUT" ]; then
  echo "Model already exists: $OUT"
  echo "Set MEMKIT_ONTOLOGY_MODEL=$OUT"
  exit 0
fi

echo "Downloading TinyLlama GGUF (~700MB) to $OUT"
curl -fL "$URL" -o "$OUT"
echo "Downloaded: $OUT"
echo ""
echo "Set MEMKIT_ONTOLOGY_MODEL=$OUT"
echo "Then restart the daemon: bun run local:stop && bun run local:start"
