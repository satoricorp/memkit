use std::path::PathBuf;

use anyhow::Result;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::pack_location::PackLocation;
use crate::query::{run_query, run_query_multi};
use crate::query_synth::{QueryProvider, synthesize_answer_async};
use crate::registry::pack_dir_for_path;

use super::{
    AppState, authenticated_cloud_context, default_top_k, default_use_reranker,
    resolve_cloud_query_location, resolve_strict_local_pack_dir,
};

#[derive(Deserialize)]
pub(super) struct QueryRequest {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
    #[serde(default = "default_use_reranker")]
    use_reranker: bool,
    #[serde(default)]
    raw: bool,
    pack: Option<String>,
    #[serde(default)]
    pack_uri: Option<String>,
    #[serde(default)]
    path_filter: Option<String>,
}

/// Query flow: (1) Retrieval: run_query() loads pack docs, embeds the query, and runs vector search
/// (Helix: helix_hybrid_query). Returns QueryResponse with results (top chunks).
/// (2) Synthesis: unless req.raw, synthesize_answer_async calls OpenAI chat/completions. Use ?raw=true or --raw to skip synthesis.
pub(super) async fn query(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<QueryRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if req.pack.is_some() && req.pack_uri.is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error":{"code":"INVALID_QUERY","message":"use either pack or pack_uri, not both"}}),
            ),
        ));
    }

    let requested_cloud_uri = req
        .pack_uri
        .as_deref()
        .or_else(|| req.pack.as_deref().filter(|p| p.starts_with("memkit://")));
    let resp_result = if let Some(pack_uri) = requested_cloud_uri {
        let auth = authenticated_cloud_context(&headers).await?;
        let loc = resolve_cloud_query_location(&state, &auth, pack_uri)?;
        run_query(
            &loc,
            &req.query,
            req.top_k,
            req.use_reranker,
            req.path_filter.as_deref(),
        )
    } else if let Some(ref path) = req.pack {
        if path.starts_with("s3://") {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(
                    json!({"error":{"code":"INVALID_PACK","message":"S3 pack paths are not supported in this build"}}),
                ),
            ));
        }
        let pack = resolve_strict_local_pack_dir(path).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"INVALID_PACK","message":e.to_string()}})),
            )
        })?;
        let loc = PackLocation::local(pack);
        run_query(
            &loc,
            &req.query,
            req.top_k,
            req.use_reranker,
            req.path_filter.as_deref(),
        )
    } else if state.packs.len() > 1 {
        let pack_dirs: Vec<PathBuf> = state
            .packs
            .iter()
            .map(|r| pack_dir_for_path(r.as_path()))
            .collect();
        run_query_multi(
            &pack_dirs,
            &req.query,
            req.top_k,
            req.use_reranker,
            req.path_filter.as_deref(),
        )
    } else {
        let Some(pack_root) = state.packs.first() else {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"NO_PACK","message":"no pack configured"}})),
            ));
        };
        let pack_dir = pack_dir_for_path(pack_root);
        run_query(
            &PackLocation::local(&pack_dir),
            &req.query,
            req.top_k,
            req.use_reranker,
            req.path_filter.as_deref(),
        )
    };

    match resp_result {
        Ok(resp) => {
            let mut by_path: std::collections::HashMap<String, f32> =
                std::collections::HashMap::new();
            for h in &resp.results {
                by_path
                    .entry(h.file_path.clone())
                    .and_modify(|s| *s = (*s).max(h.score))
                    .or_insert(h.score);
            }
            let mut sources: Vec<_> = by_path
                .into_iter()
                .map(|(path, score)| json!({ "path": path, "score": score }))
                .collect();
            sources.sort_by(|a, b| {
                let sa = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let sb = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            });

            if req.raw {
                return Ok(Json(json!({
                    "mode": resp.mode,
                    "timings_ms": resp.timings_ms,
                    "results": resp.results,
                    "grouped_results": resp.grouped_results,
                    "retrieval_results": resp.retrieval_results,
                    "sources": sources,
                    "resolved_pack_path": resp.resolved_pack_path,
                    "notes": resp.notes,
                    "hydrated_evidence": resp.hydrated_evidence,
                    "query_time": resp.query_time
                })));
            }

            let (answer, provider) = synthesize_answer_async(&req.query, &resp)
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error":{"code":"QUERY_SYNTH_FAILED","message":e.to_string()}})),
                    )
                })?;
            let provider_label = match provider {
                QueryProvider::None => "none".to_string(),
                QueryProvider::OpenAI(model) => model,
            };
            Ok(Json(json!({
                "answer": answer,
                "model": provider_label,
                "mode": resp.mode,
                "timings_ms": resp.timings_ms,
                "results": resp.results,
                "grouped_results": resp.grouped_results,
                "retrieval_results": resp.retrieval_results,
                "sources": sources,
                "resolved_pack_path": resp.resolved_pack_path,
                "notes": resp.notes,
                "hydrated_evidence": resp.hydrated_evidence,
                "query_time": resp.query_time
            })))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"QUERY_FAILED","message":e.to_string()}})),
        )),
    }
}
