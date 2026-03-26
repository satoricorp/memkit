use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::add_docs::run_add;
use crate::google::{
    self, fetch_doc_content, fetch_sheet_content, get_access_token, parse_doc_id, parse_sheet_ids,
    GoogleAuthenticator,
};
use crate::indexer::run_index;
use crate::file_tree::format_file_tree;
use crate::helix_store::{
    helix_graph_counts, helix_load_all_docs, helix_pack_path_for_local, helix_read_index_warnings,
    helix_try_cached_index_status, remove_helix_for_pack,
};
use crate::pack::{
    add_source_root, has_manifest_at, init_pack, load_manifest, remove_source_root,
    resolve_pack_dir, resolve_source_roots, scrub_pack_from_dir,
};
use crate::pack_location::PackLocation;
use crate::publish::{publish_pack_to_s3, PublishDestination};
use crate::registry::{pack_dir_for_path, ensure_registered, load_registry, remove_pack_by_path, resolve_pack_by_name_or_path};
use crate::query::{run_query, run_query_multi};
use crate::query_synth::{synthesize_answer_async, QueryProvider};
use crate::types::SourceDoc;

mod jobs;
use jobs::{JobRecord, JobRegistry, JobState, JobType};

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
    jobs: Arc<Mutex<JobRegistry>>,
    google: Option<Arc<GoogleAuthState>>,
    google_load_error: Option<String>,
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
    /// Pack path when adding documents/conversation. When adding a directory (no documents/conversation), this is the content path (directory or file to add).
    #[serde(default)]
    path: Option<String>,
    /// Pack override: pack root path or name. When adding a directory, which pack to add to (default: first pack or ~).
    #[serde(default)]
    pack: Option<String>,
    documents: Option<Vec<AddDocumentItem>>,
    conversation: Option<Vec<AddConversationMessage>>,
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
}

pub async fn run_server(packs: Vec<PathBuf>, host: String, port: u16) -> Result<()> {
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
    let state = AppState {
        packs: Arc::new(packs),
        jobs: Arc::new(Mutex::new(JobRegistry::new())),
        google,
        google_load_error,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/graph/view", get(graph_view))
        .route("/google/service-account-email", get(google_service_account_email))
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
        (StatusCode::NOT_FOUND, Json(json!({"error":{"code":"GOOGLE_NOT_CONFIGURED","message":msg}})))
    })?;
    Ok(Json(json!({ "email": google.client_email })))
}

async fn health(State(_state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    (
        StatusCode::OK,
        Json(HealthResponse {
            status: "ok",
            version: crate::term::BUILD_VERSION,
        }),
    )
}

fn job_targets_this_pack(
    j: &JobRecord,
    pack_root: Option<&str>,
    pack_dir: Option<&str>,
) -> bool {
    let Some(ref jp) = j.pack_path else {
        return false;
    };
    if Some(jp.as_str()) == pack_root {
        return true;
    }
    if Some(jp.as_str()) == pack_dir {
        return true;
    }
    false
}

fn job_is_index_work(j: &JobRecord) -> bool {
    matches!(j.job_type, JobType::IndexSources)
}

async fn status(
    State(state): State<AppState>,
    Query(q): Query<StatusQuery>,
) -> Json<Value> {
    let (
        pack_str,
        sources,
        vector_count,
        indexed,
        file_paths,
        pack_for_helix,
        source_root_paths,
        index_warnings,
    ) = if let Some(ref path) = q.path {
            // Resolve as pack name (registry) first, then as filesystem path.
            match resolve_pack_by_name_or_path(path) {
                Ok(pack_root) => {
                    // If path was already the pack dir (manifest.json here), use it; else pack_root is parent of .memkit.
                    let pack_dir = resolve_pack_dir(&pack_root);
                    let manifest = load_manifest(&pack_dir).ok();
                    let sources = manifest.as_ref().map(|m| m.sources.clone()).unwrap_or_default();
                    let source_root_paths: Vec<String> = manifest
                        .as_ref()
                        .map(|m| {
                            resolve_source_roots(&pack_dir, m)
                                .into_iter()
                                .map(|p| p.to_string_lossy().to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    let (vector_count, file_paths, indexed, index_warnings) =
                        if let Some((vc, mut fp, w)) = helix_try_cached_index_status(&pack_dir) {
                            fp.sort_unstable();
                            fp.dedup();
                            (vc, fp, vc > 0, w)
                        } else {
                            let docs = manifest
                                .as_ref()
                                .and_then(|m| load_pack_docs(&pack_dir, m.embedding.dimension).ok())
                                .unwrap_or_default();
                            let mut fp: Vec<String> =
                                docs.iter().map(|d| d.source_path.clone()).collect();
                            fp.sort_unstable();
                            fp.dedup();
                            let n = docs.len();
                            let iw = helix_read_index_warnings(&pack_dir);
                            (n, fp, n > 0, iw)
                        };
                    let display = if dirs::home_dir().as_ref().and_then(|h| h.canonicalize().ok()).as_ref()
                        == pack_root.canonicalize().as_ref().ok()
                    {
                        "~/.memkit".to_string()
                    } else {
                        pack_root.display().to_string()
                    };
                    (
                        display,
                        sources,
                        vector_count,
                        indexed,
                        file_paths,
                        Some(pack_dir),
                        source_root_paths,
                        index_warnings,
                    )
                }
                Err(_) => {
                    let dir = PathBuf::from(path)
                        .canonicalize()
                        .unwrap_or_else(|_| PathBuf::from(path));
                    let pack = resolve_pack_dir(&dir);
                    let manifest = load_manifest(&pack).ok();
                    let sources = manifest.as_ref().map(|m| m.sources.clone()).unwrap_or_default();
                    let source_root_paths: Vec<String> = manifest
                        .as_ref()
                        .map(|m| {
                            resolve_source_roots(&pack, m)
                                .into_iter()
                                .map(|p| p.to_string_lossy().to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    let (vector_count, file_paths, indexed, index_warnings) =
                        if let Some((vc, mut fp, w)) = helix_try_cached_index_status(&pack) {
                            fp.sort_unstable();
                            fp.dedup();
                            (vc, fp, vc > 0, w)
                        } else {
                            let docs = manifest
                                .as_ref()
                                .and_then(|m| load_pack_docs(&pack, m.embedding.dimension).ok())
                                .unwrap_or_default();
                            let mut fp: Vec<String> =
                                docs.iter().map(|d| d.source_path.clone()).collect();
                            fp.sort_unstable();
                            fp.dedup();
                            let n = docs.len();
                            let iw = helix_read_index_warnings(&pack);
                            (n, fp, n > 0, iw)
                        };
                    (
                        pack.display().to_string(),
                        sources,
                        vector_count,
                        indexed,
                        file_paths,
                        Some(pack),
                        source_root_paths,
                        index_warnings,
                    )
                }
            }
        } else {
            let mut all_sources = Vec::new();
            let mut all_paths = Vec::new();
            let mut total_vectors = 0usize;
            for pack_root in state.packs.iter() {
                let pack_dir = pack_dir_for_path(pack_root);
                if let Ok(m) = load_manifest(&pack_dir) {
                    all_sources.extend(m.sources);
                }
                if let Some((n, paths, _)) = helix_try_cached_index_status(&pack_dir) {
                    total_vectors += n;
                    all_paths.extend(paths);
                } else if let Ok(m) = load_manifest(&pack_dir) {
                    if let Ok(docs) = load_pack_docs(&pack_dir, m.embedding.dimension) {
                        total_vectors += docs.len();
                        all_paths.extend(docs.iter().map(|d| d.source_path.clone()));
                    }
                }
            }
            let pack_str = if state.packs.len() == 1 {
                let root = &state.packs[0];
                let is_home = dirs::home_dir()
                    .as_ref()
                    .and_then(|h| h.canonicalize().ok())
                    .as_ref()
                    == root.canonicalize().as_ref().ok();
                if is_home {
                    "~/.memkit".to_string()
                } else {
                    root.display().to_string()
                }
            } else {
                format!("{} packs", state.packs.len())
            };
            let pack_for_helix = state.packs.first().map(|r| pack_dir_for_path(r));
            (
                pack_str,
                all_sources,
                total_vectors,
                total_vectors > 0,
                all_paths,
                pack_for_helix,
                Vec::<String>::new(),
                Vec::<String>::new(),
            )
        };

    let (entities, relationships) = pack_for_helix
        .as_ref()
        .map(|p| helix_graph_counts(p.as_path()))
        .unwrap_or((0, 0));
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
            .map(|j| {
                let mut v = json!({
                    "id": j.id,
                    "job_type": j.job_type,
                    "pack_path": j.pack_path,
                    "state": j.state,
                });
                if let Some(ref s) = j.indexing_sources {
                    v["indexing_sources"] = json!(s);
                }
                v
            })
            .collect();
        (active, last, queued_list)
    };

    let pack_root_opt = pack_for_helix.as_ref().and_then(|pack_dir| {
        pack_dir
            .parent()
            .map(PathBuf::from)
            .or_else(|| Some(pack_dir.clone()))
            .and_then(|p| p.canonicalize().ok())
            .map(|p| p.to_string_lossy().to_string())
    });
    let pack_dir_opt = pack_for_helix
        .as_ref()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.to_string_lossy().to_string());
    let pr = pack_root_opt.as_deref();
    let pd = pack_dir_opt.as_deref();

    let (pending_removal, pending_add) = if q.path.is_some() {
        let path_matches_pack = |p: &str| Some(p) == pr || Some(p) == pd;
        let active_remove = active_job.as_ref().map_or(false, |j| {
            matches!(j.job_type, JobType::RemovePack)
                && j.pack_path.as_deref().map(path_matches_pack).unwrap_or(false)
        });
        let queued_remove = queued_list.iter().any(|j| {
            j.get("job_type").and_then(Value::as_str) == Some("remove_pack")
                && j.get("pack_path")
                    .and_then(Value::as_str)
                    .map(path_matches_pack)
                    .unwrap_or(false)
        });
        let active_add = active_job.as_ref().map_or(false, |j| {
            job_is_index_work(j) && job_targets_this_pack(j, pr, pd)
        });
        let queued_add = queued_list.iter().any(|j| {
            let jt = j.get("job_type").and_then(Value::as_str);
            let is_index = matches!(
                jt,
                Some("index_sources") | Some("index_new_pack") | Some("add_documents")
            );
            is_index
                && j.get("pack_path")
                    .and_then(Value::as_str)
                    .map(path_matches_pack)
                    .unwrap_or(false)
        });
        (active_remove || queued_remove, active_add || queued_add)
    } else {
        (false, false)
    };

    let pack_indexing_busy = if q.path.is_some() {
        let active_busy = active_job.as_ref().map_or(false, |j| {
            job_is_index_work(j) && job_targets_this_pack(j, pr, pd)
        });
        let queued_busy = queued_list.iter().any(|j| {
            let jt = j.get("job_type").and_then(Value::as_str);
            matches!(
                jt,
                Some("index_sources") | Some("index_new_pack") | Some("add_documents")
            ) && j.get("pack_path")
                .and_then(Value::as_str)
                .map(|p| Some(p) == pr || Some(p) == pd)
                .unwrap_or(false)
        });
        active_busy || queued_busy
    } else {
        false
    };

    let active_for_this_pack = if q.path.is_some() {
        active_job
            .as_ref()
            .filter(|j| job_targets_this_pack(j, pr, pd))
            .cloned()
    } else {
        None
    };
    let queued_jobs_for_this_pack: Vec<Value> = if q.path.is_some() {
        queued_list
            .iter()
            .filter(|j| {
                j.get("pack_path")
                    .and_then(Value::as_str)
                    .map(|p| Some(p) == pr || Some(p) == pd)
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    let pack_paths: Vec<String> = state.packs.iter().map(|p| {
        let is_home = dirs::home_dir().as_ref().and_then(|h| h.canonicalize().ok()).as_ref() == p.canonicalize().as_ref().ok();
        if is_home { "~/.memkit".to_string() } else { p.display().to_string() }
    }).collect();
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
        "source_root_paths": source_root_paths,
        "index_warnings": index_warnings,
        "pending_removal": pending_removal,
        "pending_add": pending_add,
        "pack_indexing_busy": pack_indexing_busy,
        "jobs": {
            "active": active_job,
            "active_for_this_pack": active_for_this_pack,
            "last_completed": last_job,
            "queued": queued_list.len(),
            "queued_jobs": queued_list,
            "queued_jobs_for_this_pack": queued_jobs_for_this_pack
        }
    }))
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

/// Query flow: (1) Retrieval: run_query() loads pack docs, embeds the query, and runs vector search
/// (Helix: helix_hybrid_query). Returns QueryResponse with results (top chunks).
/// (2) Synthesis: unless req.raw, synthesize_answer_async calls OpenAI chat/completions. Use ?raw=true or --raw to skip synthesis.
async fn query(
    State(state): State<AppState>,
    Json(req): Json<QueryRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let resp_result = if let Some(ref path) = req.pack {
        if path.starts_with("s3://") {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"INVALID_PACK","message":"S3 pack paths are not supported in this build"}})),
            ));
        }
        let dir = PathBuf::from(path);
        let pack = if has_manifest_at(&dir) {
            resolve_pack_dir(&dir)
        } else {
            state.packs.first().cloned().unwrap_or(dir)
        };
        let loc = PackLocation::local(pack);
        run_query(
            &loc,
            &req.query,
            req.top_k,
            req.use_reranker,
            req.path_filter.as_deref(),
        )
    } else if state.packs.len() > 1 {
        let pack_dirs: Vec<PathBuf> = state.packs.iter().map(|r| pack_dir_for_path(r.as_path())).collect();
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
                Ok(Json(json!(resp)))
            } else {
                match synthesize_answer_async(&req.query, &resp).await {
                    Ok((answer, provider)) => {
                        let model = match &provider {
                            QueryProvider::OpenAI(m) => m.clone(),
                            QueryProvider::None => String::new(),
                        };
                        Ok(Json(json!({
                            "answer": answer,
                            "sources": sources,
                            "provider": provider.label(),
                            "model": model,
                            "retrieval_results": resp.retrieval_results
                        })))
                    }
                    Err(e) => {
                        Ok(Json(json!({
                            "answer": serde_json::Value::Null,
                            "synthesis_error": e.to_string(),
                            "sources": sources,
                            "results": resp.results,
                            "retrieval_results": resp.retrieval_results
                        })))
                    }
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
        resolve_pack_dir(&dir)
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
            let Some(rest) = uri.strip_prefix("s3://") else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"code":"INVALID_DESTINATION","message":"destination must be s3://bucket/prefix"}})),
                ));
            };
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

/// Resolve pack root for "add directory": pack override or first pack. Used when we may create the pack.
fn resolve_pack_root_for_add(
    state: &AppState,
    pack_override: Option<&str>,
) -> Result<PathBuf, (StatusCode, Json<Value>)> {
    let root = if let Some(p) = pack_override {
        PathBuf::from(p).canonicalize().map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"PATH_INVALID","message":format!("pack path not accessible: {}", e)}})),
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
        let root = PathBuf::from(p)
            .canonicalize()
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"code":"PATH_INVALID","message":format!("pack path not accessible: {}", e)}})),
                )
            })?;
        return Ok(resolve_pack_dir(&root));
    }
    if let Some(path) = path {
        let dir = PathBuf::from(path).canonicalize().map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":{"code":"PATH_INVALID","message":format!("path not accessible: {}", e)}})),
            )
        })?;
        let resolved = resolve_pack_dir(&dir);
        return Ok(resolved);
    }
    state.packs.first().map(|r| pack_dir_for_path(r)).ok_or((
        StatusCode::BAD_REQUEST,
        Json(json!({"error":{"code":"NO_PACK","message":"no pack configured"}})),
    ))
}

/// Create pack at pack_dir if manifest.json does not exist.
fn ensure_pack_exists(pack_dir: &Path) -> anyhow::Result<()> {
    if pack_dir.join("manifest.json").exists() {
        return Ok(());
    }
    init_pack(pack_dir, false, "fastembed", "BAAI/bge-small-en-v1.5", 384)?;
    let pack_root = pack_dir
        .parent()
        .unwrap_or_else(|| pack_dir)
        .to_path_buf();
    let normalized = pack_root.canonicalize()?.to_string_lossy().to_string();
    let reg = load_registry().unwrap_or_default();
    ensure_registered(&normalized, None, reg.packs.is_empty())?;
    Ok(())
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
    indexing_sources: Option<Vec<String>>,
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
        indexing_sources,
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
        indexing_sources: None,
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
                _ => {
                    // IndexSources or any other job type treated as index (pack_path from job or all packs).
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
                    let mut all_warnings: Vec<String> = Vec::new();
                    let _multi = packs_to_index.len() > 1;
                    for pack in &packs_to_index {
                        let manifest = load_manifest(pack)?;
                        let sources = resolve_source_roots(pack, &manifest);
                        let (scanned, updated, chunks, warnings) =
                            run_index(pack, &sources)?;
                        total_scanned += scanned;
                        total_updated += updated;
                        total_chunks += chunks;
                        all_warnings.extend(warnings);
                    }
                    Ok(json!({
                        "scanned": total_scanned,
                        "updated_files": total_updated,
                        "chunks": total_chunks,
                        "warnings": all_warnings
                    }))
                })
                .await;
                match run_result {
                    Ok(Ok(v)) => Ok((v, cleanup_after_index)),
                    Ok(Err(e)) => Err((e, cleanup_after_index)),
                    Err(e) => Err((anyhow::anyhow!("job task failed: {}", e), cleanup_after_index)),
                }
            }
            JobWork::RemovePack { pack_root } => {
                let run_result = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
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
    let has_content = req.documents.as_ref().map_or(false, |d| !d.is_empty())
        || req.conversation.as_ref().map_or(false, |c| !c.is_empty());

    if !has_content {
        // Add directory (or file) mode: path = content to add, pack = optional pack override.
        let content_path = req.path.as_deref().ok_or((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"PATH_REQUIRED","message":"path required to add a directory or file"}})),
        ))?;
        let content = PathBuf::from(content_path)
            .canonicalize()
            .map_err(|e| {
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
        let pack_path_str = pack_dir.canonicalize().unwrap_or(pack_dir.clone()).to_string_lossy().to_string();
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

    // Documents/conversation mode: path (or pack) = pack location.
    let pack_dir = resolve_pack_dir_for_docs(&state, req.path.as_deref(), req.pack.as_deref())?;
    ensure_pack_exists(&pack_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"INIT_FAILED","message":e.to_string()}})),
        )
    })?;

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

    let pack_path = pack_dir.clone();
    let items_clone: Vec<(String, String)> = items
        .iter()
        .map(|(c, s)| (c.clone(), s.clone()))
        .collect();
    let run_result = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let mut total_chunks = 0usize;
        for (content, source_path) in &items_clone {
            let n = run_add(&pack_path, content, source_path)?;
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
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": { "code": "ADD_FAILED", "message": msg }
                })),
            ))
        }
        Err(e) => {
            let msg = e.to_string();
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": { "code": "ADD_TASK_FAILED", "message": msg }
                })),
            ))
        }
    }
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
            "serverInfo": {"name":"memkit","version": crate::term::BUILD_VERSION},
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

                    let pack_dirs: Vec<PathBuf> = state.packs.iter().map(|r| pack_dir_for_path(r.as_path())).collect();
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
                    let pack_path_display = state.packs.first().map(|p| {
                        let is_home = dirs::home_dir().as_ref().and_then(|h| h.canonicalize().ok()).as_ref() == p.canonicalize().as_ref().ok();
                        if is_home { "~/.memkit".to_string() } else { p.display().to_string() }
                    }).unwrap_or_default();
                    json!({
                        "content":[{"type":"text","text":json!({
                            "status":"ok",
                            "pack_path": pack_path_display,
                            "pack_paths": state.packs.iter().map(|p| {
                                let is_home = dirs::home_dir().as_ref().and_then(|h| h.canonicalize().ok()).as_ref() == p.canonicalize().as_ref().ok();
                                if is_home { "~/.memkit".to_string() } else { p.display().to_string() }
                            }).collect::<Vec<_>>()
                        }).to_string()}]
                    })
                },
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
