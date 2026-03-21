# memkit CLI — Agent Context

This document provides guidance for AI agents invoking the memkit CLI (`mk`).

## Overview

memkit is a local memory pack CLI. The server must be running (`mk serve` or `./scripts/local-start.sh`) before most commands.

## Agent JSON (single entry point)

Use **one** JSON object with a **`command`** field. Pass it after **`--json`** or **`-j`**. Do **not** use `mk <cmd> --json` — that form was removed.

```bash
mk -j '{"command":"add","path":"./specs","pack":"./memory-pack"}'
mk -j '{"command":"query","query":"how does auth work","top_k":8}'
mk -j '{"command":"status","dir":"./memory-pack"}'
mk -j '{"command":"use","pack":null}'
mk -j '{"command":"use","model":"openai:gpt-5.2"}'
mk -j '{"command":"list"}'
mk -j '{"command":"doctor"}'
```

- **`list`**: packs registered packs plus current/supported model IDs (no input fields).
- **`use`**: omit both `pack` and `model` to show defaults for both; `null` on one key shows only that field; strings set pack or model. Shell argv only supports `mk use pack <name>` and `mk use model <id>` (set).
- **`status`**: omit `dir` to list all registered packs (pack-only; use **`list`** for packs + models).

### Use `--output json` for machine-readable output

```bash
mk status --output json
mk query "x" --output json
mk doctor --output json
```

### Use `--dry-run` before mutating operations

Mutating commands: `add`, `remove`.

```bash
mk -j '{"command":"add","path":"./new-dir"}' --dry-run
mk remove ./memory-pack --dry-run --yes
```

### Schema introspection

```bash
mk schema
mk schema query
mk schema --format json-schema query
mk schema use
mk schema doctor
```

Machine-readable JSON Schema for agent inputs: `mk schema --format json-schema <command>`. See [docs/llm-configuration.md](docs/llm-configuration.md) for model precedence.

## Input Hardening

The CLI validates all inputs. Rejected patterns:

- **Paths**: `..` traversal, `%` (pre-encoded), control characters
- **Resource IDs**: `?`, `#`, `%`, control characters
- **Query strings**: control characters

Assume inputs are validated; do not rely on the CLI to accept adversarial strings.

## Key commands (JSON `command` values)

| command   | Required    | Optional |
|-----------|-------------|----------|
| add       | `path` or `documents` / `conversation` | `pack` |
| remove    | —           | `dir`, `confirm` |
| status    | —           | `dir` (omit = all packs) |
| query     | `query`     | `top_k`, `use_reranker`, `raw`, `pack` |
| publish   | —           | `pack` / `path`, `destination` |
| use       | —           | `pack`, `model` (see above) |
| list      | —           | — |
| doctor    | —           | — |

## Environment

- `OUTPUT_FORMAT=json` — Equivalent to `--output json` for all commands.
- `API_PORT` (default `4242`) — Server port.
- `API_HOST` (default `127.0.0.1`) — Server host.

## Local config

- Config file path: `~/.config/memkit/memkit.json` (or `$XDG_CONFIG_HOME/memkit/memkit.json`)
- Supported field: `model` (optional default model id; use `mk use model <id>`)

Precedence for **OpenAI query synthesis** is documented in [docs/llm-configuration.md](docs/llm-configuration.md) (`MEMKIT_OPENAI_MODEL` → `memkit.json` `openai:*` → default).
