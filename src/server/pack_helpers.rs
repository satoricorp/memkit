use std::path::{Path, PathBuf};

use anyhow::Result;
use axum::Json;
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::pack::{has_manifest_at, init_pack, resolve_pack_dir};
use crate::registry::{
    ensure_registered, load_registry, pack_dir_for_path, resolve_pack_by_name_or_path,
};

use super::AppState;

pub(super) fn resolve_pack_root_for_add(
    state: &AppState,
    pack_override: Option<&str>,
) -> Result<PathBuf, (StatusCode, Json<Value>)> {
    let root = if let Some(p) = pack_override {
        resolve_strict_local_pack_root(p).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"PATH_INVALID","message":e.to_string()}})),
            )
        })?
    } else {
        state.packs.first().cloned().ok_or((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"NO_PACK","message":"no pack configured"}})),
        ))?
    };
    Ok(root)
}

pub(super) fn resolve_pack_dir_for_docs(
    state: &AppState,
    path: Option<&str>,
    pack_override: Option<&str>,
) -> Result<PathBuf, (StatusCode, Json<Value>)> {
    if let Some(p) = pack_override {
        return resolve_strict_local_pack_dir(p).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"PATH_INVALID","message":e.to_string()}})),
            )
        });
    }
    if let Some(path) = path {
        return resolve_strict_local_pack_dir(path).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"PATH_INVALID","message":e.to_string()}})),
            )
        });
    }
    state.packs.first().map(|r| pack_dir_for_path(r)).ok_or((
        StatusCode::BAD_REQUEST,
        Json(json!({"error":{"code":"NO_PACK","message":"no pack configured"}})),
    ))
}

pub(super) fn resolve_strict_local_pack_root(selector: &str) -> anyhow::Result<PathBuf> {
    let pack_root = resolve_pack_by_name_or_path(selector)?;
    if !has_manifest_at(&pack_root) {
        anyhow::bail!("no memory pack at {}", pack_root.display());
    }
    Ok(pack_root)
}

pub(super) fn resolve_strict_local_pack_dir(selector: &str) -> anyhow::Result<PathBuf> {
    let pack_root = resolve_strict_local_pack_root(selector)?;
    let pack_dir = resolve_pack_dir(&pack_root);
    if !pack_dir.join("manifest.json").exists() {
        anyhow::bail!("no memory pack at {}", pack_root.display());
    }
    Ok(pack_dir)
}

pub(super) fn ensure_pack_exists(pack_dir: &Path) -> anyhow::Result<()> {
    if pack_dir.join("manifest.json").exists() {
        return Ok(());
    }
    init_pack(pack_dir, false, "fastembed", "BAAI/bge-small-en-v1.5", 384)?;
    let pack_root = pack_dir.parent().unwrap_or(pack_dir).to_path_buf();
    let normalized = pack_root.canonicalize()?.to_string_lossy().to_string();
    let reg = load_registry().unwrap_or_default();
    ensure_registered(&normalized, None, reg.packs.is_empty())?;
    Ok(())
}
