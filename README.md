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

# Or run server directly
mk serve --pack ./memory-pack

# CLI commands (require server to be running)
mk list
mk status ~/memory
mk index ~/memory
mk query "local memory pack"
mk graph
```

## Commands

- `mk serve [--pack <path>] [--host] [--port]` — Start the server (API, MCP, SDK)
- `mk status [dir]` — With dir: show status for that pack. Without dir: show mk list
- `mk list` — List indexed directories with [local] [cloud]
- `mk index <dir>` — Start background index job, print job id
- `mk graph [--pack <dir>]` — Open graph view in browser
- `mk query "<text>" [--pack <dir>]` — Query default pack (or --pack)

## Environment

- `FALKORDB_SOCKET` (default `/tmp/falkordb.sock`)
- `FALKOR_GRAPH` (default `memkit`)
- `LANCEDB_PATH` (default `./.local-data/lance`)
- `API_PORT` (default `4242`)
- `MEMKIT_PACK_PATH` (default `./memory-pack` when using serve)
- `MEMKIT_ONTOLOGY_PROVIDER` (`llama` default; `rules` or `candle` optional)
- `MEMKIT_ONTOLOGY_MODEL` (GGUF model path for query synthesis)
- `MEMKIT_ONTOLOGY_MAX_TOKENS` (default `512`)
- `MEMKIT_ONTOLOGY_TIMEOUT_MS` (default `20000`)

For query synthesis, run `./scripts/model-fetch.sh` once to download a GGUF model, or set `MEMKIT_ONTOLOGY_MODEL` to your own path.

## Docker

```bash
cargo build --release
docker build -f docker/Dockerfile -t memkit .
docker run -p 4242:4242 -v memkit-data:/data -e AUTH_SECRET=dev-secret memkit
```

## API

- `GET /health` — Health check
- `GET /status` — Pack status
- `POST /query` — Query with synthesis
- `POST /index` — Trigger indexing
- `GET /graph/view` — Graph visualization
- `POST /mcp` — MCP JSON-RPC (memory_query, memory_status, memory_sources)
