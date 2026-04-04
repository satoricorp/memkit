use axum::Json;
use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use chrono::Utc;
use serde_json::{Value, json};

use crate::cloud::{
    CloudCurrentPointer, CloudPackMetadata, ensure_cloud_pack_dirs, parse_cloud_pack_uri,
    read_current_pointer, read_pack_metadata, write_json_atomically,
};
use crate::publish::{sha256_for_path, unpack_cloud_publish_archive};

use super::{
    AppState, authenticated_cloud_context, authorize_cloud_uri, header_string,
    temp_upload_limit_bytes, unique_temp_upload_path,
};

pub(super) async fn publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let raw_pack_uri = header_string(&headers, "x-memkit-pack-uri").ok_or((
        StatusCode::BAD_REQUEST,
        Json(json!({"error":{"code":"PACK_URI_REQUIRED","message":"x-memkit-pack-uri header required"}})),
    ))?;
    let pack_uri = parse_cloud_pack_uri(&raw_pack_uri).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"INVALID_PACK_URI","message":e.to_string()}})),
        )
    })?;
    let auth = authenticated_cloud_context(&headers).await?;
    authorize_cloud_uri(&auth, &pack_uri)?;

    let overwrite = header_string(&headers, "x-memkit-overwrite")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);
    let expected_sha = header_string(&headers, "x-memkit-sha256");
    let display_name = header_string(&headers, "x-memkit-pack-name");

    let bytes = to_bytes(body, temp_upload_limit_bytes())
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"UPLOAD_READ_FAILED","message":e.to_string()}})),
            )
        })?;

    let upload_path = unique_temp_upload_path("publish");
    if let Some(parent) = upload_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":{"code":"UPLOAD_PREPARE_FAILED","message":format!("failed to create {}: {}", parent.display(), e)}})),
            )
        })?;
    }
    tokio::fs::write(&upload_path, &bytes).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"UPLOAD_WRITE_FAILED","message":format!("failed to write {}: {}", upload_path.display(), e)}})),
        )
    })?;
    drop(bytes);

    let upload_path_for_hash = upload_path.clone();
    let (computed_sha, size_bytes) =
        tokio::task::spawn_blocking(move || sha256_for_path(&upload_path_for_hash))
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error":{"code":"UPLOAD_HASH_FAILED","message":e.to_string()}})),
                )
            })?
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error":{"code":"UPLOAD_HASH_FAILED","message":e.to_string()}})),
                )
            })?;

    if let Some(ref expected_sha) = expected_sha
        && expected_sha != &computed_sha
    {
        let _ = tokio::fs::remove_file(&upload_path).await;
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error":{"code":"CHECKSUM_MISMATCH","message":"uploaded artifact checksum mismatch"}}),
            ),
        ));
    }

    ensure_cloud_pack_dirs(&pack_uri, &state.cloud_root).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"PACK_PREPARE_FAILED","message":e.to_string()}})),
        )
    })?;

    let current = read_current_pointer(&pack_uri, &state.cloud_root).ok();
    if let Some(ref current) = current {
        if current.sha256 == computed_sha {
            let _ = tokio::fs::remove_file(&upload_path).await;
            return Ok(Json(json!({
                "status": "ok",
                "pack_uri": pack_uri.to_string(),
                "revision": current.revision,
                "sha256": current.sha256,
                "size_bytes": size_bytes,
                "unchanged": true,
            })));
        }
        if !overwrite {
            let _ = tokio::fs::remove_file(&upload_path).await;
            return Err((
                StatusCode::CONFLICT,
                Json(json!({
                    "error": {
                        "code": "PUBLISH_CONFLICT",
                        "message": "cloud pack already exists with different content; retry with overwrite=true"
                    },
                    "current_revision": current.revision,
                    "current_sha256": current.sha256,
                })),
            ));
        }
    }

    let revision_id = format!("rev-{}", uuid::Uuid::new_v4());
    let revision_root = pack_uri.revision_root(&state.cloud_root, &revision_id);
    let upload_path_for_unpack = upload_path.clone();
    let revision_root_for_unpack = revision_root.clone();
    let unpacked = tokio::task::spawn_blocking(move || {
        unpack_cloud_publish_archive(&upload_path_for_unpack, &revision_root_for_unpack)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"PUBLISH_UNPACK_FAILED","message":e.to_string()}})),
        )
    })?;
    let unpacked = match unpacked {
        Ok(artifact) => artifact,
        Err(e) => {
            let _ = tokio::fs::remove_file(&upload_path).await;
            let _ = tokio::fs::remove_dir_all(&revision_root).await;
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"PUBLISH_INVALID_ARTIFACT","message":e.to_string()}})),
            ));
        }
    };
    if unpacked.manifest.pack_id != pack_uri.pack_id {
        let _ = tokio::fs::remove_file(&upload_path).await;
        let _ = tokio::fs::remove_dir_all(&revision_root).await;
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "code": "PACK_ID_MISMATCH",
                    "message": format!(
                        "artifact pack_id {} does not match publish target {}",
                        unpacked.manifest.pack_id,
                        pack_uri
                    )
                }
            })),
        ));
    }

    let now = Utc::now();
    let mut metadata =
        read_pack_metadata(&pack_uri, &state.cloud_root).unwrap_or(CloudPackMetadata {
            pack_uri: pack_uri.to_string(),
            pack_id: pack_uri.pack_id.clone(),
            tenant_type: pack_uri.tenant_kind,
            tenant_id: pack_uri.tenant_id.clone(),
            display_name: None,
            source_pack_id: None,
            current_revision: None,
            created_at: now,
            updated_at: now,
        });
    metadata.current_revision = Some(revision_id.clone());
    metadata.display_name = display_name
        .clone()
        .or(metadata.display_name)
        .or_else(|| Some(pack_uri.display_name()));
    metadata.source_pack_id = None;
    metadata.updated_at = now;

    let current_pointer = CloudCurrentPointer {
        revision: revision_id.clone(),
        sha256: computed_sha.clone(),
        published_at: now,
    };
    write_json_atomically(&pack_uri.pack_json_path(&state.cloud_root), &metadata).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"PACK_METADATA_WRITE_FAILED","message":e.to_string()}})),
        )
    })?;
    write_json_atomically(&pack_uri.current_path(&state.cloud_root), &current_pointer).map_err(
        |e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":{"code":"CURRENT_POINTER_WRITE_FAILED","message":e.to_string()}})),
            )
        },
    )?;

    let _ = tokio::fs::remove_file(&upload_path).await;
    Ok(Json(json!({
        "status": "ok",
        "pack_uri": pack_uri.to_string(),
        "revision": revision_id,
        "sha256": computed_sha,
        "size_bytes": size_bytes,
        "manifest_path": unpacked.manifest_path,
        "helix_path": unpacked.helix_path,
    })))
}
