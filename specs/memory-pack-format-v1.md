# Memory Pack Format V1

## Purpose

Define a portable, self-describing folder format for local memory indexing and querying.

## Directory Layout

```text
memory-pack/
  manifest.json
  config.json
  sources/
    <ingested files mirrored or referenced>
  lancedb/
    <lancedb tables and indexes>
  state/
    file_state.json
    checkpoints.json
  logs/
    ingest.log
```

## Manifest

`manifest.json` is required and versioned.

```json
{
  "format_version": "1.0.0",
  "pack_id": "uuid",
  "created_at": "2026-03-02T00:00:00Z",
  "updated_at": "2026-03-02T00:00:00Z",
  "embedding": {
    "provider": "fastembed",
    "model": "BAAI/bge-small-en-v1.5",
    "model_sha256": "hex",
    "dimension": 384
  },
  "storage": {
    "engine": "lancedb",
    "path": "lancedb/"
  },
  "chunking": {
    "strategy": "token_window",
    "target_tokens": 450,
    "overlap_tokens": 50
  },
  "sources": [
    {
      "id": "source-id",
      "root_path": "/absolute/or/logical/path",
      "include": ["**/*"],
      "exclude": ["**/.git/**", "**/node_modules/**"]
    }
  ]
}
```

## Data Model

### Required LanceDB Table: `chunks`

- `chunk_id` (string, primary logical key)
- `source_id` (string)
- `file_path` (string)
- `content` (string)
- `embedding` (fixed-size float list)
- `content_hash` (string)
- `chunk_index` (int)
- `start_offset` (int, nullable)
- `end_offset` (int, nullable)
- `updated_at` (timestamp)

### Optional Table: `documents`

- `document_id`, `file_path`, `file_hash`, `mtime`, `size`, `last_indexed_at`

## Index Requirements

- Vector index on `embedding`.
- FTS/BM25 index on `content`.
- Index creation parameters are implementation-configurable but must be persisted in `config.json`.

## Incremental Indexing State

`state/file_state.json` stores file-level digest and indexing checkpoint:

- `file_path`
- `content_hash`
- `mtime`
- `size`
- `last_chunk_count`
- `last_indexed_at`

## Portability Rules

- Pack is portable as a folder copy.
- Relative paths inside manifest and config must remain valid after relocation.
- Absolute source paths are advisory only and not required for querying existing indexed content.

## Compatibility

- Breaking changes require `format_version` major bump.
- Non-breaking additive fields are allowed in minor updates.
- Reader behavior:
  - Unknown fields: ignore.
  - Missing required fields: fail fast with actionable error.

## Integrity

- `model_sha256` is required when model artifacts are pinned.
- `content_hash` required per chunk for change detection and debugging.
