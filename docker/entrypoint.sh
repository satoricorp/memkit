#!/bin/sh
set -eu

if [ -n "${MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON:-}" ]; then
  creds_path="${GOOGLE_APPLICATION_CREDENTIALS:-/run/secrets/memkit/google-service-account.json}"
  creds_dir="$(dirname "$creds_path")"

  mkdir -p "$creds_dir"
  umask 077
  tmp_path="$(mktemp "${creds_dir}/google-service-account.XXXXXX.json")"
  printf '%s' "$MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON" > "$tmp_path"
  mv "$tmp_path" "$creds_path"
  chmod 600 "$creds_path"

  export GOOGLE_APPLICATION_CREDENTIALS="$creds_path"
  unset MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON
fi

exec "$@"
