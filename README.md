# memkit

Local memory pack CLI + server (Rust).

## Build

```bash
cargo build --release
```

The CLI binary is `mk` (at `target/release/mk`).

## Quick start

```bash
# Build
cargo build --release

# Start server in background (builds release binary if missing)
./scripts/local-start.sh

# Or run server directly (single or multiple packs, comma-delimited)
mk serve --pack ./memory-pack
mk serve --pack ./pack1,./pack2

# CLI commands (require server to be running)
mk list
mk status
mk status ~/memory
mk add ~/Documents/project-notes
mk query "local memory pack"
mk doctor
```

## Commands

- `mk add <path-or-url> [--pack <name-or-path>]` — Add local files or URL/docs to a pack.
- `mk remove [dir]` — Remove a pack (prompts unless `--yes`).
- `mk list` — Registered packs (same listing as `mk status` with no `dir`) plus current and supported model IDs.
- `mk status [dir]` — Without `dir`: list all registered packs. With `dir`: status for that pack.
- `mk query "<text>" [--pack <name-or-path>] [--top-k N] [--no-rerank] [--raw]` — Query a pack.
- `mk publish [--pack <name-or-path>] [--destination s3://bucket/prefix]` — Publish pack artifacts.
- `mk use pack <name-or-path>` — Set default pack.
- `mk use model <model-id>` — Set default model (see `mk list` for IDs).
- `mk doctor` — Config path and whether the API is reachable (`GET /health`).
- `mk serve [--pack <path>] [--host] [--port] [--foreground]` — Start server (background by default).
- `mk stop [--port]` — Stop background server on the configured port.
- `mk schema [--format json|json-schema] [command]` — Introspect memkit or JSON Schema for agent inputs.

**Agents:** use a single JSON object with `mk -j '{...}'` or `mk --json` (see [CONTEXT.md](CONTEXT.md)). Global flags: `--output json`, `--dry-run`, `--version` / `-V`. Set `NO_COLOR=1` to disable ANSI colors in terminal output.

## Storage backend

The current build uses Helix as the local vector/graph store.

```bash
cargo build --release
```

- `MEMKIT_HELIX_ROOT` — Base directory for Helix pack DBs (default `~/.memkit/helix`).

## Configuration file

`mk` stores user preferences in:

- `~/.config/memkit/memkit.json` (or `$XDG_CONFIG_HOME/memkit/memkit.json`)

Fields:

- `model` (optional) — Default model ID for `mk use model <id>` (namespaced, e.g. `openai:gpt-5.4`). When it starts with `openai:`, the server may use it for query synthesis (see precedence below).

**Query synthesis (OpenAI)** — order of precedence for which model the API calls:

1. `MEMKIT_OPENAI_MODEL` (raw OpenAI model id, e.g. `gpt-5.4`)
2. `memkit.json` `model` if it is an `openai:*` id
3. Built-in default `gpt-5.4`

Full detail: [docs/llm-configuration.md](docs/llm-configuration.md).

## Environment

- `API_HOST` / `API_PORT` (defaults `127.0.0.1` / `4242`)
- `MEMKIT_PACK_PATH` (default `./memory-pack` when using serve)
- `MEMKIT_PACK_PATHS` — Comma-delimited pack paths for multi-pack mode (overrides `MEMKIT_PACK_PATH` when set)
- `MEMKIT_HELIX_ROOT` — Helix pack DB base directory (default `~/.memkit/helix`)
- `OPENAI_API_KEY` — Required for query synthesis (OpenAI path; no local GGUF fallback in default builds)
- `MEMKIT_OPENAI_MODEL` — Optional override for OpenAI chat model (see precedence above)
- `MEMKIT_LLM_PROVIDER` — Ontology / extraction backend: `rules` (default), or `llama` when built with `--features llama-embedded`
- `MEMKIT_LLM_MODEL` — Optional GGUF path for local embed / llama feature builds (not used for OpenAI synthesis)
- `MEMKIT_LLM_MAX_TOKENS` (default `512`)
- `MEMKIT_LLM_TIMEOUT_MS` (default `20000`)
- `GOOGLE_APPLICATION_CREDENTIALS` — Path to service account JSON key (optional, for Google Docs/Sheets)
- `MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON` — Inline service account JSON (optional; overrides path)

`MEMKIT_ONTOLOGY_*` env vars are deprecated aliases; use `MEMKIT_LLM_*` where applicable.

**Google Docs and Sheets (optional):** To index Google Docs or Sheets, configure a service account and share each doc/sheet with the service account email (no user OAuth). Set one of:

- `GOOGLE_APPLICATION_CREDENTIALS` — Path to a JSON key file for the service account (e.g. from GCP Console → IAM → Service accounts → Keys).
- `MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON` — Inline JSON string of the same key.

The service account email is fixed (e.g. `name@project-id.iam.gserviceaccount.com`). You can get it from the JSON key (`client_email`) or from the API: `GET /google/service-account-email` when configured. Share your Doc or Sheet with that email (Viewer or Editor), then add via `POST /add` with `documents: [{ "type": "google_doc", "value": "<URL or doc ID>" }]` or `"type": "google_sheet"` with a Sheet URL or spreadsheet ID.

## Docker

Helix-only image: build the binary on the host, then copy it into a small Debian image.

```bash
cargo build --release
docker build -f docker/Dockerfile -t memkit .
docker run --rm -p 4242:4242 \
  -v "$PWD/memory-pack:/data/pack" \
  -e MEMKIT_HELIX_ROOT=/data/helix \
  -e MEMKIT_PACK_PATH=/data/pack \
  -e OPENAI_API_KEY="$OPENAI_API_KEY" \
  memkit
```

For local iteration, `./scripts/local-start.sh` is usually simpler than Docker.

## API

- `GET /health` — Health check
- `GET /status` — Pack status
- `GET /google/service-account-email` — Service account email for sharing (when Google is configured)
- `POST /query` — Query with synthesis
- `POST /index` — Trigger indexing
- `POST /add` — Add documents. Body: `documents: [{ "type": "url"|"content"|"google_doc"|"google_sheet", "value": "..." }]`, or `conversation: [{ "role", "content" }]`. For `google_doc` / `google_sheet`, share the doc/sheet with the service account email first.
- `GET /graph/view` — Graph visualization
- `POST /mcp` — MCP JSON-RPC (memory_query, memory_status, memory_sources)
