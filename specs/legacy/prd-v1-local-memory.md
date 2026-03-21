# PRD: Local Memory Pack V1.1 (Command-First)

## Objective

Ship a local-first memory system that lets users index folders into a portable memory pack and query that pack from MCP-compatible AI clients.

## Product Summary

`satori` V1.1 is a Rust command-first CLI + daemon that turns local folders into a queryable memory pack. The pack is portable (folder copy), local by default, and consumable via MCP so AI clients can retrieve grounded context with citations.

## Problem Statement

Current memory workflows are fragmented across tools and often cloud-dependent. Users need a local-first option that:

- does not require managed infrastructure,
- is fast on laptop hardware,
- is portable across machines, and
- exposes one standard retrieval interface for AI clients.

## Scope

### In Scope

- Rust single-binary CLI and daemon.
- Local folder ingestion, chunking, embedding, and incremental reindex.
- Portable on-disk memory pack format with versioned manifest.
- Local query API and MCP adapter.

### Out of Scope

- Cloud sync, team sharing, identity, and hosted UI.
- Desktop app UX.
- Public connector marketplace (only extension points are defined).
- TUI as a required runtime surface.

## Non-Goals

- Multi-tenant cloud architecture.
- Billing, auth, and account management.
- GUI onboarding and non-technical setup flow.
- Remote connectors (Gmail, Slack, etc.) beyond format/extension readiness.

## Users

- Developer users integrating memory into MCP clients.
- Power users who run a local daemon and point AI tools at local memory.

## Primary User Stories

1. As a developer, I can initialize a pack and index my repo so my MCP client can answer with project-grounded context.
2. As a power user, I can keep a daemon running in watch mode and get fresh results after file changes.
3. As a user moving machines, I can copy my pack folder and serve it on another machine without full rebuild.

## Success Criteria

- User runs `satori init`, `satori index`, and `satori serve` successfully.
- One MCP client can query local memory and receive ranked context with citations.
- Pack directory can be copied to another machine and served without rebuild.
- Incremental reindex updates only changed files/chunks.

## Functional Requirements

1. Create memory pack scaffold and config.
2. Index one or more local folders into a pack.
3. Watch mode for incremental updates (add/edit/delete).
4. Query by vector-only and hybrid retrieval modes.
5. Serve local query functionality through MCP.
6. Return source metadata: file path, chunk ID, byte/line range (when available).

## Command and Interface Contract (V1.1)

- Required CLI surface:
  - `satori init`
  - `satori index`
  - `satori serve`
  - `satori watch start`
  - `satori watch stop`
  - `satori query`
  - `satori status`
- Required daemon interfaces:
  - local health/status endpoints
  - query endpoint with vector and hybrid modes
  - watch control endpoints
- Required MCP tools:
  - `memory_query`
  - `memory_status`
  - `memory_sources`

## Non-Functional Requirements

### Baseline Hardware Target

- Baseline target machine: MacBook Pro class device.
- Primary optimization target: low-latency local query while indexing runs in background.

### Performance Targets (initial V1 gates)

- Warm query latency (vector-only): P95 <= 300 ms.
- Warm query latency (hybrid): P95 <= 450 ms.
- Incremental reindex after single file edit: <= 5 s for files <= 1 MB.
- Idle daemon memory: <= 500 MB.
- Active indexing memory: <= 2.5 GB.
- Idle CPU: <= 10% sustained.

### Reliability Targets

- No data corruption during repeated index/update/query cycles.
- Daemon restart can reopen existing pack without repair in normal shutdown scenarios.
- Watch mode can run for 24h without crash in soak test.

### Portability Targets

- Pack folder copied to a second machine must serve existing indexed data without full reindex.
- Manifest/schema compatibility checks must fail fast with actionable errors for unsupported versions.

## Locked Technical Decisions (V1)

- Storage/index engine candidate: LanceDB, gated by decision milestone.
- Embedding runtime: ONNX-based in-process inference.
- Embedding provider strategy:
  - Primary: `fastembed`.
  - Fallback: direct `ort` + tokenizer pipeline.
  - Implementation: `EmbeddingProvider` abstraction.
- Retrieval strategy: hybrid (BM25/FTS + vector) with deterministic ranking and citation-first output.

## Data and Security Constraints

- Local-first default: no required outbound data transfer for core V1 flow.
- Query responses must include citations to reduce hallucinated context.
- Model artifacts may be downloaded once (unless pre-bundled); versions must be pinned in pack config/manifest.

## Validation and Gating

V1.1 build can proceed only after:

1. LanceDB decision gate passes (stability + hybrid + portability + performance).
2. ONNX provider choice validated (`fastembed` primary; `ort` fallback available by abstraction).
3. Baseline MacBook performance gates pass on representative datasets.

V1.1 release can proceed only after:

1. End-to-end MCP validation passes with one client.
2. Portability test passes across two environments.
3. 24h watch/query soak test passes.
4. Error taxonomy and recovery behaviors are documented and tested.

## Risks

- LanceDB Rust stability in update-heavy workloads.
- Model artifact size and first-run download friction.
- File watcher behavior differences across platforms.
- Manifest migration complexity as pack format evolves.

## Milestones

1. Specs finalized and locked.
2. LanceDB decision gate completed.
3. Index/query core implemented.
4. MCP adapter implemented and validated.
5. Baseline MacBook performance gate passed.

## Acceptance

V1.1 is accepted when all success criteria, performance gates, and reliability targets in this document are met, including watch lifecycle reliability and freshness reporting.
