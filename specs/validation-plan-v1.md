# Validation Plan V1

## Purpose

Define the mandatory validation steps before and during implementation for V1.

## Test Dataset Profiles

- Small: 1k chunks
- Medium: 25k chunks
- Large: 100k chunks

Each profile includes mixed markdown, code, and plain text files with known answer keys for recall checks.

## LanceDB Decision Milestone

## Pass/Fail Protocol

- Run 100-cycle ingest/update/query/reopen loop on small and medium datasets.
- Run 24h watch-mode soak with periodic queries.
- Validate cross-machine portability by copying pack and serving on second environment.

### Pass Conditions

- Zero blocking crashes.
- No detected data corruption.
- Hybrid retrieval operational with deterministic ordering.
- Meets PRD latency and indexing targets.

### Fail Conditions

- Reproducible instability in core operations.
- Portability or reopen failures.
- Performance gates consistently missed.

### Fallback

- Switch to `sqlite-vec` + separate keyword retrieval path.

## ONNX Runtime Choice Validation

- Run side-by-side benchmark:
  - `fastembed` provider
  - direct `ort` provider
- Same model, same query set, same hardware.
- Compare:
  - embedding throughput
  - query latency impact
  - memory footprint
  - implementation complexity and reliability

Decision rule:

- Keep `fastembed` if it meets performance/reliability targets and no blocker found.
- Use direct `ort` if `fastembed` introduces material limitations or instability.

## Baseline MacBook Performance Gate

- Target machine: baseline MacBook Pro class hardware.
- Measure:
  - P50/P95 warm query latency (vector and hybrid).
  - Incremental reindex time for add/edit/delete events.
  - Idle and active CPU/RAM envelope.
  - Long-run watch/query stability.

Gate passes only if all PRD thresholds are met.

## End-to-End MCP Validation

1. Create pack.
2. Index source folder.
3. Start daemon and MCP adapter.
4. Connect one MCP client.
5. Execute fixed query suite.
6. Verify returned citations and ranking determinism.

## Portability Validation

1. Build pack on machine A.
2. Copy pack directory to machine B.
3. Start serve on machine B.
4. Run fixed query suite.
5. Compare result integrity and acceptable ranking variance.

## Final Pre-Build Checklist Signoff

- Scope locked and documented.
- Interfaces locked (CLI, API, MCP, manifest).
- ONNX path locked with fallback.
- LanceDB decision protocol agreed.
- Baseline MacBook targets agreed.
- Validation dataset and scripts defined.
