# Benchmarks

This directory is intentionally small.

The current benchmark baseline is:

- `run_longmemeval_benchmark.ts` for LongMemEval indexing + QA runs
- `summarize_longmemeval_misses.py` for miss clustering
- `download_data.ts` for fetching benchmark data when needed

The current benchmark path uses an **LLM extractor**:

- `MEMKIT_CONVERSATION_PROVIDER=openai` when `OPENAI_API_KEY` is set
- otherwise `MEMKIT_CONVERSATION_PROVIDER=llama`
- `EMBED_PROVIDER=fastembed`
- reranker as an explicit on/off ablation

Heuristics live in the extraction prompt and validation layer. The extractor should not silently fall back to rule-based memory creation.

## Generated Directories

These paths are generated and ignored:

- `benchmarks/data/`
- `benchmarks/node_modules/`
- `benchmarks/output/`

If `benchmarks/data/` is missing, download it again:

```bash
bun run download
```

## Canonical Commands

20-question stable baseline without reranking:

```bash
bun run run:20
```

20-question stable baseline with reranking:

```bash
bun run run:20:rerank
```

Generic run using the canonical runner:

```bash
MAX_QUESTIONS=50 USE_RERANKER=0 bun run run_longmemeval_benchmark.ts
```

Summarize misses for a saved run:

```bash
python3 summarize_longmemeval_misses.py output/<file>.jsonl
```

## Output Expectations

Each run writes:

- a JSONL predictions file in `benchmarks/output/`
- a matching log file in `benchmarks/output/`

The canonical runner logs:

- pack path
- reranker mode
- embedding config
- conversation extraction provider
- indexing elapsed time
- query elapsed time

## Workflow

1. Start `memkit-server`.
2. Run the canonical benchmark script with a fresh pack.
3. Compare the no-rerank and rerank arms on the same question count.
4. Run the miss summarizer on the winning arm.
5. Choose one next improvement wave from the dominant miss class.
