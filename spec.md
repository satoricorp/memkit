# Project specification (memkit)

**Scope:** the **memkit** Rust CLI + HTTP server, **SDKs** (`packages/sdk*`), **scripts**, and **docs**. The **`www/`** tree is out of scope.

---

## What this project is

**memkit** is a local **memory pack** system: a Rust binary **`mk`** with an **Axum** API. Packs store vectors and graph data in **Helix** only (LMDB per pack). Thin **TS / Go / Python** clients call the same HTTP API.

---

## ML architecture (policy)

| Layer | Mechanism | Notes |
|--------|-----------|--------|
| **Embeddings** | **fastembed** (local) | Pack indexing and query embedding use the configured fastembed embedding model (e.g. BGE-small). Keeps recall path fast and avoids OpenAI latency on the hot path. |
| **Reranking** | **fastembed** cross-encoder (local, `rerank.rs`) | Jina / BGE-style rerankers stay embedded—no HTTP rerank call. |
| **Everything else** | **OpenAI API** | Query synthesis, ontology / structured extraction, and any other LLM-backed features **require** `OPENAI_API_KEY` and call OpenAI over HTTPS. |

**Default OpenAI chat model:** **`gpt-5.4`** (override via config/env when you wire it, e.g. `MEMKIT_OPENAI_MODEL` or whatever the codebase standardizes alongside `README.md`).

**Removed / avoided for LLM work:** in-process **GGUF** / **`llama-cpp`** as the primary path for those features—use OpenAI instead when a key is present.

*Implementation may still lag this document until refactors land.*

---

## Rust crate (summary)

| Area | Crates (representative) |
|------|-------------------------|
| Server | `axum`, `tokio` |
| Store | `helix-db` (+ `heed3`, `bumpalo`) with `helix` / `store-helix-only` |
| Local ML | `fastembed` (embeddings + reranker) |
| HTTP / CLI | `reqwest`, `serde`, `anyhow`, … |
| Cloud / Google / publish | `aws-sdk-s3`, `yup-oauth2`, … |

**Binary:** `mk` → `src/main.rs`. See **`Cargo.toml`** for feature flags after LLM refactors.

---

## SDKs (`packages/sdk`, `sdk-go`, `sdk-py`)

- HTTP client + **`memkit(model)`** tool shapes (OpenAI vs Anthropic).
- **`configure`**, **`query`**, **`add`**, **`executeTool`** → daemon routes (`/query`, `/add`, `/status`, …).
- TS build: **`bun run tsc`**.

---

## Layout (in scope)

```
Cargo.toml, src/          # mk + server + query/index/pack/…
packages/sdk{,-go,-py}/   # clients
scripts/, docker/
specs/, docs/, tests/, benchmarks/
README.md, CONTEXT.md
```

---

## Operations

| Scripts | Role |
|---------|------|
| `local-build.sh` | `cargo build --release --bin mk` from repo root |
| `local-start.sh` | Build `mk` if missing, then background `mk serve` (pid + log under `.local-run/`) |
| `local-stop.sh` | Stop the background `mk` process |
| `local-status.sh` | Pid status + `GET /health` |

---

## Environment (high level)

- **Daemon:** `API_HOST`, `API_PORT`, `MEMKIT_PACK_PATH(S)`, Helix (`MEMKIT_HELIX_ROOT`, etc.) — see **`README.md`**.
- **OpenAI (required for non–fastembed LLM features):** `OPENAI_API_KEY`; default model **`gpt-5.4`** for chat/completions-style calls (exact env name TBD in code).
- **fastembed:** `MEMKIT_MODEL_CACHE` (and pack manifest embedding model ids).
- **Google Docs/Sheets:** service-account vars if used.
- **SDK:** `MEMKIT_URL` (TS client default) for remote vs local daemon.

---

## CLI & schema (agents)

- **One JSON entry point:** `mk --json | -j '<object>'` with a required **`command`** field. Per-subcommand `--json` was removed; agents should not rely on `mk add --json`.
- **`mk list`** — registered packs (same core listing as **`mk status`** with no `dir`) plus current and supported models; replaces **`mk models`**.
- **`mk use` (shell):** **`mk use pack <name>`** / **`mk use model <id>`** only (set defaults). JSON **`use`** still accepts **`pack`** / **`model`** (`null` = show, string = set).
- **`mk doctor`** — local checks: config path, `GET /health` against `API_HOST`:`API_PORT`.
- **`mk schema`** — lists commands with schemas; includes **`publish`** and **`doctor`**. JSON Schema–style strictness can be added later (see plan below).

### Schema evolution plan

1. **Done:** `schema_for_command` documents `add`, `remove`, `status`, `query`, `publish`, `use`, `list`, `doctor`; `use` describes `pack` / `model` semantics.
2. **Next:** Optional **JSON Schema** export (`mk schema --format json-schema`) for validation in CI or agent loops.
3. **Next:** Mark `required` arrays per command to match runtime validation exactly.

---

## Further reading

**`specs/`**, **`docs/`**, **`README.md`**, **`CONTEXT.md`** — API lists, pack format, agent CLI usage.
