# memkit

Memory pack CLI + server built on Helix (Rust).

## Install

Once a semver tag such as `v0.1.0` has been released, users can install `mk` with:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/satoricorp/memkit/releases/download/v0.1.0/memkit-installer.sh | sh
```

The CLI prints both the release version and build SHA in human-facing output, for example `memkit 0.1.0 (abc1234)`.

## Build From Source

```bash
cargo build --release
```

The CLI binary is `mk` (at `target/release/mk`).

## Release Automation

Tagging a semver release such as `v0.1.0` triggers [`release.yml`](/Users/joe/git/memkit/.github/workflows/release.yml), which:

- builds release archives for macOS arm64, macOS x86_64, and Linux x86_64
- generates a `curl | sh` installer script
- publishes GitHub Release artifacts and checksums

The curl-only release flow does not require a Homebrew tap repo or `HOMEBREW_TAP_TOKEN`.

## Convex Backend

The Convex backend for memkit lives in the repo-root [`convex/`](/Users/joe/git/memkit/convex), not in [`www/`](/Users/joe/git/memkit/www).

```bash
bun install
npx convex dev
```

Run those commands from the repo root (`/Users/joe/git/memkit`). The Rust CLI uses the Rust `convex` crate to talk to that deployment; the marketing site only consumes the generated API/types and the deployment URL.

## Quick start

```bash
# Build
cargo build --release

# Start server in background (single-pack by default; builds release binary if missing)
./scripts/local-start.sh

# Or run server directly
mk start --pack ./memory-pack

# Advanced: multi-pack mode
mk start --pack ./pack1,./pack2

# CLI commands (require server to be running)
mk list
mk status
mk status ~/memory
mk add ~/Documents/project-notes
mk query "local memory pack"
mk doctor
```

## Commands

- `mk add <path-or-url> [--pack <name-or-path>]` — Add local files or URL/docs to a pack.
- `mk remove [dir]` — Remove a pack (prompts unless `--yes`).
- `mk list` — Registered packs (same listing as `mk status` with no `dir`) plus current and supported model IDs.
- `mk status [dir]` — Without `dir`: list all registered packs. With `dir`: status for that pack.
- `mk query "<text>" [--pack <name-or-path>] [--top-k N] [--no-rerank] [--raw]` — Query a pack.
- `mk publish [--pack <name-or-path>] [--destination s3://bucket/prefix]` — Publish pack artifacts.
- `mk use pack <name-or-path>` — Set default pack.
- `mk use model <model-id>` — Set default model (see `mk list` for IDs).
- `mk login` — Browser sign-in for cloud auth.
- `mk logout` — Clear local auth and revoke the remote CLI session when possible.
- `mk whoami` — Show current auth state, profile, and JWT expiry.
- `mk doctor` — Config path and whether the API is reachable (`GET /health`).
- `mk start [--pack <path>] [--host] [--port] [--foreground]` — Start server (background by default).
- `mk stop [--port]` — Stop background server on the configured port.
- `mk schema [--format json|json-schema] [command]` — Introspect memkit or JSON Schema for agent inputs.

**Agents:** use a single JSON object with `mk -j '{...}'` or `mk --json` (see [CONTEXT.md](CONTEXT.md)). Global flags: `--output json`, `--dry-run`, `--version` / `-V`. Set `NO_COLOR=1` to disable ANSI colors in terminal output.

## Storage backend

The current build uses Helix as the local vector store with structured memory metadata.
Graph/entity extraction is available as an explicit opt-in, but it is not the default path.

```bash
cargo build --release
```

- `MEMKIT_HELIX_ROOT` — Base directory for Helix pack DBs (default `~/.memkit/helix`).
- `MEMKIT_GRAPH_ENABLED` — Optional graph/entity extraction toggle (`0` by default, set `1` to enable ontology/edge extraction).

## Configuration file

`mk` stores user preferences in:

- `~/.config/memkit/memkit.json` (or `$XDG_CONFIG_HOME/memkit/memkit.json`)

Fields:

- `model` (optional) — Default model ID for `mk use model <id>` (namespaced, e.g. `openai:gpt-5.4`). When it starts with `openai:`, the server may use it for query synthesis (see precedence below).
- `auth` (optional) — Persisted CLI auth state:
  - `sessionToken`
  - `jwt`
  - `jwtExpiresAt`
  - `profile`

**Query synthesis (OpenAI)** — order of precedence for which model the API calls:

1. `MEMKIT_OPENAI_MODEL` (raw OpenAI model id, e.g. `gpt-5.4`)
2. `memkit.json` `model` if it is an `openai:*` id
3. Built-in default `gpt-5.4`

Full detail: [docs/llm-configuration.md](docs/llm-configuration.md).

## Environment

- `API_HOST` / `API_PORT` (defaults `127.0.0.1` / `4242`)
- `MEMKIT_PACK_PATH` (default `./memory-pack` when using start; this is the normal single-pack path)
- `MEMKIT_PACK_PATHS` — Comma-delimited pack paths for advanced multi-pack mode
- `MEMKIT_PACKS` — Server-only comma-delimited pack override (advanced)
- `MEMKIT_HELIX_ROOT` — Helix pack DB base directory (default `~/.memkit/helix`)
- `MEMKIT_AUTH_BASE_URL` — Required for `mk login`; should point at the Convex site URL that serves `/api/auth/cli/start` and `/api/auth/cli/finish` (typically `https://<deployment>.convex.site`)
- `MEMKIT_CONVEX_URL` — Optional override for direct Convex SDK auth calls; if unset, memkit derives the matching `https://<deployment>.convex.cloud` URL from `MEMKIT_AUTH_BASE_URL`
- `OPENAI_API_KEY` — Required for query synthesis (OpenAI path; no local GGUF fallback in default builds)
- `MEMKIT_OPENAI_MODEL` — Optional override for OpenAI chat model (see precedence above)
- `MEMKIT_LLM_PROVIDER` — Optional ontology provider for graph extraction (`rules` or `llama` when graph extraction is enabled)
- `MEMKIT_LLM_MODEL` — Optional GGUF path for local llama feature builds (used for local extraction / graph mode when enabled)
- `MEMKIT_CONVERSATION_PROVIDER` — Conversation memory extraction backend (`openai` or `llama`)
- `MEMKIT_CONVERSATION_MODEL` — Optional conversation extraction model override
- `MEMKIT_LLM_MAX_TOKENS` (default `512`)
- `MEMKIT_LLM_TIMEOUT_MS` (default `20000`)
- `GOOGLE_APPLICATION_CREDENTIALS` — Path to service account JSON key (optional, for Google Docs/Sheets; preferred for local development)
- `MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON` — Inline service account JSON (optional; useful for CI / deployed runtimes that materialize the secret into a file at startup)

`MEMKIT_ONTOLOGY_*` env vars are deprecated aliases; use `MEMKIT_LLM_*` where applicable.

**Google Docs and Sheets (optional):** To index Google Docs or Sheets, configure a service account and share each doc/sheet with the service account email (no user OAuth). Set one of:

- `GOOGLE_APPLICATION_CREDENTIALS` — Path to a JSON key file for the service account (e.g. from GCP Console → IAM → Service accounts → Keys). This is the preferred local setup.
- `MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON` — Inline JSON string of the same key. Prefer this for deployment secrets, not for checked-in `.env` files.

The service account email is fixed (e.g. `name@project-id.iam.gserviceaccount.com`). You can get it from the JSON key (`client_email`) or from the API: `GET /google/service-account-email` when configured. Share your Doc or Sheet with that email (Viewer or Editor), then add via `POST /add` with `documents: [{ "type": "google_doc", "value": "<URL or doc ID>" }]` or `"type": "google_sheet"` with a Sheet URL or spreadsheet ID.

For local development, a typical setup is:

```bash
GOOGLE_APPLICATION_CREDENTIALS=/Users/joe/.config/memkit/google-service-account.json
```

For containerized deploys, the image entrypoint can accept `MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON` as a secret, write it to a locked-down file, and export `GOOGLE_APPLICATION_CREDENTIALS` automatically. See [docs/deployment-secrets.md](docs/deployment-secrets.md).

TODO: revisit Google Docs/Sheets auth before writing full public docs. The current service-account flow works for local power users who bring their own credentials, but it is not the right default story for an open-source CLI. The likely product split is: bring-your-own Google credentials for local/open-source use, and hosted Google ingestion as a paid API feature. See [docs/google-auth-roadmap.md](docs/google-auth-roadmap.md).

## Docker

Helix-only image: build the binary on the host, then copy it into a small Debian image.

```bash
cargo build --release
docker build -f docker/Dockerfile -t memkit .
docker run --rm -p 4242:4242 \
  -v "$PWD/memory-pack:/data/pack" \
  -e MEMKIT_HELIX_ROOT=/data/helix \
  -e MEMKIT_PACK_PATH=/data/pack \
  -e OPENAI_API_KEY="$OPENAI_API_KEY" \
  memkit
```

For local iteration, `./scripts/local-start.sh` is usually simpler than Docker.

## API

- `GET /health` — Health check (`version` is semver; `git_sha` is included when available)
- `GET /status` — Pack status
- `GET /google/service-account-email` — Service account email for sharing (when Google is configured)
- `POST /query` — Query memory records with synthesis
- `POST /index` — Trigger indexing
- `POST /add` — Add documents or conversations. Body: `documents: [{ "type": "url"|"content"|"google_doc"|"google_sheet", "value": "..." }]`, or `conversation: [{ "role", "content" }]`.
- `GET /graph/view` — Graph visualization
- `POST /mcp` — MCP JSON-RPC (memory_query, memory_status, memory_sources)
