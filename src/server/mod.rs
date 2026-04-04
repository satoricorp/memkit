use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::cloud::{
    CloudPackSummary, CloudPackUri, CloudTenantKind, cloud_root, parse_cloud_pack_uri,
    read_current_pointer, summarize_cloud_pack,
};
use crate::google::{self, GoogleAuthenticator};
use crate::helix_store::{helix_load_all_docs, helix_pack_path_for_local};
use crate::pack::{add_source_root, has_manifest_at, init_pack, resolve_pack_dir};
use crate::pack_location::PackLocation;
use crate::registry::{
    ensure_registered, load_registry, pack_dir_for_path, resolve_pack_by_name_or_path,
};
use crate::types::SourceDoc;

mod add_route;
mod jobs;
mod mcp_route;
mod publish_route;
mod query_route;
mod status_route;
use add_route::add_now;
use jobs::{
    JobRecord, JobRegistry, JobState, JobType, enqueue_index_job, enqueue_remove_job,
    job_is_index_work, job_targets_this_pack, start_next_job_if_idle,
};
use mcp_route::mcp;
use publish_route::publish;
use query_route::query;
use status_route::status;

fn load_pack_docs(pack: &Path, dim: usize) -> anyhow::Result<Vec<SourceDoc>> {
    helix_load_all_docs(&helix_pack_path_for_local(pack), dim)
}

/// Optional Google integration (service account). When set, google_doc / google_sheet document types are supported.
#[derive(Clone)]
struct GoogleAuthState {
    auth: Arc<GoogleAuthenticator>,
    client_email: String,
}

#[derive(Clone)]
struct AppState {
    packs: Arc<Vec<PathBuf>>,
    cloud_root: Arc<PathBuf>,
    jobs: Arc<Mutex<JobRegistry>>,
    google: Option<Arc<GoogleAuthState>>,
    google_load_error: Option<String>,
}

#[derive(Deserialize, Default)]
struct RemoveRequest {
    path: Option<String>,
}

fn default_top_k() -> usize {
    8
}

fn default_use_reranker() -> bool {
    true
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_sha: Option<&'static str>,
}

pub async fn run_server(packs: Vec<PathBuf>, host: String, port: u16) -> Result<()> {
    let (google, google_load_error) = match google::load_service_account_key().await {
        Ok(key) => {
            let client_email = google::service_account_email_from_key(&key).to_string();
            match google::build_google_authenticator(key).await {
                Ok(auth) => (
                    Some(Arc::new(GoogleAuthState {
                        auth: Arc::new(auth),
                        client_email,
                    })),
                    None,
                ),
                Err(e) => (None, Some(e.to_string())),
            }
        }
        Err(e) => (None, Some(e.to_string())),
    };
    let state = AppState {
        packs: Arc::new(packs),
        cloud_root: Arc::new(cloud_root()),
        jobs: Arc::new(Mutex::new(JobRegistry::new())),
        google,
        google_load_error,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/graph/view", get(graph_view))
        .route(
            "/google/service-account-email",
            get(google_service_account_email),
        )
        .route("/packs", get(list_cloud_packs))
        .route("/query", post(query))
        .route("/remove", post(remove_now))
        .route("/add", post(add_now))
        .route("/publish", post(publish))
        .route("/mcp", post(mcp))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn google_service_account_email(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let msg = state
        .google_load_error
        .as_deref()
        .map(|e| format!("Google integration not configured: {}", e))
        .unwrap_or_else(|| "Google integration not configured".to_string());
    let google = state.google.as_ref().ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error":{"code":"GOOGLE_NOT_CONFIGURED","message":msg}})),
        )
    })?;
    Ok(Json(json!({ "email": google.client_email })))
}

async fn health(State(_state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    (
        StatusCode::OK,
        Json(HealthResponse {
            status: "ok",
            version: crate::term::release_version(),
            git_sha: crate::term::git_sha(),
        }),
    )
}

#[derive(Clone, Default)]
struct CloudAuthContext {
    user_id: Option<String>,
    org_id: Option<String>,
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

async fn authenticated_cloud_context(
    headers: &HeaderMap,
) -> Result<CloudAuthContext, (StatusCode, Json<Value>)> {
    let session_token = bearer_token(headers).ok_or((
        StatusCode::UNAUTHORIZED,
        Json(json!({"error":{"code":"AUTH_REQUIRED","message":"Bearer session token required"}})),
    ))?;

    let profile = crate::auth::authenticate_cloud_session(&session_token)
        .await
        .map_err(|err| match err {
            crate::auth::CloudSessionAuthError::Unauthorized(message) => (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":{"code":"INVALID_SESSION","message":message}})),
            ),
            crate::auth::CloudSessionAuthError::Misconfigured(message) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":{"code":"AUTH_BACKEND_NOT_CONFIGURED","message":message}})),
            ),
            crate::auth::CloudSessionAuthError::Backend(message) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error":{"code":"AUTH_BACKEND_FAILED","message":message}})),
            ),
        })?;

    let auth = CloudAuthContext {
        user_id: profile.user_id.map(|v| v.to_string()),
        org_id: profile.org_id.map(|v| v.to_string()),
    };
    if auth.user_id.is_none() && auth.org_id.is_none() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(
                json!({"error":{"code":"AUTH_PROFILE_INCOMPLETE","message":"authenticated session is missing user/org identity"}}),
            ),
        ));
    }
    Ok(auth)
}

fn authorize_cloud_uri(
    auth: &CloudAuthContext,
    pack_uri: &CloudPackUri,
) -> Result<(), (StatusCode, Json<Value>)> {
    match pack_uri.tenant_kind {
        CloudTenantKind::Users => {
            if let Some(ref user_id) = auth.user_id
                && user_id != &pack_uri.tenant_id
            {
                return Err((
                    StatusCode::FORBIDDEN,
                    Json(
                        json!({"error":{"code":"PACK_FORBIDDEN","message":"cloud pack does not belong to the current user"}}),
                    ),
                ));
            }
        }
        CloudTenantKind::Orgs => {
            if let Some(ref org_id) = auth.org_id
                && org_id != &pack_uri.tenant_id
            {
                return Err((
                    StatusCode::FORBIDDEN,
                    Json(
                        json!({"error":{"code":"PACK_FORBIDDEN","message":"cloud pack does not belong to the current org"}}),
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn discover_cloud_packs_for_tenant(
    cloud_root: &Path,
    tenant_kind: CloudTenantKind,
    tenant_id: &str,
) -> Result<Vec<CloudPackSummary>, (StatusCode, Json<Value>)> {
    let tenant_root = cloud_root
        .join("packs")
        .join(tenant_kind.as_str())
        .join(tenant_id);
    if !tenant_root.exists() {
        return Ok(Vec::new());
    }
    let read_dir = std::fs::read_dir(&tenant_root).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"PACK_LIST_FAILED","message":format!("failed to read {}: {}", tenant_root.display(), e)}})),
        )
    })?;
    let mut packs = Vec::new();
    for entry in read_dir {
        let entry = entry.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":{"code":"PACK_LIST_FAILED","message":e.to_string()}})),
            )
        })?;
        let file_type = entry.file_type().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":{"code":"PACK_LIST_FAILED","message":e.to_string()}})),
            )
        })?;
        if !file_type.is_dir() {
            continue;
        }
        let pack_id = entry.file_name().to_string_lossy().to_string();
        let uri = CloudPackUri {
            tenant_kind,
            tenant_id: tenant_id.to_string(),
            pack_id,
        };
        if let Ok(summary) = summarize_cloud_pack(&uri, cloud_root) {
            packs.push(summary);
        }
    }
    packs.sort_by(|a, b| a.pack_id.cmp(&b.pack_id));
    Ok(packs)
}

async fn list_cloud_packs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let auth = authenticated_cloud_context(&headers).await?;
    let mut packs = Vec::new();
    if let Some(ref user_id) = auth.user_id {
        packs.extend(discover_cloud_packs_for_tenant(
            &state.cloud_root,
            CloudTenantKind::Users,
            user_id,
        )?);
    }
    if let Some(ref org_id) = auth.org_id {
        packs.extend(discover_cloud_packs_for_tenant(
            &state.cloud_root,
            CloudTenantKind::Orgs,
            org_id,
        )?);
    }
    Ok(Json(json!({ "packs": packs })))
}

fn temp_upload_root() -> PathBuf {
    std::env::var("MEMKIT_UPLOAD_TMP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("memkit-uploads"))
}

fn temp_upload_limit_bytes() -> usize {
    std::env::var("MEMKIT_PUBLISH_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2 * 1024 * 1024 * 1024)
}

fn unique_temp_upload_path(prefix: &str) -> PathBuf {
    temp_upload_root().join(format!(
        "{}-{}-{}.tar.gz",
        prefix,
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ))
}

fn resolve_cloud_query_location(
    state: &AppState,
    auth: &CloudAuthContext,
    raw_pack_uri: &str,
) -> Result<PackLocation, (StatusCode, Json<Value>)> {
    let pack_uri = parse_cloud_pack_uri(raw_pack_uri).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"INVALID_PACK_URI","message":e.to_string()}})),
        )
    })?;
    authorize_cloud_uri(auth, &pack_uri)?;
    let current = read_current_pointer(&pack_uri, &state.cloud_root).map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error":{"code":"PACK_NOT_PUBLISHED","message":e.to_string()}})),
        )
    })?;
    let revision_root = pack_uri.revision_root(&state.cloud_root, &current.revision);
    if !revision_root.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(
                json!({"error":{"code":"REVISION_NOT_FOUND","message":format!("missing revision {}", current.revision)}}),
            ),
        ));
    }
    Ok(PackLocation::cloud(revision_root))
}

async fn graph_view() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8" />
  <title>Memkit Graph View</title>
  <style>
    body { font-family: ui-sans-serif, system-ui, sans-serif; margin: 0; background: #111; color: #f5f5f5; }
    #top { padding: 12px; display: flex; gap: 8px; align-items: center; }
    #q { width: 320px; }
    #out { width: 100vw; height: calc(100vh - 56px); }
    .node text { fill: #f5f5f5; font-size: 10px; }
    .link { stroke: #888; stroke-opacity: .7; }
  </style>
  <script src="https://cdn.jsdelivr.net/npm/d3@7"></script>
</head>
<body>
  <div id="top">
    <label>Query:</label>
    <input id="q" value="memory" />
    <button id="load">Load</button>
  </div>
  <svg id="out"></svg>
  <script>
    async function load() {
      const q = document.getElementById('q').value || 'memory';
      const resp = await fetch('/graph/subgraph', {
        method: 'POST',
        headers: {'content-type':'application/json'},
        body: JSON.stringify({query:q, depth:2, limit:25})
      });
      const data = await resp.json();
      render(data.nodes || [], data.edges || []);
    }
    function render(nodes, edges) {
      const svg = d3.select('#out');
      svg.selectAll('*').remove();
      const w = window.innerWidth, h = window.innerHeight - 56;
      svg.attr('viewBox', [0, 0, w, h]);
      const sim = d3.forceSimulation(nodes)
        .force('link', d3.forceLink(edges).id(d => d.id).distance(90))
        .force('charge', d3.forceManyBody().strength(-220))
        .force('center', d3.forceCenter(w / 2, h / 2));
      const link = svg.append('g').selectAll('line')
        .data(edges).enter().append('line').attr('class', 'link');
      const node = svg.append('g').selectAll('g')
        .data(nodes).enter().append('g').attr('class', 'node');
      node.append('circle')
        .attr('r', d => d.kind === 'Chunk' ? 9 : 6)
        .attr('fill', d => d.kind === 'Chunk' ? '#4ade80' : '#60a5fa');
      node.append('text').attr('x', 10).attr('y', 4).text(d => d.label || d.id);
      sim.on('tick', () => {
        link.attr('x1', d => d.source.x).attr('y1', d => d.source.y)
            .attr('x2', d => d.target.x).attr('y2', d => d.target.y);
        node.attr('transform', d => `translate(${d.x},${d.y})`);
      });
    }
    document.getElementById('load').addEventListener('click', load);
    load();
  </script>
</body>
</html>"#,
    )
}

/// Resolve pack root for "add directory": pack override or first pack. Used when we may create the pack.
fn resolve_pack_root_for_add(
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

/// Resolve pack dir for documents/conversation add: path (pack path) or pack override or first pack.
fn resolve_pack_dir_for_docs(
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

fn resolve_strict_local_pack_root(selector: &str) -> anyhow::Result<PathBuf> {
    let pack_root = resolve_pack_by_name_or_path(selector)?;
    if !has_manifest_at(&pack_root) {
        anyhow::bail!("no memory pack at {}", pack_root.display());
    }
    Ok(pack_root)
}

fn resolve_strict_local_pack_dir(selector: &str) -> anyhow::Result<PathBuf> {
    let pack_root = resolve_strict_local_pack_root(selector)?;
    let pack_dir = resolve_pack_dir(&pack_root);
    if !pack_dir.join("manifest.json").exists() {
        anyhow::bail!("no memory pack at {}", pack_root.display());
    }
    Ok(pack_dir)
}

/// Create pack at pack_dir if manifest.json does not exist.
fn ensure_pack_exists(pack_dir: &Path) -> anyhow::Result<()> {
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

async fn remove_now(
    State(state): State<AppState>,
    Json(req): Json<RemoveRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let path = req.path.as_deref().ok_or((
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": { "code": "PATH_REQUIRED", "message": "remove requires path" }
        })),
    ))?;
    let dir = PathBuf::from(path)
        .canonicalize()
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"PATH_INVALID","message":format!("path not accessible: {}", e)}})),
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
