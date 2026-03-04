# CLI Specification V1

## Binary

- Command: `satori`
- Config root default: `~/.satori/`

## Commands

## `satori init`

Creates a new memory pack scaffold in target directory.

Usage:

```bash
satori init --pack ./memory-pack
```

Flags:

- `--pack <path>` required
- `--model <id>` optional (default `BAAI/bge-small-en-v1.5`)
- `--force` optional overwrite scaffold

## `satori index`

Indexes source files into pack.

Usage:

```bash
satori index --pack ./memory-pack --source ./project
```

Flags:

- `--pack <path>` required
- `--source <path>` repeatable
- `--include <glob>` repeatable
- `--exclude <glob>` repeatable
- `--watch` optional enable watch mode after initial index
- `--max-workers <n>` optional

## `satori serve`

Starts daemon and MCP adapter.

Usage:

```bash
satori serve --pack ./memory-pack --port 7821
```

Flags:

- `--pack <path>` required
- `--host <host>` default `127.0.0.1`
- `--port <port>` default `7821`
- `--no-watch` optional

## `satori query`

Runs local query against pack (debug and scripting).

Usage:

```bash
satori query "how auth works" --pack ./memory-pack --mode hybrid --top-k 8
```

Flags:

- positional query string required
- `--pack <path>` required
- `--mode <vector|hybrid>` default `hybrid`
- `--top-k <n>` default `8`
- `--json` output machine-readable JSON

## `satori status`

Displays pack health, index freshness, and watcher status.

Usage:

```bash
satori status --pack ./memory-pack
```

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

## Error Message Rules

- Include:
  - short message
  - probable cause
  - one actionable hint

Example:

`PACK_INVALID: manifest.json missing format_version. Hint: run 'satori init --force --pack <path>' to regenerate scaffold.`
