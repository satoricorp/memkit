# CLI Specification V1.1 (Command-First)

> Legacy spec note: this file documents an older `satori` command set.
> Current implementation uses the `mk` binary and commands documented in `README.md`.
> Key deltas: no `init/index/watch` commands, default server port is `4242`, and storage is Helix-backed.

This document supersedes earlier CLI guidance that treated non-command interfaces as primary.

## Binary

- Command: `satori`
- Primary interface: command-only CLI
- Optional UIs are non-normative and must not replace command contracts
- Local runtime scripts (`scripts/local-start.sh`, etc.) start `mk serve` only; storage is Helix-backed (no separate graph DB sidecar in those scripts).

## Required Commands

### `satori init`

Creates a new memory pack scaffold in target directory.

Usage:

```bash
satori init --pack ./memory-pack
```

Flags:

- `--pack <path>` required
- `--provider <hash|fastembed>` optional (default `fastembed`)
- `--model <id>` optional (default `BAAI/bge-small-en-v1.5`)
- `--dim <n>` optional (default `384`)
- `--force` optional overwrite scaffold

### `satori index`

Enqueues indexing for configured sources and returns a job id.

Usage:

```bash
satori index
```

Flags: none (uses daemon-configured pack and sources).

### `satori sources add`

Adds a source path and enqueues background indexing.

Usage:

```bash
satori sources add ./project
satori sources add ./README.md
```

Rules:

- `<path>` may be a directory or a single file.
- Ingestion discovery is recursive through a unified traversal path.
- If `<path>` is a file, traversal yields that file only.

### `satori jobs list`

Lists recent background ingestion/index jobs.

### `satori jobs status <job-id>`

Returns detailed state for a single job.

### `satori serve`

Starts daemon + MCP adapter for the selected pack.

Usage:

```bash
satori serve --pack ./memory-pack --host 127.0.0.1 --port 7821
```

Flags:

- `--pack <path>` required
- `--host <host>` optional, default `127.0.0.1`
- `--port <port>` optional, default `7821`

### `satori watch start`

Starts background watch mode for incremental ingestion.

Usage:

```bash
satori watch start --pack ./memory-pack --debounce-ms 500
```

Flags:

- `--pack <path>` required
- `--debounce-ms <n>` optional

### `satori watch stop`

Stops the watch job for a pack.

Usage:

```bash
satori watch stop --pack ./memory-pack
```

Flags:

- `--pack <path>` required

### `satori query`

Runs local query against pack (debug and scripting).

Usage:

```bash
satori query "how auth works" --pack ./memory-pack --mode hybrid --top-k 8
```

Flags:

- positional query string required
- `--pack <path>` required
- `--mode <vector|hybrid>` optional, default `hybrid`
- `--top-k <n>` optional, default `8`
- `--json` optional machine-readable output

### `satori status`

Displays pack health, daemon state, and watch freshness.

Usage:

```bash
satori status --pack ./memory-pack --json
```

Flags:

- `--pack <path>` required
- `--json` optional machine-readable output

## Output Rules

- Human-readable default with concise sections.
- `--json` must provide stable schema for scripting.
- Non-zero exit codes on failure.

## Exit Codes

- `0` success
- `1` generic runtime failure
- `2` invalid arguments
- `3` pack/config validation failure
- `4` indexing failure
- `5` query failure
- `6` watch lifecycle failure
- `7` background job failure

## Error Message Rules

- Include:
  - short message
  - probable cause
  - one actionable hint

Example:

`PACK_INVALID: manifest.json missing format_version. Hint: run 'satori init --force --pack <path>' to regenerate scaffold.`
