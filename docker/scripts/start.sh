#!/usr/bin/env sh
# Legacy entrypoint kept for compatibility; the Helix image runs `mk serve` directly (see Dockerfile CMD).
set -eu
exec mk serve --foreground
