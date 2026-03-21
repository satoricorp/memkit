# Satori Benchmarks

> Legacy doc note: benchmark text below still references `satori` and older architecture assumptions.
> Current CLI binary is `mk`; current defaults and APIs are in `README.md`.

## Context

Satori indexes **documents** (files, markdown, code) into a memory pack, but the primary use case is **conversational and multi-turn**: users query memory through the CLI (`satori query`) and, more commonly, through the **SDK** when integrating with AI clients. Each query in a conversation may depend on prior turns; benchmarks must reflect this retrieval-in-conversation reality.

---

## Spec Criteria to Build Benchmarks

The following criteria from existing specs must be met or supported to implement the benchmark suite.

### From PRD (specs/legacy/prd-v1-local-memory.md)

| Criterion | Target | Benchmark Relevance |
|-----------|--------|---------------------|
| Warm query latency (vector-only) | P95 ≤ 300 ms | Primary latency benchmark gate |
| Warm query latency (hybrid) | P95 ≤ 450 ms | Hybrid mode latency benchmark |
| Incremental reindex (single file ≤ 1 MB) | ≤ 5 s | Indexing throughput benchmark |
| Idle daemon memory | ≤ 500 MB | Resource envelope benchmark |
| Active indexing memory | ≤ 2.5 GB | Resource envelope benchmark |
| Baseline hardware | MacBook Pro class | Reference machine for all benchmarks |

### From Validation Plan (specs/legacy/validation-plan-v1.md)

| Criterion | Target | Benchmark Relevance |
|-----------|--------|---------------------|
| Test dataset profiles | Small (1k), Medium (25k), Large (100k) chunks | Dataset sizes for latency and recall |
| Dataset content | Mixed markdown, code, plain text with known answer keys | Recall evaluation ground truth |
| 100-cycle ingest/update/query/reopen loop | No crashes, no corruption | Stability benchmark |
| P50/P95 warm query latency | Vector and hybrid | Latency benchmark metrics |
| Deterministic ordering | Hybrid retrieval | Reproducibility requirement |

### From Architecture (specs/legacy/architecture-v1.md)

| Criterion | Target | Benchmark Relevance |
|-----------|--------|---------------------|
| Query pipeline stages | Embed → Retrieve (Helix hybrid) → Rerank | Per-stage timing breakdown |
| Retrieval strategy | Hybrid (vector + FTS) with citation-first output | Recall and ranking evaluation |

---

## Benchmark Categories

### 1. Latency Benchmarks

**Purpose:** Measure query speed for conversational use (CLI and SDK).

**Spec criteria:**
- P95 vector-only ≤ 300 ms
- P95 hybrid ≤ 450 ms
- Per-stage timings: embed, retrieval, rerank, total

**Requirements to build:**
- Criterion-based `cargo bench` harness
- Warm-up queries before measurement (avoid cold-start bias)
- Dataset profiles: 1k, 25k, 100k chunks
- Both vector and hybrid modes

### 2. Recall Benchmarks

**Purpose:** Measure retrieval quality for multi-turn conversations where context must be found across indexed documents.

**Spec criteria:**
- Known answer keys (validation plan)
- Deterministic ranking for reproducibility

**Requirements to build:**
- Synthetic dataset with query → relevant chunk mappings
- Metrics: Hit@1, Hit@5, Hit@10, MRR
- Script: index dataset → run query suite → compute recall

### 3. Conversational / Multi-Turn Benchmarks

**Purpose:** Evaluate retrieval in conversation-like scenarios (LoCoMo, LongMemEval).

**Spec criteria:**
- Document indexing flow (unchanged)
- Query interface: CLI and SDK contract

**Requirements to build:**
- Adapter to index conversation histories as documents (turns or sessions as chunks)
- Adapter to run benchmark QA queries against indexed pack
- Alignment with external benchmark formats (LoCoMo, LongMemEval) for comparability

### 4. Stability and Resource Benchmarks

**Purpose:** Validate daemon behavior under sustained conversational load.

**Spec criteria:**
- 24h watch-mode soak with periodic queries
- Idle memory ≤ 500 MB, active indexing ≤ 2.5 GB
- 100-cycle ingest/update/query/reopen loop

**Requirements to build:**
- Soak test harness with configurable duration and query rate
- Memory/CPU sampling during run

---

## Interface Requirements

Benchmarks must exercise the same interfaces used in production:

1. **CLI:** `satori query "<query>" [--mode vector|hybrid] [--top-k N]`
2. **HTTP API:** `POST /query` with `query`, `mode`, `top_k`
3. **SDK (future):** Programmatic query API—benchmarks should be designed so SDK clients can reuse the same query contract and dataset.

---

## Profiling the query pipeline

Use `mk query` with `--output json` and inspect server logs / response `timings_ms` where exposed. For embed + rerank hot paths, profile a release build (`cargo build --release`) with your platform sampler (e.g. Instruments on macOS, `perf` on Linux) while running repeated queries against a fixed pack. Watch for duplicate embedding work between retrieve and rerank stages in `src/query.rs` / `src/rerank.rs`.

---

## Pre-Build Checklist

Before implementing benchmark scripts and harnesses:

- [ ] Dataset profiles (1k, 25k, 100k chunks) defined and obtainable
- [ ] Known answer keys or ground-truth mappings for recall
- [ ] Reference hardware documented (MacBook Pro class)
- [ ] PRD latency gates (300 ms / 450 ms) agreed as pass/fail
- [ ] CLI and API query contract stable
- [ ] Per-stage timing (`embed`, `retrieval`, `rerank`, `total`) exposed and consumable by benchmarks
