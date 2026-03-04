# Daemon API and MCP Contract V1

## Runtime

- Local daemon default bind: `127.0.0.1:7821`
- Transport: HTTP JSON for local API.
- MCP adapter exposed by daemon process (stdio or local HTTP bridge, implementation choice).

## Local HTTP API

### `GET /health`

Returns daemon health and version.

Response:

```json
{
  "status": "ok",
  "version": "0.1.0",
  "pack_loaded": true
}
```

### `GET /status`

Returns active pack, indexing state, and watcher state.

### `POST /query`

Request:

```json
{
  "query": "string",
  "mode": "hybrid",
  "top_k": 10,
  "filters": {
    "source_id": "optional"
  }
}
```

Response:

```json
{
  "results": [
    {
      "chunk_id": "id",
      "score": 0.91,
      "content": "text",
      "citation": {
        "file_path": "path",
        "chunk_index": 4,
        "start_offset": 1200,
        "end_offset": 1570
      }
    }
  ],
  "timings_ms": {
    "embed": 12,
    "retrieval": 55,
    "rerank": 9,
    "total": 79
  }
}
```

### `POST /index`

Triggers index run for configured sources.

### `POST /watch/start`

Starts incremental watch mode.

### `POST /watch/stop`

Stops watch mode.

## MCP Tools (V1)

### `memory_query`

Input:

- `query` (required, string)
- `top_k` (optional, int default 8)
- `mode` (optional: `vector` or `hybrid`, default `hybrid`)
- `source_id` (optional)

Output:

- `results[]` with `content`, `score`, `file_path`, `chunk_id`, `chunk_index`.
- `timings_ms`.

### `memory_status`

Returns daemon status, pack metadata, and index freshness.

### `memory_sources`

Lists configured source roots and watch/index status.

## Error Contract

- Standard response:

```json
{
  "error": {
    "code": "PACK_NOT_FOUND",
    "message": "No pack loaded",
    "hint": "Run satori init and satori index first"
  }
}
```

- Required error codes:
  - `PACK_NOT_FOUND`
  - `PACK_INVALID`
  - `INDEX_IN_PROGRESS`
  - `INDEX_FAILED`
  - `EMBEDDING_FAILED`
  - `QUERY_INVALID`
  - `INTERNAL_ERROR`

## Determinism Rules

- Given identical pack and query inputs, result ordering must be stable for equal scores.
- Response always includes citations for every returned chunk.
