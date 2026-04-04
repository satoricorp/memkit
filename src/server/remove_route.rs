use std::path::PathBuf;

use anyhow::Result;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::pack::resolve_pack_dir;

use super::AppState;
use super::jobs::{enqueue_remove_job, start_next_job_if_idle};

#[derive(Deserialize, Default)]
pub(super) struct RemoveRequest {
    path: Option<String>,
}

pub(super) async fn remove_now(
    State(state): State<AppState>,
    Json(req): Json<RemoveRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let path = req.path.as_deref().ok_or((
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": { "code": "PATH_REQUIRED", "message": "remove requires path" }
        })),
    ))?;
    let dir = PathBuf::from(path).canonicalize().map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error":{"code":"PATH_INVALID","message":format!("path not accessible: {}", e)}}),
            ),
        )
    })?;
    let pack_dir = resolve_pack_dir(&dir);
    if !pack_dir.join("manifest.json").exists() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": { "code": "PACK_NOT_FOUND", "message": "No pack found at path" }
            })),
        ));
    }
    let pack_root = pack_dir
        .parent()
        .unwrap_or(pack_dir.as_path())
        .to_path_buf();
    let job = enqueue_remove_job(&state, pack_root.to_string_lossy().to_string()).await;
    start_next_job_if_idle(state.clone());
    Ok(Json(json!({
        "status": "accepted",
        "job": job
    })))
}
