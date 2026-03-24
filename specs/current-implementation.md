# memkit Current Implementation Baseline

This file is the authoritative current-state reference for runtime behavior.

## Naming and binary

- Product/runtime name: `memkit`
- CLI binary: `mk`

## Runtime defaults

- API bind: `127.0.0.1:4242` (unless overridden by `API_HOST`/`API_PORT`)
- Server command: `mk start`
- Stop command: `mk stop`

## Storage

- Primary local store: Helix-backed pack data
- Pack manifest path: `manifest.json` in resolved pack dir (`<root>/.memkit` or direct pack dir)

## Current command set

- `mk add`
- `mk remove`
- `mk status`
- `mk status` (no `dir` lists packs only)
- `mk list` (packs + current/supported models)
- `mk query`
- `mk publish`
- `mk use pack <name>` / `mk use model <id>` (shell); JSON `use` still supports show/set
- `mk start`
- `mk stop`
- `mk schema`

## API error shape

Current server error envelope is:

```json
{
  "error": {
    "code": "STRING_CODE",
    "message": "human-readable message"
  }
}
```

Use this baseline when older spec files still describe `satori`, port `7821`, or LanceDB-only flows.
