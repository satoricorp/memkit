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

# Start server (FalkorDB sidecar + API)
./scripts/local-start.sh

# Or run server directly (single or multiple packs, comma-delimited)
mk serve --pack ./memory-pack
mk serve --pack ./pack1,./pack2

# CLI commands (require server to be running)
mk list
mk status ~/memory
mk index ~/memory
mk query "local memory pack"
mk graph
```

## Commands

- `mk serve [--pack <path>] [--host] [--port]` — Start the server. `--pack` accepts comma-delimited paths for multi-pack mode.
- `mk status [dir]` — With dir: show status for that pack. Without dir: show mk list
- `mk list` — List indexed directories with [local] [cloud]
- `mk index <dir>` — Start background index job, print job id
- `mk graph [--pack <dir>]` — Open graph view in browser
- `mk query "<text>" [--pack <dir>]` — Query default pack (or --pack)
- `mk schema [command]` — Introspect input schema for commands (agent-friendly)

Agent-friendly flags: `--json` (input), `--output json`, `--dry-run`. See [CONTEXT.md](CONTEXT.md).

## Environment

- `FALKORDB_SOCKET` (default `/tmp/falkordb.sock`)
- `FALKOR_GRAPH` (default `memkit`)
- `LANCEDB_PATH` (default `./.local-data/lance`)
- `API_PORT` (default `4242`)
- `MEMKIT_PACK_PATH` (default `./memory-pack` when using serve)
- `MEMKIT_PACK_PATHS` — Comma-delimited pack paths for multi-pack mode (overrides `MEMKIT_PACK_PATH` when set)
- `MEMKIT_LLM_PROVIDER` (`llama` default; `rules` or `candle` optional)
- `MEMKIT_LLM_MODEL` (GGUF model path for ontology extraction and query synthesis)
- `MEMKIT_LLM_MAX_TOKENS` (default `512`)
- `MEMKIT_LLM_TIMEOUT_MS` (default `20000`)
- `GOOGLE_APPLICATION_CREDENTIALS` — Path to service account JSON key (optional, for Google Docs/Sheets)
- `MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON` — Inline service account JSON (optional; overrides path)

`MEMKIT_ONTOLOGY_*` env vars are deprecated aliases; use `MEMKIT_LLM_*` instead.

For query synthesis, run `./scripts/model-fetch.sh` once to download a GGUF model, or set `MEMKIT_LLM_MODEL` to your own path.

**Google Docs and Sheets (optional):** To index Google Docs or Sheets, configure a service account and share each doc/sheet with the service account email (no user OAuth). Set one of:

- `GOOGLE_APPLICATION_CREDENTIALS` — Path to a JSON key file for the service account (e.g. from GCP Console → IAM → Service accounts → Keys).
- `MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON` — Inline JSON string of the same key.

The service account email is fixed (e.g. `name@project-id.iam.gserviceaccount.com`). You can get it from the JSON key (`client_email`) or from the API: `GET /google/service-account-email` when configured. Share your Doc or Sheet with that email (Viewer or Editor), then add via `POST /add` with `documents: [{ "type": "google_doc", "value": "<URL or doc ID>" }]` or `"type": "google_sheet"` with a Sheet URL or spreadsheet ID.

## Docker

```bash
cargo build --release
docker build -f docker/Dockerfile -t memkit .
docker run -p 4242:4242 -v memkit-data:/data -e AUTH_SECRET=dev-secret memkit
```

## API

- `GET /health` — Health check
- `GET /status` — Pack status
- `GET /google/service-account-email` — Service account email for sharing (when Google is configured)
- `POST /query` — Query with synthesis
- `POST /index` — Trigger indexing
- `POST /add` — Add documents. Body: `documents: [{ "type": "url"|"content"|"google_doc"|"google_sheet", "value": "..." }]`, or `conversation: [{ "role", "content" }]`. For `google_doc` / `google_sheet`, share the doc/sheet with the service account email first.
- `GET /graph/view` — Graph visualization
- `POST /mcp` — MCP JSON-RPC (memory_query, memory_status, memory_sources)
