# memkit CLI — Agent Context

This document provides guidance for AI agents invoking the memkit CLI (`mk`).

## Overview

memkit is a local memory pack CLI. The server must be running (`mk serve` or `./scripts/local-start.sh`) before most commands.

## Agent-Friendly Usage

### Use `--json` for parameterized commands

All commands that accept parameters support `--json` with a single JSON object. This avoids shell escaping and maps directly to the API.

```bash
mk add --json '{"path":"./specs","pack":"./memory-pack"}'
mk index --json '{"dir":"./memory-pack"}'
mk query --json '{"query":"how does auth work","mode":"hybrid","top_k":8}'
mk status --json '{"dir":"./memory-pack"}'
mk remove --json '{"dir":"./memory-pack"}'
mk graph --json '{"pack":"./memory-pack"}'
```

### Use `--output json` for machine-readable output

Always use `--output json` when parsing CLI output programmatically. This ensures raw JSON instead of human-formatted text.

```bash
mk status --output json
mk list --output json
mk query "x" --output json
```

### Use `--dry-run` before mutating operations

Mutating commands: `add`, `remove`, `index`. Use `--dry-run` to validate without side effects.

```bash
mk add --json '{"path":"./new-dir"}' --dry-run
mk index --json '{"dir":"./memory-pack"}' --dry-run
```

### Schema introspection

Use `mk schema <command>` to get the input schema for any command at runtime.

```bash
mk schema
mk schema query
mk schema add
```

## Input Hardening

The CLI validates all inputs. Rejected patterns:

- **Paths**: `..` traversal, `%` (pre-encoded), control characters
- **Resource IDs**: `?`, `#`, `%`, control characters
- **Query strings**: control characters

Assume inputs are validated; do not rely on the CLI to accept adversarial strings.

## Key Commands and JSON Shapes

| Command | Required | Optional |
|---------|----------|----------|
| add | `path` | `pack` |
| remove | — | `dir` |
| status | — | `dir` |
| index | `dir` | — |
| graph | — | `pack` |
| query | `query` | `mode`, `top_k`, `raw`, `pack` |

## Environment

- `OUTPUT_FORMAT=json` — Equivalent to `--output json` for all commands.
- `API_PORT` (default `4242`) — Server port.
- `API_HOST` (default `127.0.0.1`) — Server host.
