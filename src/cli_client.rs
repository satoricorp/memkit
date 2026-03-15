use std::fs;
use std::io::{Read, Seek, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use owo_colors::OwoColorize;
use serde_json::{Value, json};

use crate::term;

// --- Session refcount and PID file (~/.memkit) ---

fn memkit_dir() -> Result<PathBuf> {
    dirs::home_dir()
        .context("home directory not available")
        .map(|h| h.join(".memkit"))
}

fn refcount_path() -> Result<PathBuf> {
    Ok(memkit_dir()?.join("cli-refcount"))
}

fn server_pid_path() -> Result<PathBuf> {
    Ok(memkit_dir()?.join("server.pid"))
}

fn refcount_inc() -> Result<()> {
    let dir = memkit_dir()?;
    fs::create_dir_all(&dir).context("create ~/.memkit")?;
    let path = refcount_path()?;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .context("open refcount file")?;
    f.lock_exclusive().context("lock refcount file")?;
    let mut s = String::new();
    f.read_to_string(&mut s).context("read refcount")?;
    let n: u32 = s.trim().parse().unwrap_or(0);
    f.set_len(0).context("truncate refcount")?;
    f.rewind().context("rewind refcount")?;
    write!(f, "{}", n + 1).context("write refcount")?;
    f.unlock().context("unlock refcount")?;
    Ok(())
}

fn refcount_dec() -> Result<u32> {
    let path = refcount_path()?;
    let mut f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .context("open refcount file")?;
    f.lock_exclusive().context("lock refcount file")?;
    let mut s = String::new();
    f.read_to_string(&mut s).context("read refcount")?;
    let n: u32 = s.trim().parse().unwrap_or(0);
    let new_n = n.saturating_sub(1);
    f.set_len(0).context("truncate refcount")?;
    f.rewind().context("rewind refcount")?;
    write!(f, "{}", new_n).context("write refcount")?;
    f.unlock().context("unlock refcount")?;
    Ok(new_n)
}

fn pid_write(pid: u32) -> Result<()> {
    let dir = memkit_dir()?;
    fs::create_dir_all(&dir).context("create ~/.memkit")?;
    let path = server_pid_path()?;
    fs::write(&path, pid.to_string()).context("write server.pid")?;
    Ok(())
}

fn pid_remove() {
    let _ = server_pid_path().and_then(|p| fs::remove_file(p));
}

fn pid_read_and_kill() {
    let path = match server_pid_path() {
        Ok(p) => p,
        Err(_) => return,
    };
    let s = match fs::read_to_string(&path) {
        Ok(x) => x,
        Err(_) => return,
    };
    let pid: u32 = match s.trim().parse() {
        Ok(x) => x,
        Err(_) => return,
    };
    #[cfg(unix)]
    {
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }
    let _ = fs::remove_file(&path);
}

/// Guard for a server process started by the CLI. Call `shutdown()` when done so the child is killed and reaped (when refcount hits 0).
pub struct ServerGuard {
    child: Option<Child>,
    /// Some only for ensure_server (shared port); None for ensure_server_standalone (Index).
    session_refcounted: Option<()>,
}

impl ServerGuard {
    /// Decrement refcount if we're on the shared path; if refcount reaches 0, kill our child or the PID in ~/.memkit/server.pid.
    pub fn shutdown(mut self) -> Result<()> {
        if self.session_refcounted.take().is_some() {
            let count = refcount_dec()?;
            if count == 0 {
                if let Some(mut child) = self.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                    pid_remove();
                } else {
                    pid_read_and_kill();
                }
            }
            // count > 0: another CLI is still using the server; drop child without killing
        } else {
            // Standalone (Index): always kill our child
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        Ok(())
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        if self.session_refcounted.take().is_some() {
            let _ = refcount_dec();
        }
    }
}

#[derive(Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl ServerConfig {
    pub fn from_env() -> Self {
        let host = std::env::var("API_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let port = std::env::var("API_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(4242);
        Self { host, port }
    }

    fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

#[derive(Clone)]
pub struct QueryArgs {
    pub query: String,
    pub top_k: usize,
    pub use_reranker: bool,
    pub raw: bool,
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("failed to build HTTP client")
}

/// Short timeout for /health so we don't wait 120s when something on the port is blocking (e.g. old server).
fn health_check_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("failed to build HTTP client")
}

/// Client with long timeout for /index (copy + enqueue can be slow for iCloud/FileProvider).
fn index_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .context("failed to build HTTP client")
}

async fn server_is_up(cfg: &ServerConfig) -> bool {
    let Ok(client) = health_check_client() else {
        return false;
    };
    let url = format!("{}/health", cfg.base_url());
    match client.get(url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

const HEALTH_POLL_INTERVAL_MS: u64 = 300;
const HEALTH_POLL_ATTEMPTS: usize = 200; // 200 * 300ms = 60s total

fn spawn_server(cfg: &ServerConfig, packs: &[PathBuf]) -> Result<Child> {
    let exe = std::env::current_exe().context("failed to get current executable")?;
    let pack_paths: String = packs
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let mut cmd = Command::new(&exe);
    cmd.arg("--headless-serve")
        .arg("--pack")
        .arg(&pack_paths)
        .arg("--host")
        .arg(&cfg.host)
        .arg("--port")
        .arg(cfg.port.to_string())
        .env("API_PORT", cfg.port.to_string())
        .env("MEMKIT_PACK_PATHS", &pack_paths);
    if let Ok(v) = std::env::var("MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON") {
        cmd.env("MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON", v);
    }
    if let Ok(v) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        cmd.env("GOOGLE_APPLICATION_CREDENTIALS", v);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let child = cmd.spawn().with_context(|| {
        format!(
            "failed to start server ({}). try a different port or ensure no other process is using it",
            exe.display()
        )
    })?;
    Ok(child)
}

fn find_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("find free port")?;
    Ok(listener.local_addr().context("local_addr")?.port())
}

pub async fn ensure_server(cfg: &ServerConfig, packs: &[PathBuf]) -> Result<ServerGuard> {
    refcount_inc()?;
    if server_is_up(cfg).await {
        return Ok(ServerGuard {
            child: None,
            session_refcounted: Some(()),
        });
    }
    let mut child = match spawn_server(cfg, packs) {
        Ok(c) => c,
        Err(e) => {
            let _ = refcount_dec();
            return Err(e);
        }
    };
    for _ in 0..HEALTH_POLL_ATTEMPTS {
        tokio::time::sleep(Duration::from_millis(HEALTH_POLL_INTERVAL_MS)).await;
        if server_is_up(cfg).await {
            pid_write(child.id()).context("write server.pid")?;
            return Ok(ServerGuard {
                child: Some(child),
                session_refcounted: Some(()),
            });
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = refcount_dec();
    Err(anyhow!(
        "server did not become healthy after {}s on {}:{}. try a different port or ensure no other process is using it",
        (HEALTH_POLL_ATTEMPTS as u64 * HEALTH_POLL_INTERVAL_MS) / 1000,
        cfg.host,
        cfg.port
    ))
}

/// Start a server on an ephemeral port (no reuse of existing server). Use for index so our process creates the pack.
pub async fn ensure_server_standalone(
    cfg: &ServerConfig,
    packs: &[PathBuf],
) -> Result<(ServerGuard, ServerConfig)> {
    let port = find_free_port()?;
    let standalone_cfg = ServerConfig {
        host: cfg.host.clone(),
        port,
    };
    let mut child = spawn_server(&standalone_cfg, packs)?;
    for _ in 0..HEALTH_POLL_ATTEMPTS {
        tokio::time::sleep(Duration::from_millis(HEALTH_POLL_INTERVAL_MS)).await;
        if server_is_up(&standalone_cfg).await {
            return Ok((
                ServerGuard {
                    child: Some(child),
                    session_refcounted: None,
                },
                standalone_cfg,
            ));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(anyhow!(
        "server did not become healthy after {}s on {}:{}",
        (HEALTH_POLL_ATTEMPTS as u64 * HEALTH_POLL_INTERVAL_MS) / 1000,
        standalone_cfg.host,
        standalone_cfg.port
    ))
}

/// Poll /status until the index job is no longer active (or timeout). Use after POST /index when the CLI started the server.
pub async fn poll_until_index_done(cfg: &ServerConfig, pack_path: &str) -> Result<()> {
    const POLL_INTERVAL: Duration = Duration::from_secs(2);
    const MAX_WAIT: Duration = Duration::from_secs(7200); // 2 hours
    let deadline = std::time::Instant::now() + MAX_WAIT;
    while std::time::Instant::now() < deadline {
        let data = status(cfg, Some(pack_path)).await?;
        let active = data
            .get("jobs")
            .and_then(|j| j.get("active"))
            .map(|v| !v.is_null())
            .unwrap_or(false);
        if !active {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(anyhow!(
        "index job did not complete within {}s",
        MAX_WAIT.as_secs()
    ))
}

pub async fn status(cfg: &ServerConfig, dir: Option<&str>) -> Result<Value> {
    let client = http_client()?;
    let mut url = format!("{}/status", cfg.base_url());
    if let Some(d) = dir {
        url.push_str(&format!("?path={}", d));
    }
    let resp = client.get(url).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("status request failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}

pub fn print_status(data: &Value) {
    let pack_path = data
        .get("pack_path")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let indexed = data.get("indexed").and_then(Value::as_bool).unwrap_or(false);
    let vector_count = data.get("vector_count").and_then(Value::as_u64).unwrap_or(0) as usize;
    let entities = data.get("entities").and_then(Value::as_u64).unwrap_or(0) as usize;
    let relationships = data.get("relationships").and_then(Value::as_u64).unwrap_or(0) as usize;
    let file_tree = data.get("file_tree").and_then(Value::as_str).unwrap_or("");
    let jobs = data.get("jobs").and_then(Value::as_object);
    let active_job = jobs.and_then(|j| j.get("active")).map(|v| !v.is_null()).unwrap_or(false);
    let active_job_id = jobs.and_then(|j| j.get("active")).and_then(Value::as_object).and_then(|o| o.get("id")).and_then(Value::as_str);
    let queued_jobs = jobs.and_then(|j| j.get("queued_jobs")).and_then(Value::as_array).map(|a| a.as_slice()).unwrap_or(&[]);

    if term::color_stdout() {
        if active_job {
            let id = active_job_id.unwrap_or("?");
            println!("{} {} {}", pack_path.bold(), id.dimmed(), "...pending".yellow());
        } else if indexed {
            println!("{} successfully indexed", pack_path.bold().green());
        } else {
            println!("{} not indexed", pack_path.bold().yellow());
        }
        println!("location: [local] [cloud]");
        if !file_tree.is_empty() {
            println!();
            println!("{}", file_tree.dimmed());
        }
        println!();
        println!(
            "{} vector entries",
            vector_count.to_string().cyan()
        );
        println!(
            "{} entities, {} relationships",
            entities.to_string().cyan(),
            relationships.to_string().cyan()
        );
        for q in queued_jobs {
            if let Some(id) = q.get("id").and_then(Value::as_str) {
                println!("  {} {}", id.dimmed(), "...pending".yellow());
            }
        }
    } else {
        if active_job {
            let id = active_job_id.unwrap_or("?");
            println!("{} {} ...pending", pack_path, id);
        } else if indexed {
            println!("{} successfully indexed", pack_path);
        } else {
            println!("{} not indexed", pack_path);
        }
        println!("location: [local] [cloud]");
        if !file_tree.is_empty() {
            println!();
            println!("{}", file_tree);
        }
        println!();
        println!("{} vector entries", vector_count);
        println!("{} entities, {} relationships", entities, relationships);
        for q in queued_jobs {
            if let Some(id) = q.get("id").and_then(Value::as_str) {
                println!("  {} ...pending", id);
            }
        }
    }
}

pub async fn list(cfg: &ServerConfig, output_json: bool) -> Result<Value> {
    let reg = crate::registry::load_registry().unwrap_or_default();
    if reg.packs.is_empty() {
        let data = status(cfg, None).await?;
        if !output_json {
            let pack_path = data
                .get("pack_path")
                .and_then(Value::as_str)
                .unwrap_or("?");
            let sources = data.get("sources").and_then(Value::as_array).cloned().unwrap_or_default();
            let active_job = data.get("jobs").and_then(|j| j.get("active"));

            if term::color_stdout() {
                println!("{} [local] [cloud]", pack_path.bold());
                for s in sources.iter().take(10) {
                    let path = s.get("root_path").and_then(Value::as_str).unwrap_or("?");
                    println!("  {}", path.dimmed());
                }
                if let Some(obj) = active_job.and_then(Value::as_object) {
                    let job_id = obj.get("id").and_then(Value::as_str).unwrap_or("?");
                    println!("  {} {}", job_id.dimmed(), "...pending".yellow());
                }
            } else {
                println!("{} [local] [cloud]", pack_path);
                for s in sources.iter().take(10) {
                    let path = s.get("root_path").and_then(Value::as_str).unwrap_or("?");
                    println!("  {}", path);
                }
                if let Some(obj) = active_job.and_then(Value::as_object) {
                    let job_id = obj.get("id").and_then(Value::as_str).unwrap_or("?");
                    println!("  {} ...pending", job_id);
                }
            }
        }
        return Ok(data);
    }

    if !output_json {
        let home_canon = dirs::home_dir()
            .and_then(|h| std::path::Path::new(&h).canonicalize().ok())
            .and_then(|h| h.to_str().map(String::from));
        for p in &reg.packs {
            let default_marker = if p.default { " (default)" } else { "" };
            let path_display = if home_canon.as_ref() == Some(&p.path) {
                "~"
            } else {
                p.path.as_str()
            };
            let (lead, path_part) = if let Some(ref name) = p.name {
                (name.as_str(), path_display)
            } else {
                (path_display, "")
            };
            if term::color_stdout() {
                let cloud = if p.cloud { format!("{}", "[cloud]".cyan()) } else { format!("{}", "cloud".dimmed()) };
                if path_part.is_empty() {
                    println!(
                        "{} [local] {} {}",
                        lead.bold(),
                        cloud,
                        default_marker.dimmed()
                    );
                } else {
                    println!(
                        "{}  {} [local] {} {}",
                        lead.bold(),
                        path_part.dimmed(),
                        cloud,
                        default_marker.dimmed()
                    );
                }
            } else {
                let cloud = if p.cloud { "[cloud]" } else { "cloud" };
                if path_part.is_empty() {
                    println!("{} [local] {} {}", lead, cloud, default_marker);
                } else {
                    println!("{}  {} [local] {} {}", lead, path_part, cloud, default_marker);
                }
            }
            // Show sources and pending job for this pack (requires server).
            if let Ok(data) = status(cfg, Some(&p.path)).await {
                let sources = data.get("sources").and_then(Value::as_array).map_or([].as_ref(), |v| v.as_slice());
                for s in sources.iter().take(20) {
                    let path = s.get("root_path").and_then(Value::as_str).unwrap_or("?");
                    if term::color_stdout() {
                        println!("  {}", path.dimmed());
                    } else {
                        println!("  {}", path);
                    }
                }
                let active_job = data.get("jobs").and_then(|j| j.get("active"));
                if let Some(obj) = active_job.and_then(Value::as_object) {
                    let job_id = obj.get("id").and_then(Value::as_str).unwrap_or("?");
                    if term::color_stdout() {
                        println!("  {} {}", job_id.dimmed(), "...pending".yellow());
                    } else {
                        println!("  {} ...pending", job_id);
                    }
                }
                let queued_jobs = data.get("jobs").and_then(|j| j.get("queued_jobs")).and_then(Value::as_array).map(|a| a.as_slice()).unwrap_or(&[]);
                for q in queued_jobs {
                    if let Some(id) = q.get("id").and_then(Value::as_str) {
                        if term::color_stdout() {
                            println!("  {} {}", id.dimmed(), "...pending".yellow());
                        } else {
                            println!("  {} ...pending", id);
                        }
                    }
                }
                // Status summary: vectors, entities, relationships (so progress is visible when indexing).
                let indexed = data.get("indexed").and_then(Value::as_bool).unwrap_or(false);
                let vector_count = data.get("vector_count").and_then(Value::as_u64).unwrap_or(0) as usize;
                let entities = data.get("entities").and_then(Value::as_u64).unwrap_or(0) as usize;
                let relationships = data.get("relationships").and_then(Value::as_u64).unwrap_or(0) as usize;
                let counts_suffix = format!("{} vectors, {} entities, {} relationships", vector_count, entities, relationships);
                let active_obj = active_job.and_then(Value::as_object);
                let status_line = if let Some(ref obj) = active_obj {
                    let id = obj.get("id").and_then(Value::as_str).unwrap_or("?");
                    format!("indexing ({}) — {}", id, counts_suffix)
                } else if indexed {
                    format!("indexed, {}", counts_suffix)
                } else {
                    format!("not indexed ({})", counts_suffix)
                };
                if term::color_stdout() {
                    if active_obj.is_some() {
                        println!("  {}", status_line.yellow());
                    } else if indexed {
                        println!("  {}", status_line.green());
                    } else {
                        println!("  {}", status_line.dimmed());
                    }
                } else {
                    println!("  {}", status_line);
                }
            }
        }
    }
    Ok(json!({"packs": reg.packs}))
}

pub async fn index(cfg: &ServerConfig, path: &str, name: Option<&str>, dry_run: bool, output_json: bool) -> Result<Value> {
    if dry_run {
        return Ok(json!({
            "dry_run": true,
            "would": "index",
            "path": path,
            "name": name,
            "status": "skipped"
        }));
    }
    let client = index_http_client()?;
    let url = format!("{}/index", cfg.base_url());
    let mut body = json!({"path": path});
    if let Some(n) = name {
        body["name"] = json!(n);
    }
    let resp = client.post(url).json(&body).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("index request failed: {}", body));
    }
    let out: Value = serde_json::from_str(&body)?;
    if !output_json {
        if let Some(job) = out.get("job").and_then(|j| j.get("id")).and_then(Value::as_str) {
            if term::color_stdout() {
                println!(
                    "{} {} ({}). Run 'mk status {}' to check progress.",
                    "Indexing".green(),
                    path,
                    job,
                    path
                );
            } else {
                println!("Indexing {} ({}). Run 'mk status {}' to check progress.", path, job, path);
            }
        }
    }
    Ok(out)
}

pub async fn query(cfg: &ServerConfig, args: &QueryArgs, pack: Option<&str>) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/query", cfg.base_url());
    let mut body = json!({
        "query": args.query,
        "top_k": args.top_k,
        "use_reranker": args.use_reranker,
        "raw": args.raw
    });
    if let Some(p) = pack {
        body["pack"] = json!(p);
    }
    let resp = client.post(url).json(&body).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("query request failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}

pub async fn graph_show(cfg: &ServerConfig) -> Result<()> {
    let url = format!("{}/graph/view", cfg.base_url());
    opener::open(url).context("failed to open graph view in browser")?;
    Ok(())
}

pub async fn publish(
    cfg: &ServerConfig,
    pack: Option<&str>,
    destination: Option<&str>,
    output_json: bool,
) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/publish", cfg.base_url());
    let mut body = serde_json::Map::new();
    if let Some(p) = pack {
        body.insert("path".to_string(), json!(p));
    }
    if let Some(d) = destination {
        body.insert("destination".to_string(), json!(d));
    }
    let resp = client
        .post(url)
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let resp_body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("publish request failed: {}", resp_body));
    }
    let out: Value = serde_json::from_str(&resp_body)?;
    if !output_json {
        if let Some(uri) = out.get("uri").and_then(Value::as_str) {
            if term::color_stdout() {
                println!("{} {}", "Published to".green(), uri);
            } else {
                println!("Published to {}", uri);
            }
        }
    }
    Ok(out)
}

pub async fn add(cfg: &ServerConfig, body: &serde_json::Value) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/add", cfg.base_url());
    let resp = client.post(url).json(body).send().await?;
    let status = resp.status();
    let resp_body = resp.text().await?;
    if !status.is_success() {
        if let Ok(err_json) = serde_json::from_str::<Value>(&resp_body) {
            if let Some(msg) = err_json.get("error").and_then(|e| e.get("message")).and_then(Value::as_str) {
                return Err(anyhow!("add request failed: {}", msg));
            }
        }
        if resp_body.is_empty() {
            return Err(anyhow!("add request failed: HTTP {} (empty response)", status.as_u16()));
        }
        return Err(anyhow!("add request failed: {}", resp_body));
    }
    serde_json::from_str(&resp_body).context("parse add response")
}

/// Print "Adding … (job-N). Run 'mk status' to check progress." when add response has status accepted and job id.
pub fn print_add_started(data: &Value, pack_path: &str) {
    if let Some(job_id) = data.get("job").and_then(|j| j.get("id")).and_then(Value::as_str) {
        if term::color_stdout() {
            println!(
                "{} ({}). Run 'mk status {}' to check progress.",
                "Adding".green(),
                job_id,
                pack_path
            );
        } else {
            println!("Adding ({}). Run 'mk status {}' to check progress.", job_id, pack_path);
        }
    }
}
