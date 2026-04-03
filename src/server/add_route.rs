use std::path::PathBuf;

use anyhow::Result;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::add_docs::{run_add, run_add_conversations};
use crate::conversation::{ConversationSessionInput, ConversationTurn};
use crate::google::{
    fetch_doc_content, fetch_sheet_content, get_access_token, parse_doc_id, parse_sheet_ids,
};

use super::{
    AppState, add_source_root, ensure_pack_exists, enqueue_index_job, pack_dir_for_path,
    resolve_pack_dir_for_docs, resolve_pack_root_for_add, start_next_job_if_idle,
};

#[derive(Deserialize)]
struct AddDocumentItem {
    #[serde(rename = "type")]
    doc_type: String,
    value: String,
}

#[derive(Deserialize)]
struct AddConversationMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AddConversationSession {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    session_time: Option<String>,
    conversation: Vec<AddConversationMessage>,
}

#[derive(Deserialize, Default)]
pub(super) struct AddRequest {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    pack: Option<String>,
    documents: Option<Vec<AddDocumentItem>>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    session_time: Option<String>,
    conversation: Option<Vec<AddConversationMessage>>,
    #[serde(default)]
    conversations: Option<Vec<AddConversationSession>>,
}

pub(super) async fn add_now(
    State(state): State<AppState>,
    Json(req): Json<AddRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let has_content = req.documents.as_ref().map_or(false, |d| !d.is_empty())
        || req.conversation.as_ref().map_or(false, |c| !c.is_empty())
        || req.conversations.as_ref().map_or(false, |c| !c.is_empty());

    if !has_content {
        let content_path = req.path.as_deref().ok_or((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"PATH_REQUIRED","message":"path required to add a directory or file"}})),
        ))?;
        let content = PathBuf::from(content_path).canonicalize().map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"PATH_INVALID","message":format!("path not accessible: {}", e)}})),
            )
        })?;
        let is_home = dirs::home_dir()
            .as_ref()
            .and_then(|h| h.canonicalize().ok())
            .as_ref()
            == Some(&content);
        if is_home {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": {
                        "code": "ADD_HOME_REFUSED",
                        "message": "Cannot add home directory as a source. Add specific directories (e.g. ~/Documents/...) instead."
                    }
                })),
            ));
        }
        let root_path = if content.is_dir() {
            content.to_string_lossy().to_string()
        } else {
            content
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| content.to_string_lossy().to_string())
        };
        let pack_root = resolve_pack_root_for_add(&state, req.pack.as_deref())?;
        let pack_dir = pack_dir_for_path(&pack_root);
        ensure_pack_exists(&pack_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":{"code":"INIT_FAILED","message":e.to_string()}})),
            )
        })?;
        add_source_root(&pack_dir, &root_path).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":{"code":"ADD_SOURCE_FAILED","message":e.to_string()}})),
            )
        })?;
        let pack_path_str = pack_dir
            .canonicalize()
            .unwrap_or(pack_dir.clone())
            .to_string_lossy()
            .to_string();
        let job = enqueue_index_job(
            &state,
            "add",
            Some(pack_path_str),
            None,
            Some(vec![root_path.clone()]),
        )
        .await;
        start_next_job_if_idle(state.clone());
        return Ok(Json(json!({
            "status": "accepted",
            "job": job
        })));
    }

    let pack_dir = resolve_pack_dir_for_docs(&state, req.path.as_deref(), req.pack.as_deref())?;
    ensure_pack_exists(&pack_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"INIT_FAILED","message":e.to_string()}})),
        )
    })?;

    let mut items: Vec<(String, String)> = Vec::new();

    if let Some(docs) = &req.documents {
        for item in docs {
            match item.doc_type.as_str() {
                "url" => {
                    let client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(30))
                        .build()
                        .map_err(|e| {
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(json!({"error":{"code":"HTTP_CLIENT","message":e.to_string()}})),
                            )
                        })?;
                    let resp = client.get(&item.value).send().await.map_err(|e| {
                        (
                            StatusCode::BAD_REQUEST,
                            Json(json!({"error":{"code":"FETCH_FAILED","message":e.to_string()}})),
                        )
                    })?;
                    let content = resp.text().await.map_err(|e| {
                        (
                            StatusCode::BAD_REQUEST,
                            Json(json!({"error":{"code":"FETCH_FAILED","message":e.to_string()}})),
                        )
                    })?;
                    let source_path = format!("memkit://add/{}", Utc::now().timestamp_millis());
                    items.push((content, source_path));
                }
                "content" => {
                    let source_path = format!("memkit://add/{}", Utc::now().timestamp_millis());
                    items.push((item.value.clone(), source_path));
                }
                "google_doc" => {
                    let msg = state
                        .google_load_error
                        .as_deref()
                        .map(|e| format!("Google integration not configured: {}", e))
                        .unwrap_or_else(|| "Google integration not configured".to_string());
                    let google = state.google.as_ref().ok_or_else(|| {
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({"error":{"code":"GOOGLE_NOT_CONFIGURED","message":msg}})),
                        )
                    })?;
                    let doc_id = parse_doc_id(&item.value).ok_or_else(|| {
                        (
                            StatusCode::BAD_REQUEST,
                            Json(json!({"error":{"code":"INVALID_GOOGLE_DOC","message":"invalid Google Doc URL or ID"}})),
                        )
                    })?;
                    let token = get_access_token(google.auth.as_ref()).await.map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error":{"code":"GOOGLE_TOKEN","message":e.to_string()}})),
                        )
                    })?;
                    let (content, source_path) = fetch_doc_content(&doc_id, &token)
                        .await
                        .map_err(|e| {
                            (
                                StatusCode::BAD_REQUEST,
                                Json(json!({"error":{"code":"GOOGLE_FETCH","message":e.to_string()}})),
                            )
                        })?;
                    items.push((content, source_path));
                }
                "google_sheet" => {
                    let msg = state
                        .google_load_error
                        .as_deref()
                        .map(|e| format!("Google integration not configured: {}", e))
                        .unwrap_or_else(|| "Google integration not configured".to_string());
                    let google = state.google.as_ref().ok_or_else(|| {
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({"error":{"code":"GOOGLE_NOT_CONFIGURED","message":msg}})),
                        )
                    })?;
                    let (spreadsheet_id, gid) = parse_sheet_ids(&item.value).ok_or_else(|| {
                        (
                            StatusCode::BAD_REQUEST,
                            Json(json!({"error":{"code":"INVALID_GOOGLE_SHEET","message":"invalid Google Sheet URL or ID"}})),
                        )
                    })?;
                    let token = get_access_token(google.auth.as_ref()).await.map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error":{"code":"GOOGLE_TOKEN","message":e.to_string()}})),
                        )
                    })?;
                    let pairs = fetch_sheet_content(&spreadsheet_id, gid, &token)
                        .await
                        .map_err(|e| {
                            (
                                StatusCode::BAD_REQUEST,
                                Json(json!({"error":{"code":"GOOGLE_FETCH","message":e.to_string()}})),
                            )
                        })?;
                    items.extend(pairs);
                }
                _ => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error":{"code":"INVALID_TYPE","message":"document type must be url, content, google_doc, or google_sheet"}})),
                    ));
                }
            }
        }
    }

    let mut conversation_sessions = Vec::new();
    if let Some(conv) = &req.conversation {
        conversation_sessions.push(ConversationSessionInput {
            session_id: req.session_id.clone(),
            session_time: req.session_time.clone(),
            conversation: conv
                .iter()
                .map(|m| ConversationTurn {
                    role: m.role.clone(),
                    content: m.content.clone(),
                })
                .collect(),
        });
    }
    if let Some(conv_batches) = &req.conversations {
        for session in conv_batches {
            conversation_sessions.push(ConversationSessionInput {
                session_id: session.session_id.clone(),
                session_time: session.session_time.clone(),
                conversation: session
                    .conversation
                    .iter()
                    .map(|m| ConversationTurn {
                        role: m.role.clone(),
                        content: m.content.clone(),
                    })
                    .collect(),
            });
        }
    }

    if items.is_empty() && conversation_sessions.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"EMPTY_ADD","message":"documents or conversation required"}})),
        ));
    }

    let add_document_count = items.len();
    let add_conversation_count = conversation_sessions.len();
    let add_session_ids = conversation_sessions
        .iter()
        .take(8)
        .map(|session| {
            session
                .session_id
                .as_deref()
                .unwrap_or("<missing-session-id>")
                .to_string()
        })
        .collect::<Vec<_>>()
        .join(", ");
    let pack_path = pack_dir.clone();
    let items_clone: Vec<(String, String)> =
        items.iter().map(|(c, s)| (c.clone(), s.clone())).collect();
    let run_result = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let mut total_chunks = 0usize;
        for (content, source_path) in &items_clone {
            let n = run_add(&pack_path, content, source_path)?;
            total_chunks += n;
        }
        if !conversation_sessions.is_empty() {
            let n = run_add_conversations(&pack_path, &conversation_sessions)?;
            total_chunks += n;
        }
        Ok(json!({
            "status": "ok",
            "chunks_added": total_chunks
        }))
    })
    .await;

    match run_result {
        Ok(Ok(v)) => Ok(Json(json!({
            "status": "ok",
            "result": v
        }))),
        Ok(Err(e)) => {
            let msg = e.to_string();
            crate::term::warn(format!(
                "warning: /add failed for pack {} (documents={}, conversation_sessions={}): {}",
                pack_dir.display(),
                add_document_count,
                add_conversation_count,
                msg
            ));
            if add_conversation_count > 0 {
                crate::term::warn(format!(
                    "warning: /add failed session ids (showing up to 8): {}",
                    add_session_ids
                ));
            }
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": { "code": "ADD_FAILED", "message": msg }
                })),
            ))
        }
        Err(e) => {
            let msg = e.to_string();
            crate::term::warn(format!(
                "warning: /add task failed for pack {} (documents={}, conversation_sessions={}): {}",
                pack_dir.display(),
                add_document_count,
                add_conversation_count,
                msg
            ));
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": { "code": "ADD_TASK_FAILED", "message": msg }
                })),
            ))
        }
    }
}
