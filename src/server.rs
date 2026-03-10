use std::net::SocketAddr;
use std::path::PathBuf;
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

use crate::falkor_store::{
    graph_counts as falkor_graph_counts, graph_name_from_env,
    graph_schema as falkor_graph_schema, graph_subgraph as falkor_graph_subgraph,
};
use crate::indexer::run_index;
use crate::file_tree::format_file_tree;
use crate::pack::{init_pack, load_index, load_manifest, save_manifest};
use crate::memkit_txt::ensure_memkit_txt;
use crate::registry::{pack_dir_for_path, ensure_registered};
use crate::types::SourceConfig;
use crate::query::run_query;
use crate::query_synth::synthesize_answer;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pack_path: Option<String>,
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
    pack: Option<String>,
}

#[derive(Deserialize, Default)]
struct StatusQuery {
    path: Option<String>,
}

#[derive(Deserialize, Default)]
struct IndexRequest {
    path: Option<String>,
}

#[derive(Deserialize)]
struct SubgraphRequest {
    query: String,
    #[serde(default = "default_depth")]
    depth: usize,
    #[serde(default = "default_limit")]
    limit: usize,
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

async fn status(
    State(state): State<AppState>,
    Query(q): Query<StatusQuery>,
) -> Json<Value> {
    let pack = if let Some(ref path) = q.path {
        let dir = PathBuf::from(path)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(path));
        if dir.join(".memkit/manifest.json").exists() {
            dir.join(".memkit")
        } else if dir.join("manifest.json").exists() {
            dir
        } else {
            state.pack.as_ref().clone()
        }
    } else {
        state.pack.as_ref().clone()
    };

    let manifest = load_manifest(&pack).ok();
    let index = load_index(&pack).ok();
    let vector_count = index.as_ref().map(|i| i.docs.len()).unwrap_or(0);
    let indexed = vector_count > 0;
    let file_paths: Vec<String> = index
        .as_ref()
        .map(|i| i.docs.iter().map(|d| d.source_path.clone()).collect())
        .unwrap_or_default();

    let (entities, relationships) = state
        .falkordb_socket
        .as_ref()
        .and_then(|sock| falkor_graph_counts(sock, &state.falkor_graph).ok())
        .unwrap_or((0, 0));

    let pack_str = pack.display().to_string();
    let base_path = pack
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| *n == ".memkit")
        .and_then(|_| pack.parent())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| pack_str.clone());
    let file_tree = format_file_tree(&file_paths, &base_path);

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
        "pack_path": pack_str,
        "indexed": indexed,
        "vector_count": vector_count,
        "entities": entities,
        "relationships": relationships,
        "file_tree": file_tree,
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
    let pack = if let Some(ref path) = req.pack {
        let dir = PathBuf::from(path);
        if dir.join(".memkit/manifest.json").exists() {
            dir.join(".memkit")
        } else if dir.join("manifest.json").exists() {
            dir
        } else {
            state.pack.as_ref().clone()
        }
    } else {
        state.pack.as_ref().clone()
    };

    match run_query(&pack, &req.query, &req.mode, req.top_k) {
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
    Json(req): Json<IndexRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (pack_to_index, _is_new_default) = if let Some(path) = req.path {
        let dir = PathBuf::from(&path)
            .canonicalize()
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"code":"PATH_INVALID","message":format!("path not accessible: {}", e)}})),
                )
            })?;
        let pack_dir = pack_dir_for_path(&dir);
        let normalized = dir.to_string_lossy().to_string();

        let _ = ensure_memkit_txt(&dir);

        if !pack_dir.join("manifest.json").exists() {
            init_pack(&pack_dir, false, "fastembed", "BAAI/bge-small-en-v1.5", 384)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error":{"code":"INIT_FAILED","message":e.to_string()}})),
                    )
                })?;
            let mut manifest = load_manifest(&pack_dir).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error":{"code":"PACK_INVALID","message":e.to_string()}})),
                )
            })?;
            manifest.sources.push(SourceConfig {
                root_path: normalized.clone(),
                include: vec!["**/*".to_string()],
                exclude: vec!["**/.git/**".to_string(), "**/target/**".to_string()],
            });
            save_manifest(&pack_dir, manifest).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error":{"code":"PACK_WRITE_FAILED","message":e.to_string()}})),
                )
            })?;
        } else {
            let mut manifest = load_manifest(&pack_dir).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":{"code":"PACK_INVALID","message":e.to_string()}})),
                )
            })?;
            if !manifest.sources.iter().any(|s| s.root_path == normalized) {
                manifest.sources.push(SourceConfig {
                    root_path: normalized.clone(),
                    include: vec!["**/*".to_string()],
                    exclude: vec!["**/.git/**".to_string(), "**/target/**".to_string()],
                });
                save_manifest(&pack_dir, manifest).map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error":{"code":"PACK_WRITE_FAILED","message":e.to_string()}})),
                    )
                })?;
            }
        }

        let reg = crate::registry::load_registry().unwrap_or_default();
        let is_first = reg.packs.is_empty();
        let _ = ensure_registered(&normalized, is_first);

        (Some(pack_dir.to_string_lossy().to_string()), is_first)
    } else {
        (None, false)
    };

    let job = enqueue_index_job(&state, "manual_index", pack_to_index).await;
    start_next_job_if_idle(state.clone());
    Ok(Json(json!({
        "status":"accepted",
        "job": job
    })))
}

async fn enqueue_index_job(state: &AppState, trigger: &str, pack_path: Option<String>) -> Value {
    let mut jobs = state.jobs.lock().await;
    let id = format!("job-{}", jobs.next_id);
    jobs.next_id += 1;
    let record = JobRecord {
        id: id.clone(),
        job_type: JobType::IndexSources,
        state: JobState::Queued,
        trigger: trigger.to_string(),
        pack_path: pack_path.clone(),
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
        let (maybe_job_id, pack_to_use) = {
            let mut jobs = state.jobs.lock().await;
            if jobs.running.is_some() {
                return;
            }
            let Some(id) = jobs.queue.pop_front() else {
                return;
            };
            let pack_path = jobs.find(&id).and_then(|j| j.pack_path.clone());
            jobs.running = Some(id.clone());
            if let Some(job) = jobs.find_mut(&id) {
                job.state = JobState::Running;
                job.started_at = Some(Utc::now());
            }
            let pack = pack_path
                .map(PathBuf::from)
                .unwrap_or_else(|| (*state.pack).clone());
            (id, pack)
        };

        let run_outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
            let manifest = load_manifest(&pack_to_use)?;
            let sources: Vec<PathBuf> = manifest
                .sources
                .iter()
                .map(|s| PathBuf::from(&s.root_path))
                .collect();
            let (scanned, updated, chunks) = run_index(&pack_to_use, &sources)?;
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
