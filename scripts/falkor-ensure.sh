#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

ARTIFACTS_FILE="${FALKOR_ARTIFACTS_FILE:-${SCRIPT_DIR}/falkor-artifacts.json}"
RUNTIME_ROOT="${FALKOR_RUNTIME_ROOT:-${ROOT_DIR}/.local-runtime/falkor}"
TMP_DIR="${ROOT_DIR}/.local-runtime/tmp"

platform_key() {
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  case "${os}-${arch}" in
    darwin-arm64|darwin-aarch64) echo "darwin-arm64" ;;
    linux-x86_64|linux-amd64) echo "linux-x86_64" ;;
    *) echo "unsupported" ;;
  esac
}

PLATFORM="$(platform_key)"
if [ "$PLATFORM" = "unsupported" ]; then
  echo "unsupported platform: $(uname -s)-$(uname -m). supported: darwin-arm64, linux-x86_64" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required to provision the Falkor sidecar" >&2
  exit 1
fi

read_manifest() {
  python3 - "$ARTIFACTS_FILE" "$PLATFORM" <<'PY'
import json, sys
path, platform = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
entry = data.get("platforms", {}).get(platform)
if not entry:
    print("", end="")
    sys.exit(0)
vals = [
    data["version"],
    entry["url"],
    entry["sha256"],
    entry["redis_member"],
    entry["module_member"],
    entry["module_name"],
]
for v in vals:
    print(v)
PY
}

MANIFEST="$(read_manifest)"
if [ -z "$MANIFEST" ]; then
  echo "no artifact mapping for platform '${PLATFORM}' in ${ARTIFACTS_FILE}" >&2
  exit 1
fi

VERSION="$(printf '%s\n' "$MANIFEST" | sed -n '1p')"
URL="$(printf '%s\n' "$MANIFEST" | sed -n '2p')"
SHA256_EXPECTED="$(printf '%s\n' "$MANIFEST" | sed -n '3p')"
REDIS_MEMBER="$(printf '%s\n' "$MANIFEST" | sed -n '4p')"
MODULE_MEMBER="$(printf '%s\n' "$MANIFEST" | sed -n '5p')"
MODULE_NAME="$(printf '%s\n' "$MANIFEST" | sed -n '6p')"

INSTALL_DIR="${RUNTIME_ROOT}/${VERSION}/${PLATFORM}"
BIN_DIR="${INSTALL_DIR}/bin"
REDIS_BIN="${BIN_DIR}/redis-server"
MODULE_BIN="${BIN_DIR}/${MODULE_NAME}"

mkdir -p "$BIN_DIR" "$TMP_DIR" "${RUNTIME_ROOT}/${VERSION}"

if [ -x "$REDIS_BIN" ] && [ -f "$MODULE_BIN" ]; then
  ln -sfn "$INSTALL_DIR" "${RUNTIME_ROOT}/current"
  echo "falkor sidecar ready: ${INSTALL_DIR}"
  exit 0
fi

ARCHIVE_PATH="${TMP_DIR}/${VERSION}-${PLATFORM}.whl"
echo "downloading falkor sidecar artifact for ${PLATFORM}"
curl -fsSL "$URL" -o "$ARCHIVE_PATH"

SHA256_ACTUAL="$(shasum -a 256 "$ARCHIVE_PATH" | awk '{print $1}')"
if [ "$SHA256_ACTUAL" != "$SHA256_EXPECTED" ]; then
  echo "artifact checksum mismatch for ${ARCHIVE_PATH}" >&2
  echo "expected: ${SHA256_EXPECTED}" >&2
  echo "actual:   ${SHA256_ACTUAL}" >&2
  rm -f "$ARCHIVE_PATH"
  exit 1
fi

python3 - "$ARCHIVE_PATH" "$REDIS_MEMBER" "$MODULE_MEMBER" "$REDIS_BIN" "$MODULE_BIN" <<'PY'
import os, sys, zipfile
archive, redis_member, module_member, redis_out, module_out = sys.argv[1:6]
os.makedirs(os.path.dirname(redis_out), exist_ok=True)
with zipfile.ZipFile(archive, "r") as zf:
    with zf.open(redis_member) as src, open(redis_out, "wb") as dst:
        dst.write(src.read())
    with zf.open(module_member) as src, open(module_out, "wb") as dst:
        dst.write(src.read())
PY

chmod +x "$REDIS_BIN"
chmod +x "$MODULE_BIN"
rm -f "$ARCHIVE_PATH"

ln -sfn "$INSTALL_DIR" "${RUNTIME_ROOT}/current"
echo "falkor sidecar installed: ${INSTALL_DIR}"
