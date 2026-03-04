# Architecture V1: Local Memory Pack

## System Overview

V1 is a local-first Rust system with three runtime surfaces:

- CLI for setup, indexing, serving, and diagnostics.
- Daemon for query and watch workflows.
- MCP adapter for AI client integration.

## Component Diagram

```mermaid
flowchart LR
  cli[SatoriCLI] --> orchestrator[IndexOrchestrator]
  orchestrator --> parser[ParserChunker]
  parser --> embedder[EmbeddingProvider]
  embedder --> store[LanceDBStore]
  watcher[FileWatcher] --> orchestrator
  daemon[SatoriDaemon] --> retriever[HybridRetriever]
  retriever --> store
  mcp[MCPAdapter] --> daemon
```

## Retrieval Flow

1. Receive query through daemon or MCP.
2. Embed query using `EmbeddingProvider`.
3. Run vector retrieval from LanceDB.
4. Run FTS/BM25 retrieval from LanceDB.
5. Fuse/rerank results with deterministic sort tiebreak.
6. Return top-k chunks with citation metadata.

## Ingestion Flow

1. Scan configured source roots with include/exclude globs.
2. Parse and chunk files into normalized units.
3. Compute content hashes and compare against `state/file_state.json`.
4. Embed only new/changed chunks.
5. Upsert rows into `chunks` table and refresh indexes as required.
6. Persist file state checkpoint.

## Core Dependencies

- Rust async runtime: `tokio`
- Storage/index: `lancedb`
- Embeddings (primary): `fastembed`
- Embeddings (fallback): `ort` + tokenizer stack
- Watcher: `notify`
- Serialization: `serde`, `serde_json`
- Error handling: `thiserror` and/or `anyhow`

## ONNX Strategy

- Keep ONNX inference local and in-process.
- Default provider is `fastembed` for integration speed.
- Keep `EmbeddingProvider` trait boundary to allow fallback to direct `ort`.
- Pin model version and checksum for reproducibility.

## LanceDB Decision Gate

LanceDB is selected only if all of the following pass:

- No blocking instability in ingest/update/query/reopen loops.
- Hybrid search path functions end-to-end in Rust.
- Portability test passes across environments.
- Meets PRD latency and reindex budgets.

If not passed, fallback is `sqlite-vec` + separate keyword index while preserving pack format contract.

## Deployment Modes

### Local

- Single daemon process on user machine.
- Local filesystem pack + local model cache.

### Cloud/Server

- Same daemon binary in container/VM.
- Persistent volume for pack and model cache.
- Optional object storage sync outside V1 scope.

## Observability

- Structured logs for ingest/query/watch.
- Timing spans for embed, retrieval, rerank, and total.
- Health and status endpoints expose freshness and failure state.
