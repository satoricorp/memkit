use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::falkor_store::{
    graph_name_from_env, graph_schema as falkor_graph_schema, graph_subgraph as falkor_graph_subgraph,
};
use crate::indexer::run_index;
use crate::ontology::OntologyEngine;
use crate::pack::{load_index, load_manifest, save_manifest};
use crate::query::run_query;
use crate::query_synth::synthesize_answer;
use crate::types::SourceConfig;

#[derive(Clone)]
struct AppState {
    pack: Arc<PathBuf>,
    falkordb_socket: Option<String>,
    falkor_graph: String,
    jobs: Arc<Mutex<JobRegistry>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum JobType {
    IndexSources,
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
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
    #[serde(default)]
    raw: bool,
}

#[derive(Deserialize)]
struct SourceRequest {
    path: String,
}

#[derive(Deserialize)]
struct SubgraphRequest {
    query: String,
    #[serde(default = "default_depth")]
    depth: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Deserialize)]
struct OntologySourceQuery {
    path: String,
}

fn default_mode() -> String {
    "hybrid".to_string()
}

fn default_top_k() -> usize {
    8
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
    pack: PathBuf,
    host: String,
    port: u16,
    falkordb_socket: Option<String>,
) -> Result<()> {
    let state = AppState {
        pack: Arc::new(pack),
        falkordb_socket,
        falkor_graph: graph_name_from_env(),
        jobs: Arc::new(Mutex::new(JobRegistry::new())),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/graph/schema", get(graph_schema))
        .route("/graph/subgraph", post(graph_subgraph))
        .route("/graph/view", get(graph_view))
        .route("/query", post(query))
        .route("/index", post(index_now))
        .route("/jobs", get(jobs_list))
        .route("/jobs/{id}", get(job_status))
        .route("/sources", get(sources_list).post(sources_add).delete(sources_remove))
        .route("/ontology/sources", get(ontology_sources))
        .route("/ontology/source", get(ontology_source))
        .route("/mcp", post(mcp))
        .with_state(state);

    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    let socket_path = state.falkordb_socket.clone();
    let connected = socket_path.as_deref().map(can_connect_to_socket);
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

async fn status(State(state): State<AppState>) -> Json<Value> {
    let manifest = load_manifest(&state.pack).ok();
    let indexed = load_index(&state.pack)
        .map(|idx| !idx.docs.is_empty())
        .unwrap_or(false);
    let (active_job, last_job, queued_jobs) = {
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
        (active, last, jobs.queue.len())
    };
    Json(json!({
        "status": "ok",
        "pack_path": state.pack.display().to_string(),
        "indexed": indexed,
        "sources": manifest.map(|m| m.sources).unwrap_or_default(),
        "jobs": {
            "active": active_job,
            "last_completed": last_job,
            "queued": queued_jobs
        }
    }))
}

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
  <title>Satori Graph View</title>
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
    match run_query(&state.pack, &req.query, &req.mode, req.top_k) {
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

async fn index_now(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let job = enqueue_index_job(&state, "manual_index").await;
    start_next_job_if_idle(state.clone());
    Ok(Json(json!({
        "status":"accepted",
        "job": job
    })))
}

fn normalize_source_path(path: &str) -> Result<String, (StatusCode, Json<Value>)> {
    let p = PathBuf::from(path);
    let meta = std::fs::metadata(&p).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"SOURCE_INVALID","message":format!("path not accessible: {}", e)}})),
        )
    })?;
    if !meta.is_dir() && !meta.is_file() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"SOURCE_INVALID","message":"source must be a file or directory"}})),
        ));
    }
    let normalized = p.canonicalize().map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"SOURCE_INVALID","message":format!("failed to canonicalize source path: {}", e)}})),
        )
    })?;
    Ok(normalized.to_string_lossy().to_string())
}

fn canonicalize_if_exists(path: &str) -> Option<String> {
    PathBuf::from(path)
        .canonicalize()
        .ok()
        .map(|p| p.to_string_lossy().to_string())
}

async fn sources_list(State(state): State<AppState>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let manifest = load_manifest(&state.pack).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"PACK_INVALID","message":e.to_string()}})),
        )
    })?;
    Ok(Json(json!({
        "status":"ok",
        "sources": manifest.sources
    })))
}

async fn sources_add(
    State(state): State<AppState>,
    Json(req): Json<SourceRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut manifest = load_manifest(&state.pack).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"PACK_INVALID","message":e.to_string()}})),
        )
    })?;
    let normalized = normalize_source_path(&req.path)?;
    if manifest.sources.iter().any(|s| s.root_path == normalized) {
        return Ok(Json(json!({
            "status":"ok",
            "added": false,
            "reason":"already_exists",
            "source": normalized,
            "sources": manifest.sources
        })));
    }

    manifest.sources.push(SourceConfig {
        root_path: normalized.clone(),
        include: vec!["**/*".to_string()],
        exclude: vec!["**/.git/**".to_string(), "**/target/**".to_string()],
    });
    save_manifest(&state.pack, manifest.clone()).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"PACK_WRITE_FAILED","message":e.to_string()}})),
        )
    })?;
    let job = enqueue_index_job(&state, "source_add").await;
    start_next_job_if_idle(state.clone());

    Ok(Json(json!({
        "status":"ok",
        "added": true,
        "source": normalized,
        "sources": manifest.sources,
        "job": job
    })))
}

async fn sources_remove(
    State(state): State<AppState>,
    Json(req): Json<SourceRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut manifest = load_manifest(&state.pack).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"PACK_INVALID","message":e.to_string()}})),
        )
    })?;

    let normalized_req = canonicalize_if_exists(&req.path).unwrap_or_else(|| req.path.clone());
    let before = manifest.sources.len();
    manifest.sources.retain(|s| {
        if s.root_path == req.path || s.root_path == normalized_req {
            return false;
        }
        if let Some(canon) = canonicalize_if_exists(&s.root_path) {
            canon != req.path && canon != normalized_req
        } else {
            true
        }
    });
    let removed = before.saturating_sub(manifest.sources.len());

    save_manifest(&state.pack, manifest.clone()).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"PACK_WRITE_FAILED","message":e.to_string()}})),
        )
    })?;

    Ok(Json(json!({
        "status":"ok",
        "removed": removed,
        "source": req.path,
        "sources": manifest.sources
    })))
}

async fn jobs_list(State(state): State<AppState>) -> Json<Value> {
    let jobs = state.jobs.lock().await;
    Json(json!({
        "status":"ok",
        "jobs": jobs.jobs
    }))
}

async fn job_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let jobs = state.jobs.lock().await;
    let Some(job) = jobs.find(&id).cloned() else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error":{"code":"JOB_NOT_FOUND","message":"job not found"}})),
        ));
    };
    Ok(Json(json!({
        "status":"ok",
        "job": job
    })))
}

async fn enqueue_index_job(state: &AppState, trigger: &str) -> Value {
    let mut jobs = state.jobs.lock().await;
    let id = format!("job-{}", jobs.next_id);
    jobs.next_id += 1;
    let record = JobRecord {
        id: id.clone(),
        job_type: JobType::IndexSources,
        state: JobState::Queued,
        trigger: trigger.to_string(),
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
        let maybe_job_id = {
            let mut jobs = state.jobs.lock().await;
            if jobs.running.is_some() {
                return;
            }
            let Some(id) = jobs.queue.pop_front() else {
                return;
            };
            jobs.running = Some(id.clone());
            if let Some(job) = jobs.find_mut(&id) {
                job.state = JobState::Running;
                job.started_at = Some(Utc::now());
            }
            id
        };

        let pack = state.pack.clone();
        let run_outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
            let manifest = load_manifest(&pack)?;
            let sources: Vec<PathBuf> = manifest
                .sources
                .iter()
                .map(|s| PathBuf::from(&s.root_path))
                .collect();
            let (scanned, updated, chunks) = run_index(&pack, &sources)?;
            Ok(json!({
                "scanned": scanned,
                "updated_files": updated,
                "chunks": chunks
            }))
        })
        .await;

        let mut jobs = state.jobs.lock().await;
        let finished_at = Utc::now();
        let (state_value, result_value, error_value) = match run_outcome {
            Ok(Ok(v)) => (JobState::Succeeded, Some(v), None),
            Ok(Err(e)) => (JobState::Failed, None, Some(e.to_string())),
            Err(e) => (JobState::Failed, None, Some(format!("job task failed: {}", e))),
        };
        if let Some(job) = jobs.find_mut(&maybe_job_id) {
            job.state = state_value;
            job.result = result_value;
            job.error = error_value;
            job.finished_at = Some(finished_at);
        }
        jobs.running = None;
        jobs.trim_history(100);
        drop(jobs);

        start_next_job_if_idle(state.clone());
    });
}

async fn ontology_sources(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let candidates = OntologyEngine::source_candidates(&state.pack).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"ONTOLOGY_LIST_FAILED","message":e.to_string()}})),
        )
    })?;
    let mut out = Vec::new();
    for candidate in candidates {
        match OntologyEngine::read_artifact(&candidate.artifact_path) {
            Ok(artifact) => out.push(json!({
                "source_path": artifact.source_path,
                "provider": artifact.provider,
                "model": artifact.model,
                "chunk_count": artifact.chunk_count,
                "generated_at": artifact.generated_at,
                "artifact_path": candidate.artifact_path.display().to_string()
            })),
            Err(e) => out.push(json!({
                "artifact_path": candidate.artifact_path.display().to_string(),
                "error": e.to_string()
            })),
        }
    }
    Ok(Json(json!({"status":"ok","sources":out})))
}

async fn ontology_source(
    State(state): State<AppState>,
    axum::extract::Query(req): axum::extract::Query<OntologySourceQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let lookup_path = req.path.clone();
    let lookup_normalized = canonicalize_if_exists(&lookup_path).unwrap_or_else(|| lookup_path.clone());
    let maybe_path = OntologyEngine::find_artifact_for_source(&state.pack, &lookup_path)
        .and_then(|v| {
            if v.is_some() {
                Ok(v)
            } else {
                OntologyEngine::find_artifact_for_source(&state.pack, &lookup_normalized)
            }
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":{"code":"ONTOLOGY_SOURCE_LOOKUP_FAILED","message":e.to_string()}})),
            )
        })?;
    let Some(path) = maybe_path else {
        let candidates = OntologyEngine::source_candidates(&state.pack).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":{"code":"ONTOLOGY_SOURCE_LOOKUP_FAILED","message":e.to_string()}})),
            )
        })?;
        let is_directory = FsPath::new(&lookup_normalized).is_dir();
        let suggestions = suggestion_candidates(&lookup_normalized, is_directory, &candidates, 10)
            .into_iter()
            .map(|c| {
                json!({
                    "source_path": c.source_path,
                    "artifact_path": c.artifact_path.display().to_string()
                })
            })
            .collect::<Vec<_>>();
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "error":{
                    "code":"ONTOLOGY_SOURCE_NOT_FOUND",
                    "message":"no ontology artifact for source",
                    "lookup_path": lookup_path,
                    "is_directory": is_directory,
                    "suggestions": suggestions
                }
            })),
        ));
    };

    let artifact = OntologyEngine::read_artifact(&path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":{"code":"ONTOLOGY_SOURCE_READ_FAILED","message":e.to_string()}})),
        )
    })?;
    Ok(Json(json!({
        "status":"ok",
        "artifact_path": path.display().to_string(),
        "artifact": artifact
    })))
}

fn suggestion_candidates(
    lookup_path: &str,
    is_directory: bool,
    candidates: &[crate::ontology::OntologySourceCandidate],
    limit: usize,
) -> Vec<crate::ontology::OntologySourceCandidate> {
    if candidates.is_empty() {
        return Vec::new();
    }

    if is_directory {
        let by_prefix = OntologyEngine::filter_candidates_by_prefix(candidates, lookup_path);
        if by_prefix.is_empty() {
            return candidates.iter().take(limit).cloned().collect();
        }
        return by_prefix.into_iter().take(limit).collect();
    }

    let lookup = FsPath::new(lookup_path);
    let parent = lookup.parent().and_then(|p| p.to_str()).unwrap_or("");
    let file_name = lookup.file_name().and_then(|f| f.to_str()).unwrap_or("");

    let mut scored = candidates
        .iter()
        .map(|c| {
            let mut score = 0i32;
            if !parent.is_empty() && c.source_path.starts_with(parent) {
                score += 2;
            }
            if !file_name.is_empty() && c.source_path.contains(file_name) {
                score += 1;
            }
            (score, c.clone())
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.source_path.cmp(&b.1.source_path)));
    scored
        .into_iter()
        .filter(|(score, _)| *score > 0)
        .map(|(_, c)| c)
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::ontology::OntologySourceCandidate;

    use super::suggestion_candidates;

    #[test]
    fn directory_lookup_returns_prefix_matches() {
        let candidates = vec![
            OntologySourceCandidate {
                source_path: "/repo/specs/a.md".to_string(),
                artifact_path: PathBuf::from("/tmp/a.json"),
            },
            OntologySourceCandidate {
                source_path: "/repo/specs/b.md".to_string(),
                artifact_path: PathBuf::from("/tmp/b.json"),
            },
            OntologySourceCandidate {
                source_path: "/repo/src/main.rs".to_string(),
                artifact_path: PathBuf::from("/tmp/c.json"),
            },
        ];

        let out = suggestion_candidates("/repo/specs", true, &candidates, 10);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|c| c.source_path.starts_with("/repo/specs")));
    }

    #[test]
    fn file_lookup_prefers_same_parent() {
        let candidates = vec![
            OntologySourceCandidate {
                source_path: "/repo/specs/a.md".to_string(),
                artifact_path: PathBuf::from("/tmp/a.json"),
            },
            OntologySourceCandidate {
                source_path: "/repo/specs/b.md".to_string(),
                artifact_path: PathBuf::from("/tmp/b.json"),
            },
            OntologySourceCandidate {
                source_path: "/repo/docs/readme.md".to_string(),
                artifact_path: PathBuf::from("/tmp/c.json"),
            },
        ];

        let out = suggestion_candidates("/repo/specs/missing.md", false, &candidates, 10);
        assert!(!out.is_empty());
        assert!(out[0].source_path.starts_with("/repo/specs"));
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
            "serverInfo": {"name":"satori","version":"0.1.0"},
            "capabilities": {"tools": {}}
        }),
        "tools/list" => json!({
            "tools": [
                {"name":"memory_query","description":"Query local memory pack","inputSchema":{
                    "type":"object","properties":{
                        "query":{"type":"string"},
                        "mode":{"type":"string","enum":["vector","hybrid"]},
                        "top_k":{"type":"number"}
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
                    let mode = args
                        .get("mode")
                        .and_then(Value::as_str)
                        .unwrap_or("hybrid")
                        .to_string();
                    let top_k = args.get("top_k").and_then(Value::as_u64).unwrap_or(8) as usize;

                    match run_query(&state.pack, &query, &mode, top_k) {
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
                    "content":[{"type":"text","text":json!({"status":"ok","pack_path":state.pack.display().to_string()}).to_string()}]
                }),
                "memory_sources" => json!({
                    "content":[{"type":"text","text":match load_manifest(&state.pack) {
                        Ok(m) => json!({"sources":m.sources}).to_string(),
                        Err(_) => json!({"sources":[]}).to_string(),
                    }}]
                }),
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
