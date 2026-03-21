# Helix load audit (notes)

## `/status`

Implementation loads all chunk rows via `helix_load_all_docs` to count vectors and enumerate source paths for the file tree. That is **O(chunks)** per status request when resolving a pack. Acceptable for local CLI use; if this becomes hot (e.g. polled frequently), consider perserving counts in manifest or a sidecar instead of full scans.

## `/query`

Uses `helix_hybrid_query` and does **not** load the full document set into memory for retrieval.

## Graph view

Uses graph counts and visualization paths as implemented in `server` / `helix_store`; not the same code path as bulk `helix_load_all_docs` for every chunk body unless the handler explicitly loads docs (check `graph_view` and related).
