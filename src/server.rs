use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::add_docs::run_add;
use crate::google::{
    self, fetch_doc_content, fetch_sheet_content, get_access_token, parse_doc_id, parse_sheet_ids,
    GoogleAuthenticator,
};
#[cfg(feature = "lance-falkor")]
use crate::falkor_store::{
    graph_counts as falkor_graph_counts, graph_name_for_pack, graph_name_from_env,
    graph_schema as falkor_graph_schema, graph_subgraph as falkor_graph_subgraph,
};
use crate::indexer::run_index;
use crate::file_tree::format_file_tree;
#[cfg(feature = "lance-falkor")]
use crate::lancedb_store::load_all_docs;
#[cfg(feature = "store-helix-only")]
use crate::helix_store::{
    helix_load_all_docs, helix_graph_counts, helix_pack_path_for_local, remove_helix_for_pack,
};
use crate::pack::{
    add_source_root, copy_dir_into_sources, init_pack, load_manifest, remove_source_root,
    resolve_source_roots, scrub_pack_from_dir,
};
use crate::pack_location::PackLocation;
use crate::publish::{publish_pack_to_s3, PublishDestination};
use crate::memkit_txt::ensure_memkit_txt;
use crate::registry::{pack_dir_for_path, ensure_registered, remove_pack_by_path};
use crate::query::{run_query, run_query_multi};
use crate::query_synth::synthesize_answer;
use crate::types::SourceDoc;

fn load_pack_docs(pack: &Path, dim: usize) -> anyhow::Result<Vec<SourceDoc>> {
    #[cfg(feature = "store-helix-only")]
    return helix_load_all_docs(&helix_pack_path_for_local(pack), dim);
    #[cfg(feature = "lance-falkor")]
    return load_all_docs(pack, dim);
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
    falkordb_socket: Option<String>,
    falkor_graph: String,
    jobs: Arc<Mutex<JobRegistry>>,
    google: Option<Arc<GoogleAuthState>>,
    google_load_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum JobType {
    IndexSources,
    AddDocuments,
    RemovePack,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
struct JobRecord {
    id: String,
    job_type: JobType,
    state: JobState,
    trigger: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pack_path: Option<String>,
    /// (temp_path_to_remove, pack_path) for iCloud: remove source root and delete temp after index
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup_after_index: Option<(String, String)>,
    /// For AddDocuments: { "pack_path": string, "items": [ { "content": string, "source_path": string } ] }
    #[serde(skip_serializing_if = "Option::is_none")]
    add_payload: Option<Value>,
    enqueued_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    result: Option<Value>,
    error: Option<String>,
}

struct JobRegistry {
    jobs: Vec<JobRecord>,
    queue: std::collections::VecDeque<String>,
    running: Option<String>,
    next_id: u64,
}

impl JobRegistry {
    fn new() -> Self {
        Self {
            jobs: Vec::new(),
            queue: std::collections::VecDeque::new(),
            running: None,
            next_id: 1,
        }
    }

    fn trim_history(&mut self, keep_last: usize) {
        if self.jobs.len() <= keep_last {
            return;
        }
        let drop_n = self.jobs.len() - keep_last;
        self.jobs.drain(0..drop_n);
    }

    fn find_mut(&mut self, id: &str) -> Option<&mut JobRecord> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    fn find(&self, id: &str) -> Option<&JobRecord> {
        self.jobs.iter().find(|j| j.id == id)
    }
}

#[derive(Deserialize)]
struct QueryRequest {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
    #[serde(default = "default_use_reranker")]
    use_reranker: bool,
    #[serde(default)]
    raw: bool,
    pack: Option<String>,
    #[serde(default)]
    path_filter: Option<String>,
}

#[derive(Deserialize, Default)]
struct StatusQuery {
    path: Option<String>,
}

#[derive(Deserialize, Default)]
struct IndexRequest {
    path: Option<String>,
    /// Optional pack name for registry (default: random word).
    name: Option<String>,
}

#[derive(Deserialize, Default)]
struct RemoveRequest {
    path: Option<String>,
}

#[derive(Deserialize, Default)]
struct PublishRequest {
    /// Pack path (directory that contains the pack, or path to .memkit). If omitted, use first pack.
    path: Option<String>,
    /// Full S3 destination (e.g. s3://bucket/prefix/). If omitted, use memkit bucket with tenant keys.
    destination: Option<String>,
}

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

#[derive(Deserialize, Default)]
struct AddRequest {
    /// Pack path (directory that contains the pack, or path to .memkit). If omitted, use first pack.
    #[serde(default)]
    path: Option<String>,
    documents: Option<Vec<AddDocumentItem>>,
    conversation: Option<Vec<AddConversationMessage>>,
}

#[derive(Deserialize)]
struct SubgraphRequest {
    query: String,
    #[serde(default = "default_depth")]
    depth: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_top_k() -> usize {
    8
}

fn default_use_reranker() -> bool {
    true
}

fn default_depth() -> usize {
    2
}

fn default_limit() -> usize {
    25
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    falkordb_socket: Option<String>,
    falkordb_connected: Option<bool>,
}

pub async fn run_server(
    packs: Vec<PathBuf>,
    host: String,
    port: u16,
    falkordb_socket: Option<String>,
) -> Result<()> {
    if packs.is_empty() {
        anyhow::bail!("at least one pack path required");
    }
    let (google, google_load_error) = match google::load_service_account_key().await {
        Ok(key) => {
            let client_email = google::service_account_email_from_key(&key).to_string();
            match google::build_google_authenticator(key).await {
                Ok(auth) => (Some(Arc::new(GoogleAuthState { auth: Arc::new(auth), client_email })), None),
                Err(e) => (None, Some(e.to_string())),
            }
        }
        Err(e) => (None, Some(e.to_string())),
    };
    // #region agent log
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/Users/joe/git/local/.cursor/debug-085e7a.log") {
        let err_preview = google_load_error.as_deref().map(|s| if s.len() > 200 { format!("{}...", &s[..200]) } else { s.to_string() }).unwrap_or_default();
        let data = serde_json::json!({
            "sessionId": "085e7a",
            "location": "server.rs:run_server after google load",
            "message": "google state",
            "data": { "google_ok": google.is_some(), "google_load_error_preview": err_preview },
            "timestamp": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
            "hypothesisId": "H1 H4 H5"
        });
        let _ = std::io::Write::write_fmt(&mut f, format_args!("{}\n", data.to_string()));
    }
    // #endregion
    let state = AppState {
        packs: Arc::new(packs),
        falkordb_socket,
        falkor_graph: {
            #[cfg(feature = "lance-falkor")]
            {
                graph_name_from_env()
            }
            #[cfg(feature = "store-helix-only")]
            {
                String::new()
            }
        },
        jobs: Arc::new(Mutex::new(JobRegistry::new())),
        google,
        google_load_error,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/graph/view", get(graph_view))
        .merge({
            #[cfg(feature = "lance-falkor")]
            {
                Router::new()
                    .route("/graph/schema", get(graph_schema))
                    .route("/graph/subgraph", post(graph_subgraph))
            }
            #[cfg(feature = "store-helix-only")]
            {
                Router::new()
            }
        })
        .route("/google/service-account-email", get(google_service_account_email))
        .route("/query", post(query))
        .route("/index", post(index_now))
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
        (StatusCode::NOT_FOUND, Json(json!({"error":{"code":"GOOGLE_NOT_CONFIGURED","message":msg}})))
    })?;
    Ok(Json(json!({ "email": google.client_email })))
}

async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    let socket_path = state.falkordb_socket.clone();
    let connected = if let Some(ref path) = socket_path {
        // Timeout so /health never blocks: UnixStream::connect can block if FalkorDB is not running.
        let path = path.to_string();
        let r = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tokio::task::spawn_blocking(move || can_connect_to_socket(&path)),
        )
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or(false);
        Some(r)
    } else {
        None
    };
    let ok = connected.unwrap_or(true);
    let status = if ok { "ok" } else { "degraded" }.to_string();
    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        code,
        Json(HealthResponse {
            status,
            falkordb_socket: socket_path,
            falkordb_connected: connected,
        }),
    )
}

#[cfg(unix)]
fn can_connect_to_socket(path: &str) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
fn can_connect_to_socket(_path: &str) -> bool {
    false
}

async fn status(
    State(state): State<AppState>,
    Query(q): Query<StatusQuery>,
) -> Json<Value> {
    let (pack_str, sources, vector_count, indexed, file_paths, pack_for_helix) =
        if let Some(ref path) = q.path {
            let dir = PathBuf::from(path)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(path));
            // Per-dir layout: index creates <dir>/.memkit and writes vectors to helix under that pack path.
            // Always use dir.join(".memkit") so we read from the same place the indexer wrote.
            let pack = if dir.join(".memkit/manifest.json").exists() {
                dir.join(".memkit")
            } else if dir.join("manifest.json").exists() {
                dir.clone()
            } else {
                dir.join(".memkit")
            };
            let manifest = load_manifest(&pack).ok();
            let docs = manifest
                .as_ref()
                .and_then(|m| load_pack_docs(&pack, m.embedding.dimension).ok())
                .unwrap_or_default();
            let vector_count = docs.len();
            let indexed = vector_count > 0;
            let mut file_paths: Vec<String> = docs.iter().map(|d| d.source_path.clone()).collect();
            file_paths.sort_unstable();
            file_paths.dedup();
            let sources = manifest.map(|m| m.sources).unwrap_or_default();
            (
                pack.display().to_string(),
                sources,
                vector_count,
                indexed,
                file_paths,
                Some(pack),
            )
        } else {
            let mut all_sources = Vec::new();
            let mut all_paths = Vec::new();
            let mut total_vectors = 0usize;
            for pack in state.packs.iter() {
                if let Ok(m) = load_manifest(pack) {
                    all_sources.extend(m.sources);
                }
                if let Ok(m) = load_manifest(pack) {
                    if let Ok(docs) = load_pack_docs(pack, m.embedding.dimension) {
                        total_vectors += docs.len();
                        all_paths.extend(docs.iter().map(|d| d.source_path.clone()));
                    }
                }
            }
            let pack_str = if state.packs.len() == 1 {
                state.packs[0].display().to_string()
            } else {
                format!("{} packs", state.packs.len())
            };
            (
                pack_str,
                all_sources,
                total_vectors,
                total_vectors > 0,
                all_paths,
                state.packs.first().cloned(),
            )
        };

    let (entities, relationships) = {
        #[cfg(feature = "lance-falkor")]
        {
            state
                .falkordb_socket
                .as_ref()
                .and_then(|sock| falkor_graph_counts(sock, &state.falkor_graph).ok())
                .unwrap_or((0, 0))
        }
        #[cfg(feature = "store-helix-only")]
        {
            pack_for_helix
                .as_ref()
                .map(|p| helix_graph_counts(p.as_path()))
                .unwrap_or((0, 0))
        }
    };
    let base_path: String = state
        .packs
        .first()
        .and_then(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .filter(|n| *n == ".memkit")
                .and_then(|_| p.parent())
                .map(|pa| pa.display().to_string())
        })
        .unwrap_or_else(|| pack_str.clone());
    let file_tree = format_file_tree(&file_paths, &base_path);

    let (active_job, last_job, queued_list) = {
        let jobs = state.jobs.lock().await;
        let active = jobs
            .running
            .as_ref()
            .and_then(|id| jobs.find(id).cloned());
        let last = jobs
            .jobs
            .iter()
            .rev()
            .find(|j| !matches!(j.state, JobState::Queued | JobState::Running))
            .cloned();
        let queued_list: Vec<Value> = jobs
            .queue
            .iter()
            .filter_map(|id| jobs.find(id))
            .map(|j| json!({ "id": j.id, "job_type": j.job_type, "pack_path": j.pack_path, "state": j.state }))
            .collect();
        (active, last, queued_list)
    };

    let pack_paths: Vec<String> = state.packs.iter().map(|p| p.display().to_string()).collect();
    Json(json!({
        "status": "ok",
        "pack_path": pack_str,
        "pack_paths": pack_paths,
        "indexed": indexed,
        "vector_count": vector_count,
        "entities": entities,
        "relationships": relationships,
        "file_tree": file_tree,
        "sources": sources,
        "jobs": {
            "active": active_job,
            "last_completed": last_job,
            "queued": queued_list.len(),
            "queued_jobs": queued_list
        }
    }))
}

#[cfg(feature = "lance-falkor")]
async fn graph_schema(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(socket_path) = state.falkordb_socket.clone() else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error":{"code":"FALKOR_UNAVAILABLE","message":"FALKORDB_SOCKET is not configured"}}),
            ),
        ));
    };

    match falkor_graph_schema(&socket_path, &state.falkor_graph) {
        Ok(schema) => Ok(Json(schema)),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"GRAPH_SCHEMA_FAILED","message":e.to_string()}})),
        )),
    }
}

#[cfg(feature = "lance-falkor")]
async fn graph_subgraph(
    State(state): State<AppState>,
    Json(req): Json<SubgraphRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(socket_path) = state.falkordb_socket.clone() else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error":{"code":"FALKOR_UNAVAILABLE","message":"FALKORDB_SOCKET is not configured"}}),
            ),
        ));
    };

    match falkor_graph_subgraph(
        &socket_path,
        &state.falkor_graph,
        &req.query,
        req.depth,
        req.limit,
    ) {
        Ok(graph) => Ok(Json(graph)),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"GRAPH_SUBGRAPH_FAILED","message":e.to_string()}})),
        )),
    }
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

async fn query(
    State(state): State<AppState>,
    Json(req): Json<QueryRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let resp_result = if let Some(ref path) = req.pack {
        let loc = if path.starts_with("s3://") {
            PackLocation::from_s3_uri(path).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"code":"INVALID_PACK","message":e.to_string()}})),
                )
            })?
        } else {
            let dir = PathBuf::from(path);
            let pack = if dir.join(".memkit/manifest.json").exists() {
                dir.join(".memkit")
            } else if dir.join("manifest.json").exists() {
                dir
            } else {
                state.packs.first().cloned().unwrap_or_else(|| dir)
            };
            PackLocation::local(pack)
        };
        #[cfg(feature = "lance-falkor")]
        let graph_name = loc.as_path().and_then(|p| graph_name_for_pack(p).ok());
        #[cfg(feature = "store-helix-only")]
        let graph_name: Option<String> = None;
        run_query(
            &loc,
            &req.query,
            req.top_k,
            req.use_reranker,
            graph_name.as_deref(),
            req.path_filter.as_deref(),
        )
    } else if state.packs.len() > 1 {
        run_query_multi(
            &state.packs,
            &req.query,
            req.top_k,
            req.use_reranker,
            req.path_filter.as_deref(),
        )
    } else {
        let pack = state.packs.first().unwrap();
        run_query(
            &PackLocation::local(pack),
            &req.query,
            req.top_k,
            req.use_reranker,
            None,
            req.path_filter.as_deref(),
        )
    };

    match resp_result {
        Ok(resp) => {
            if req.raw {
                Ok(Json(json!(resp)))
            } else {
                match synthesize_answer(&req.query, &resp) {
                    Ok((answer, provider)) => {
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
                        Ok(Json(json!({
                            "answer": answer,
                            "sources": sources,
                            "provider": provider.label()
                        })))
                    }
                    Err(e) => Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error":{"code":"SYNTHESIS_FAILED","message":e.to_string()}})),
                    )),
                }
            }
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"QUERY_FAILED","message":e.to_string()}})),
        )),
    }
}

async fn publish(
    State(state): State<AppState>,
    Json(req): Json<PublishRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let pack_dir = if let Some(ref path) = req.path {
        let dir = PathBuf::from(path)
            .canonicalize()
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"code":"PATH_INVALID","message":format!("path not accessible: {}", e)}})),
                )
            })?;
        if dir.join("manifest.json").exists() {
            dir
        } else if dir.join(".memkit/manifest.json").exists() {
            dir.join(".memkit")
        } else {
            pack_dir_for_path(&dir)
        }
    } else {
        state
            .packs
            .first()
            .cloned()
            .ok_or((
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"NO_PACK","message":"no pack path and no default pack"}})),
            ))?
    };
    if !pack_dir.join("manifest.json").exists() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"PACK_INVALID","message":"manifest.json not found"}})),
        ));
    }
    let destination = match &req.destination {
        Some(uri) => {
            let uri = uri.trim();
            if !uri.starts_with("s3://") {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"code":"INVALID_DESTINATION","message":"destination must be s3://bucket/prefix"}})),
                ));
            }
            let rest = uri.strip_prefix("s3://").unwrap();
            let (bucket, prefix) = match rest.find('/') {
                Some(i) => (
                    rest[..i].to_string(),
                    rest[i + 1..].trim_end_matches('/').to_string(),
                ),
                None => (rest.to_string(), String::new()),
            };
            if bucket.is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"code":"INVALID_DESTINATION","message":"empty bucket"}})),
                ));
            }
            PublishDestination::UserBucket { bucket, prefix }
        }
        None => PublishDestination::MemkitBucket,
    };
    match publish_pack_to_s3(&pack_dir, destination).await {
        Ok(uri) => Ok(Json(json!({ "uri": uri, "status": "ok" }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"PUBLISH_FAILED","message":e.to_string()}})),
        )),
    }
}

async fn index_now(
    State(state): State<AppState>,
    Json(req): Json<IndexRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if let Some(path) = req.path {
        use std::ffi::OsStr;
        // Validate path and pack so we can fail fast; heavy work (copy) runs in background.
        let dir = PathBuf::from(&path)
            .canonicalize()
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"code":"PATH_INVALID","message":format!("path not accessible: {}", e)}})),
                )
            })?;
        let pack_dir = pack_dir_for_path(&dir);
        // When path is home directory: only enqueue index job for the pack at ~/.memkit (no copy, no new source).
        let is_home = dirs::home_dir()
            .as_ref()
            .and_then(|h| h.canonicalize().ok())
            .as_ref()
            == Some(&dir);
        if is_home {
            if !pack_dir.join("manifest.json").exists() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": {
                            "code": "PACK_NOT_FOUND",
                            "message": "No pack at ~/.memkit. Run 'mk add <path>' first to create the default pack."
                        }
                    })),
                ));
            }
            let job = enqueue_index_job(
                &state,
                "manual_index",
                Some(pack_dir.to_string_lossy().to_string()),
                None,
            )
            .await;
            start_next_job_if_idle(state.clone());
            return Ok(Json(json!({
                "status": "accepted",
                "job": job
            })));
        }
        if !pack_dir.join("manifest.json").exists() {
            init_pack(&pack_dir, false, "fastembed", "BAAI/bge-small-en-v1.5", 384)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error":{"code":"INIT_FAILED","message":e.to_string()}})),
                    )
                })?;
        }
        let path = dir.to_string_lossy().to_string();
        let name = req.name.clone();
        let state = state.clone();
        tokio::spawn(async move {
            let dir = PathBuf::from(&path);
            let pack_dir = pack_dir_for_path(&dir);
            let normalized = path.clone();
            let _ = ensure_memkit_txt(&dir);
            let source_name = dir
                .file_name()
                .unwrap_or_else(|| OsStr::new("unnamed"))
                .to_string_lossy();
            let outcome = match copy_dir_into_sources(&dir, &pack_dir, &source_name) {
                Ok(o) => o,
                Err(_) => return,
            };
            if add_source_root(&pack_dir, &outcome.source_root).is_err() {
                return;
            }
            let reg = crate::registry::load_registry().unwrap_or_default();
            let is_first = reg.packs.is_empty();
            let _ = ensure_registered(&normalized, name, is_first);
            let cleanup = outcome.cleanup_after_index.as_ref().map(|p| {
                (p.to_string_lossy().to_string(), pack_dir.to_string_lossy().to_string())
            });
            let pack_to_index = Some(pack_dir.to_string_lossy().to_string());
            let job = enqueue_index_job(&state, "manual_index", pack_to_index, cleanup).await;
            start_next_job_if_idle(state);
            let _ = job;
        });
        return Ok(Json(json!({
            "status": "accepted",
            "job": { "id": "pending" }
        })));
    }

    let job = enqueue_index_job(&state, "manual_index", None, None).await;
    start_next_job_if_idle(state.clone());
    Ok(Json(json!({
        "status":"accepted",
        "job": job
    })))
}

async fn remove_now(
    State(state): State<AppState>,
    Json(req): Json<RemoveRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let path = req
        .path
        .as_deref()
        .ok_or((
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
    let pack_dir = if dir.join(".memkit/manifest.json").exists() {
        dir.join(".memkit")
    } else if dir.join("manifest.json").exists() {
        dir
    } else {
        pack_dir_for_path(&dir)
    };
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
        .unwrap_or_else(|| pack_dir.as_path())
        .to_path_buf();
    let job = enqueue_remove_job(&state, pack_root.to_string_lossy().to_string()).await;
    start_next_job_if_idle(state.clone());
    Ok(Json(json!({
        "status": "accepted",
        "job": job
    })))
}

async fn enqueue_index_job(
    state: &AppState,
    trigger: &str,
    pack_path: Option<String>,
    cleanup_after_index: Option<(String, String)>,
) -> Value {
    let mut jobs = state.jobs.lock().await;
    let id = format!("job-{}", jobs.next_id);
    jobs.next_id += 1;
    let record = JobRecord {
        id: id.clone(),
        job_type: JobType::IndexSources,
        state: JobState::Queued,
        trigger: trigger.to_string(),
        pack_path: pack_path.clone(),
        cleanup_after_index: cleanup_after_index.clone(),
        add_payload: None,
        enqueued_at: Utc::now(),
        started_at: None,
        finished_at: None,
        result: None,
        error: None,
    };
    jobs.queue.push_back(id);
    jobs.jobs.push(record.clone());
    json!(record)
}

async fn enqueue_remove_job(state: &AppState, pack_root: String) -> Value {
    let mut jobs = state.jobs.lock().await;
    let id = format!("job-{}", jobs.next_id);
    jobs.next_id += 1;
    let record = JobRecord {
        id: id.clone(),
        job_type: JobType::RemovePack,
        state: JobState::Queued,
        trigger: "manual_remove".to_string(),
        pack_path: Some(pack_root),
        cleanup_after_index: None,
        add_payload: None,
        enqueued_at: Utc::now(),
        started_at: None,
        finished_at: None,
        result: None,
        error: None,
    };
    jobs.queue.push_back(id);
    jobs.jobs.push(record.clone());
    json!(record)
}

fn start_next_job_if_idle(state: AppState) {
    tokio::spawn(async move {
        enum JobWork {
            Index {
                packs: Vec<PathBuf>,
                cleanup: Option<(String, String)>,
            },
            Add {
                pack_path: PathBuf,
                items: Vec<(String, String)>,
            },
            RemovePack { pack_root: PathBuf },
        }
        let (maybe_job_id, work) = {
            let mut jobs = state.jobs.lock().await;
            if jobs.running.is_some() {
                return;
            }
            let Some(id) = jobs.queue.pop_front() else {
                return;
            };
            let job = jobs.find(&id).cloned();
            jobs.running = Some(id.clone());
            if let Some(ref mut job) = jobs.find_mut(&id) {
                job.state = JobState::Running;
                job.started_at = Some(Utc::now());
            }
            let work = match job.as_ref() {
                Some(j) if matches!(j.job_type, JobType::RemovePack) => {
                    let pack_root = j
                        .pack_path
                        .as_ref()
                        .map(PathBuf::from)
                        .unwrap_or_else(PathBuf::new);
                    JobWork::RemovePack { pack_root }
                }
                Some(j) if matches!(j.job_type, JobType::AddDocuments) => {
                    let pack_path = j.pack_path.as_ref().map(PathBuf::from).unwrap_or_else(PathBuf::new);
                    let items: Vec<(String, String)> = j
                        .add_payload
                        .as_ref()
                        .and_then(|p| p.get("items").and_then(Value::as_array))
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|o| {
                                    let c = o.get("content").and_then(Value::as_str).unwrap_or("").to_string();
                                    let s = o.get("source_path").and_then(Value::as_str).unwrap_or("").to_string();
                                    Some((c, s))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    JobWork::Add { pack_path, items }
                }
                _ => {
                    let pack_path = job.as_ref().and_then(|j| j.pack_path.clone());
                    let cleanup = job.as_ref().and_then(|j| j.cleanup_after_index.clone());
                    let packs: Vec<PathBuf> = pack_path
                        .map(|p| vec![PathBuf::from(p)])
                        .unwrap_or_else(|| state.packs.iter().cloned().collect());
                    JobWork::Index {
                        packs,
                        cleanup,
                    }
                }
            };
            (id, work)
        };

        let run_outcome: Result<(Value, Option<(String, String)>), (anyhow::Error, Option<(String, String)>)> = match work {
            JobWork::Index { packs: packs_to_index, cleanup: cleanup_after_index } => {
                let run_result = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
                    let mut total_scanned = 0usize;
                    let mut total_updated = 0usize;
                    let mut total_chunks = 0usize;
                    let multi = packs_to_index.len() > 1;
                    for pack in &packs_to_index {
                        let manifest = load_manifest(pack)?;
                        let sources = resolve_source_roots(pack, &manifest);
                        #[cfg(feature = "lance-falkor")]
                        let graph_name = if multi { graph_name_for_pack(pack).ok() } else { None };
                        #[cfg(feature = "store-helix-only")]
                        let graph_name: Option<String> = None;
                        let (scanned, updated, chunks) =
                            run_index(pack, &sources, graph_name.as_deref())?;
                        total_scanned += scanned;
                        total_updated += updated;
                        total_chunks += chunks;
                    }
                    Ok(json!({
                        "scanned": total_scanned,
                        "updated_files": total_updated,
                        "chunks": total_chunks
                    }))
                })
                .await;
                match run_result {
                    Ok(Ok(v)) => Ok((v, cleanup_after_index)),
                    Ok(Err(e)) => Err((e, cleanup_after_index)),
                    Err(e) => Err((anyhow::anyhow!("job task failed: {}", e), cleanup_after_index)),
                }
            }
            JobWork::Add { pack_path, items } => {
                let run_result = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
                    let mut total_chunks = 0usize;
                    for (content, source_path) in &items {
                        let chunks = run_add(&pack_path, content, source_path)?;
                        total_chunks += chunks;
                    }
                    Ok(json!({
                        "status": "ok",
                        "chunks_added": total_chunks
                    }))
                })
                .await;
                match run_result {
                    Ok(Ok(v)) => Ok((v, None)),
                    Ok(Err(e)) => Err((e, None)),
                    Err(e) => Err((anyhow::anyhow!("job task failed: {}", e), None)),
                }
            }
            JobWork::RemovePack { pack_root } => {
                let run_result = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
                    #[cfg(feature = "store-helix-only")]
                    remove_helix_for_pack(&pack_root)?;
                    remove_pack_by_path(&pack_root)?;
                    scrub_pack_from_dir(&pack_root)?;
                    Ok(json!({ "status": "removed" }))
                })
                .await;
                match run_result {
                    Ok(Ok(v)) => Ok((v, None)),
                    Ok(Err(e)) => Err((e, None)),
                    Err(e) => Err((anyhow::anyhow!("job task failed: {}", e), None)),
                }
            }
        };

        let (state_value, result_value, error_value, cleanup_after_index) = match run_outcome {
            Ok((v, cleanup)) => (JobState::Succeeded, Some(v), None, cleanup),
            Err((e, cleanup)) => (
                JobState::Failed,
                None,
                Some(e.to_string()),
                cleanup,
            ),
        };

        let mut jobs = state.jobs.lock().await;
        let finished_at = Utc::now();
        if let Some(job) = jobs.find_mut(&maybe_job_id) {
            job.state = state_value;
            job.result = result_value;
            job.error = error_value;
            job.finished_at = Some(finished_at);
        }
        jobs.running = None;
        jobs.trim_history(100);
        drop(jobs);

        if let Some((temp_path, pack_path)) = cleanup_after_index {
            let pack = PathBuf::from(&pack_path);
            let _ = remove_source_root(&pack, &temp_path);
            let _ = std::fs::remove_dir_all(&temp_path);
        }

        start_next_job_if_idle(state.clone());
    });
}

async fn add_now(
    State(state): State<AppState>,
    Json(req): Json<AddRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let pack_dir = if let Some(ref path) = req.path {
        let dir = PathBuf::from(path)
            .canonicalize()
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"code":"PATH_INVALID","message":format!("path not accessible: {}", e)}})),
                )
            })?;
        let resolved = if dir.join("manifest.json").exists() {
            dir
        } else if dir.join(".memkit/manifest.json").exists() {
            dir.join(".memkit")
        } else {
            pack_dir_for_path(&dir)
        };
        if !resolved.join("manifest.json").exists() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"PACK_INVALID","message":"manifest.json not found"}})),
            ));
        }
        resolved
    } else {
        state
            .packs
            .first()
            .cloned()
            .ok_or((
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"NO_PACK","message":"no pack configured"}})),
            ))?
    };

    let mut items: Vec<(String, String)> = Vec::new(); // (content, source_path)

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
                    let token = get_access_token(google.auth.as_ref())
                        .await
                        .map_err(|e| {
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
                    let token = get_access_token(google.auth.as_ref())
                        .await
                        .map_err(|e| {
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

    if let Some(conv) = &req.conversation {
        let text: String = conv
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n\n");
        let source_path = format!("memkit://add/{}", Utc::now().timestamp_millis());
        items.push((text, source_path));
    }

    if items.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"EMPTY_ADD","message":"documents or conversation required"}})),
        ));
    }

    let pack_path = pack_dir.display().to_string();
    let add_payload = json!({
        "pack_path": pack_path,
        "items": items
            .iter()
            .map(|(content, source_path)| json!({ "content": content, "source_path": source_path }))
            .collect::<Vec<_>>()
    });
    let job = enqueue_add_job(&state, &pack_path, add_payload).await;
    start_next_job_if_idle(state.clone());
    Ok(Json(json!({
        "status": "accepted",
        "job": job
    })))
}

async fn enqueue_add_job(state: &AppState, pack_path: &str, add_payload: Value) -> Value {
    let mut jobs = state.jobs.lock().await;
    let id = format!("job-{}", jobs.next_id);
    jobs.next_id += 1;
    let record = JobRecord {
        id: id.clone(),
        job_type: JobType::AddDocuments,
        state: JobState::Queued,
        trigger: "add".to_string(),
        pack_path: Some(pack_path.to_string()),
        cleanup_after_index: None,
        add_payload: Some(add_payload),
        enqueued_at: Utc::now(),
        started_at: None,
        finished_at: None,
        result: None,
        error: None,
    };
    jobs.queue.push_back(id);
    jobs.jobs.push(record.clone());
    json!(record)
}

async fn mcp(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let method = payload.get("method").and_then(Value::as_str).unwrap_or("");
    let id = payload.get("id").cloned().unwrap_or(json!(null));

    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name":"memkit","version":"0.1.0"},
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
                    let use_reranker = args.get("use_reranker").and_then(Value::as_bool).unwrap_or(true);

                    let resp = if state.packs.len() > 1 {
                        run_query_multi(&state.packs, &query, top_k, use_reranker, None)
                    } else {
                        let p = state.packs.first().unwrap();
                        run_query(&PackLocation::local(p), &query, top_k, use_reranker, None, None)
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
                "memory_status" => json!({
                    "content":[{"type":"text","text":json!({
                        "status":"ok",
                        "pack_path": state.packs.first().map(|p| p.display().to_string()).unwrap_or_default(),
                        "pack_paths": state.packs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()
                    }).to_string()}]
                }),
                "memory_sources" => {
                    let mut all_sources = Vec::new();
                    for pack in state.packs.iter() {
                        if let Ok(m) = load_manifest(pack) {
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
