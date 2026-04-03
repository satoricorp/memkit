# LLM configuration (single precedence story)

memkit reads model and provider settings from several places. This is the authoritative order.

## Query synthesis (OpenAI chat completions)

Used when `OPENAI_API_KEY` is set and the server synthesizes answers.

1. **`MEMKIT_OPENAI_MODEL`** — raw OpenAI model id for the HTTP API (e.g. `gpt-5.4`, `gpt-4o-mini`). Empty values are ignored.
2. **`memkit.json` `model`** — if the value starts with `openai:`, the namespace is stripped and the remainder is sent to the API (e.g. `openai:gpt-5.4` → `gpt-5.4`).
3. **Built-in default** — `gpt-5.4` (see `DEFAULT_OPENAI_SYNTHESIS_MODEL` in `src/config.rs`).

## CLI defaults (`mk use model` / `mk list`)

`mk use model <id>` writes the namespaced id to `memkit.json` (e.g. `openai:gpt-5.4`). Run `mk list` to see current and supported model IDs. That file participates in step (2) above for synthesis.

## Embeddings, conversation extraction, and optional graph inference

- **`MEMKIT_CONVERSATION_PROVIDER`** — conversation memory extraction backend. Use `openai` for hosted extraction or `llama` for local extraction.
- **`MEMKIT_CONVERSATION_MODEL`** — optional override for the conversation extraction model.
- **`MEMKIT_LLM_MODEL`** — GGUF path for **local** llama features when the `llama-embedded` feature is enabled.
- **`MEMKIT_LLM_PROVIDER`** — ontology provider for optional graph extraction (`rules` or `llama`).
- **`MEMKIT_GRAPH_ENABLED`** — graph/entity extraction toggle. Default is off.

## Deprecated aliases

`MEMKIT_ONTOLOGY_*` env vars are deprecated in favor of `MEMKIT_LLM_*` where applicable.
