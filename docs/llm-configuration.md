# LLM configuration (single precedence story)

memkit reads model and provider settings from several places. This is the authoritative order.

## Query synthesis (OpenAI chat completions)

Used when `OPENAI_API_KEY` is set and the server synthesizes answers.

1. **`MEMKIT_OPENAI_MODEL`** — raw OpenAI model id for the HTTP API (e.g. `gpt-5.2`, `gpt-4o-mini`). Empty values are ignored.
2. **`memkit.json` `model`** — if the value starts with `openai:`, the namespace is stripped and the remainder is sent to the API (e.g. `openai:gpt-5.2` → `gpt-5.2`).
3. **Built-in default** — `gpt-5.2` (see `DEFAULT_OPENAI_SYNTHESIS_MODEL` in `src/config.rs`).

## CLI defaults (`mk use model`)

`mk use model <id>` writes the namespaced id to `memkit.json` (e.g. `openai:gpt-5.2`). That file participates in step (2) above for synthesis.

## Embeddings and optional local inference

- **`MEMKIT_LLM_MODEL`** — GGUF path for **local** embed / optional llama features when the `llama-embedded` feature is enabled; not used for OpenAI synthesis.
- **`MEMKIT_LLM_PROVIDER`** — ontology / extraction backend (`rules` by default; `llama` only with the `llama-embedded` feature).

## Deprecated aliases

`MEMKIT_ONTOLOGY_*` env vars are deprecated in favor of `MEMKIT_LLM_*` where applicable.
