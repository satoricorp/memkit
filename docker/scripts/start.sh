#!/usr/bin/env sh
# Legacy entrypoint kept for compatibility; the Helix image runs `mk start` directly (see Dockerfile CMD).
set -eu
exec mk start --foreground
