# Daemon API and MCP Contract V1.1

> Legacy spec note: this document contains older `satori`/`7821` contract details.
> Current memkit runtime defaults to `127.0.0.1:4242` and is documented in `README.md`.
> Error responses are `{ "error": { "code", "message" } }` in the current server implementation.

This document aligns daemon behavior with the command-first runtime contract and explicit watch lifecycle management.

## Runtime

- Local daemon default bind: `127.0.0.1:7821`
- Transport: HTTP JSON for local API.
- MCP adapter exposed by daemon process (stdio or local HTTP bridge, implementation choice).
- TUI is not a required runtime surface.

## Local HTTP API

### `GET /health`

Returns daemon health and version.

Response:

```json
{
  "status": "ok",
  "version": "<git-short-sha>",
  "pack_loaded": true
}
```

### `GET /status`

Returns active pack, indexing state, and watcher state.

Response fields must include:

- `pack_path`
- `watch` object with:
  - `enabled` (bool)
  - `state` (`stopped|starting|running|stopping|error`)
  - `last_event_at` (nullable timestamp)
  - `last_index_at` (nullable timestamp)
  - `last_error` (nullable string)
- `index_freshness` object with:
  - `last_full_index_at`
  - `pending_changes` (count)

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

Enqueues background index run for configured sources.

Response:

```json
{
  "status": "accepted",
  "job": {
    "id": "job-12",
    "job_type": "index_sources",
    "state": "queued"
  }
}
```

### `GET /jobs`

Lists recent background jobs.

### `GET /jobs/:id`

Returns status of a specific background job.

### `POST /watch/start`

Starts incremental watch mode.

Request:

```json
{
  "pack": "./memory-pack",
  "debounce_ms": 500
}
```

### `POST /watch/stop`

Stops watch mode.

Request:

```json
{
  "pack": "./memory-pack"
}
```

## MCP Tools (V1.1)

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

Returns daemon status, pack metadata, index freshness, and watch lifecycle state.

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
  - `JOB_NOT_FOUND`
  - `WATCH_ALREADY_RUNNING`
  - `WATCH_NOT_RUNNING`
  - `WATCH_FAILED`
  - `EMBEDDING_FAILED`
  - `QUERY_INVALID`
  - `INTERNAL_ERROR`

## Determinism Rules

- Given identical pack and query inputs, result ordering must be stable for equal scores.
- Response always includes citations for every returned chunk.
