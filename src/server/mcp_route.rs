use std::path::PathBuf;

use anyhow::Result;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::pack::load_manifest;
use crate::pack_location::PackLocation;
use crate::query::{run_query, run_query_multi};
use crate::registry::pack_dir_for_path;

use super::AppState;

pub(super) async fn mcp(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let method = payload.get("method").and_then(Value::as_str).unwrap_or("");
    let id = payload.get("id").cloned().unwrap_or(json!(null));

    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {
                "name":"memkit",
                "version": crate::term::release_version(),
                "gitSha": crate::term::git_sha()
            },
            "capabilities": {"tools": {}}
        }),
        "tools/list" => json!({
            "tools": [
                {"name":"memory_query","description":"Query local memory pack","inputSchema":{
                    "type":"object","properties":{
                        "query":{"type":"string"},
                        "top_k":{"type":"number"},
                        "use_reranker":{"type":"boolean"}
                    },
                    "required":["query"]
                }},
                {"name":"memory_status","description":"Return daemon status","inputSchema":{"type":"object","properties":{}}},
                {"name":"memory_sources","description":"List active memory source roots","inputSchema":{"type":"object","properties":{}}}
            ]
        }),
        "tools/call" => {
            let name = payload
                .get("params")
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let args = payload
                .get("params")
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or_else(|| json!({}));

            match name {
                "memory_query" => {
                    let query = args
                        .get("query")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let top_k = args.get("top_k").and_then(Value::as_u64).unwrap_or(8) as usize;
                    let use_reranker = args
                        .get("use_reranker")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);

                    let pack_dirs: Vec<PathBuf> = state
                        .packs
                        .iter()
                        .map(|r| pack_dir_for_path(r.as_path()))
                        .collect();
                    let resp = if state.packs.len() > 1 {
                        run_query_multi(&pack_dirs, &query, top_k, use_reranker, None)
                    } else {
                        let Some(p) = pack_dirs.first() else {
                            return Ok(Json(json!({
                                "jsonrpc":"2.0",
                                "id":id,
                                "result":{"isError": true, "content":[{"type":"text","text":"no pack configured"}]}
                            })));
                        };
                        run_query(&PackLocation::local(p), &query, top_k, use_reranker, None)
                    };
                    match resp {
                        Ok(r) => json!({
                            "content":[{"type":"text","text":json!({
                                "mode": r.mode,
                                "timings_ms": r.timings_ms,
                                "results": r.results,
                                "grouped_results": r.grouped_results
                            }).to_string()}]
                        }),
                        Err(e) => {
                            json!({"isError": true, "content":[{"type":"text","text":e.to_string()}]})
                        }
                    }
                }
                "memory_status" => {
                    let pack_path_display = state
                        .packs
                        .first()
                        .map(|p| display_pack_path(p))
                        .unwrap_or_default();
                    let pack_paths: Vec<String> =
                        state.packs.iter().map(|p| display_pack_path(p)).collect();
                    json!({
                        "content":[{"type":"text","text":json!({
                            "status":"ok",
                            "pack_path": pack_path_display,
                            "pack_paths": pack_paths
                        }).to_string()}]
                    })
                }
                "memory_sources" => {
                    let mut all_sources = Vec::new();
                    for pack_root in state.packs.iter() {
                        let pack_dir = pack_dir_for_path(pack_root);
                        if let Ok(m) = load_manifest(&pack_dir) {
                            all_sources.extend(m.sources);
                        }
                    }
                    json!({
                        "content":[{"type":"text","text":json!({"sources":all_sources}).to_string()}]
                    })
                }
                _ => json!({"isError": true, "content":[{"type":"text","text":"unknown tool"}]}),
            }
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"BAD_METHOD","message":"unsupported method"}})),
            ));
        }
    };

    Ok(Json(json!({"jsonrpc":"2.0","id":id,"result":result})))
}

fn display_pack_path(pack_root: &std::path::Path) -> String {
    let is_home = dirs::home_dir()
        .as_ref()
        .and_then(|h| h.canonicalize().ok())
        .as_ref()
        == pack_root.canonicalize().as_ref().ok();
    if is_home {
        "~/.memkit".to_string()
    } else {
        pack_root.display().to_string()
    }
}
