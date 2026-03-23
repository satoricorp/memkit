use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

use crate::registry::{Registry, RegistryPack};
use crate::term;

fn pack_path_display(p: &RegistryPack, home_canon: &Option<PathBuf>) -> String {
    let path_is_home = PathBuf::from(&p.path)
        .canonicalize()
        .ok()
        .as_ref()
        == home_canon.as_ref();
    if path_is_home {
        "~/.memkit".to_string()
    } else {
        p.path.clone()
    }
}

fn pack_list_paren_label(p: &RegistryPack, reg: &Registry, home_canon: &Option<PathBuf>) -> Option<String> {
    if let Some(ref n) = p.name {
        return Some(format!("({})", n));
    }
    let path_is_home = PathBuf::from(&p.path)
        .canonicalize()
        .ok()
        .as_ref()
        == home_canon.as_ref();
    let is_default_pack = p.default
        || reg.default_path.as_deref() == Some(p.path.as_str())
        || reg.packs.len() == 1
        || (path_is_home && reg.default_path.is_none());
    if is_default_pack {
        Some("(default)".to_string())
    } else {
        None
    }
}

fn bracket_local_cloud(c: bool, local_on: bool, cloud_on: bool) -> String {
    let local = if local_on {
        term::magenta_words(c, "[local]")
    } else {
        term::dimmed_word(c, "[local]")
    };
    let cloud = if cloud_on {
        term::magenta_words(c, "[cloud]")
    } else {
        term::dimmed_word(c, "[cloud]")
    };
    format!("{} {}", local, cloud)
}

/// Strip internal `sources/<copy>/` prefix from status file tree lines for display.
fn user_facing_file_tree(file_tree: &str) -> String {
    if file_tree.is_empty() {
        return String::new();
    }
    file_tree
        .lines()
        .map(|line| {
            let prefix = if line.starts_with("├── ") {
                Some("├── ")
            } else if line.starts_with("└── ") {
                Some("└── ")
            } else {
                None
            };
            let Some(p) = prefix else {
                return line.to_string();
            };
            let rest = line[p.len()..].replace('\\', "/");
            if let Some(after_sources) = rest.strip_prefix("sources/") {
                if let Some(idx) = after_sources.find('/') {
                    let after_name = &after_sources[idx + 1..];
                    format!("{}{}", p, after_name)
                } else {
                    format!("{}{}", p, after_sources)
                }
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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

    /// Same host/port resolution as `serve_with_startup` after CLI defaults (`API_PORT` overrides `port`).
    pub fn for_cli_serve(host: Option<String>, port: Option<u16>) -> Self {
        let host = host.unwrap_or_else(|| "127.0.0.1".to_string());
        let port_cli = port.unwrap_or(4242);
        let port = std::env::var("API_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(port_cli);
        Self { host, port }
    }

    pub fn base_url(&self) -> String {
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

/// Shared HTTP client: per-request timeouts (`RequestBuilder::timeout`) so one policy applies everywhere.
fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .context("failed to build HTTP client")
}

const REQ_TIMEOUT_DEFAULT: Duration = Duration::from_secs(120);
const REQ_TIMEOUT_HEALTH: Duration = Duration::from_secs(5);
const REQ_TIMEOUT_INDEX: Duration = Duration::from_secs(600);
const ENSURE_SERVER_MAX_WAIT: Duration = Duration::from_secs(30);
const ENSURE_SERVER_POLL: Duration = Duration::from_millis(250);

pub async fn doctor(cfg: &ServerConfig) -> Result<Value> {
    let exe = std::env::current_exe().ok();
    let mut out = json!({
        "binary": exe.as_ref().map(|p| p.display().to_string()),
        "config_path": crate::config::config_path().display().to_string(),
        "config_exists": crate::config::config_path().exists(),
        "server_url": cfg.base_url(),
        "server_reachable": false,
    });
    if server_is_up(cfg).await {
        out["server_reachable"] = json!(true);
        let client = http_client()?;
        let url = format!("{}/health", cfg.base_url());
        match client.get(url).timeout(REQ_TIMEOUT_HEALTH).send().await {
            Ok(resp) => {
                out["health_status"] = json!(resp.status().as_u16());
                if let Ok(body) = resp.text().await {
                    if let Ok(v) = serde_json::from_str::<Value>(&body) {
                        out["health"] = v;
                    } else {
                        out["health_raw"] = json!(body);
                    }
                }
            }
            Err(e) => {
                out["health_error"] = json!(e.to_string());
            }
        }
    }
    Ok(out)
}

async fn server_is_up(cfg: &ServerConfig) -> bool {
    let Ok(client) = http_client() else {
        return false;
    };
    let url = format!("{}/health", cfg.base_url());
    match client.get(url).timeout(REQ_TIMEOUT_HEALTH).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Wait until `/health` succeeds or timeout (used after spawning a background server).
pub async fn wait_for_server_ready(cfg: &ServerConfig) -> Result<()> {
    let deadline = std::time::Instant::now() + ENSURE_SERVER_MAX_WAIT;
    while std::time::Instant::now() < deadline {
        if server_is_up(cfg).await {
            return Ok(());
        }
        tokio::time::sleep(ENSURE_SERVER_POLL).await;
    }
    Err(anyhow!(
        "memkit server did not become ready at {} within {}s (check port {} or run `mk serve --foreground` for logs)",
        cfg.base_url(),
        ENSURE_SERVER_MAX_WAIT.as_secs(),
        cfg.port
    ))
}

/// Start the server in the background if needed, then wait until `/health` is OK.
pub async fn ensure_server(cfg: &ServerConfig) -> Result<()> {
    if server_is_up(cfg).await {
        return Ok(());
    }
    let _ = crate::registry::default_serve_pack_paths()?;

    let exe = std::env::current_exe().context("current exe")?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("serve")
        .arg("--host")
        .arg(&cfg.host)
        .arg("--port")
        .arg(cfg.port.to_string())
        .env("MEMKIT_SERVE_FOREGROUND", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.spawn()
        .context("failed to start memkit server in background")?;

    wait_for_server_ready(cfg).await
}

/// One-line hint on stderr: server live + port (after a successful `ensure_server`).
pub fn print_server_note_running(cfg: &ServerConfig, output_json: bool) {
    if output_json {
        return;
    }
    let c = term::color_stderr();
    eprintln!(
        "{} {}",
        "Server is live",
        term::bracketed_cyan(c, &format!(":{}", cfg.port))
    );
}

/// One-line hint on stderr for `mk doctor`: port status if up, else how to start `mk serve`.
pub async fn print_server_note_doctor(cfg: &ServerConfig, output_json: bool) {
    if output_json {
        return;
    }
    let c = term::color_stderr();
    if server_is_up(cfg).await {
        return;
    }
    let hint = format!("mk serve --host {} --port {}", cfg.host, cfg.port);
    eprintln!(
        "{} {} {}",
        term::data_num(c, "server not running"),
        term::dimmed_word(c, "— start with:"),
        term::warn_words(c, &hint)
    );
}

/// Poll /status until the index job is no longer active (or timeout). Use after POST /add (add directory).
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
    let resp = client.get(url).timeout(REQ_TIMEOUT_DEFAULT).send().await?;
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
    let file_tree_raw = data.get("file_tree").and_then(Value::as_str).unwrap_or("");
    let file_tree = user_facing_file_tree(file_tree_raw);
    let pending_removal = data.get("pending_removal").and_then(Value::as_bool).unwrap_or(false);
    let pending_add = data.get("pending_add").and_then(Value::as_bool).unwrap_or(false);
    let jobs = data.get("jobs").and_then(Value::as_object);
    let active_job = jobs.and_then(|j| j.get("active")).map(|v| !v.is_null()).unwrap_or(false);
    let active_job_id = jobs.and_then(|j| j.get("active")).and_then(Value::as_object).and_then(|o| o.get("id")).and_then(Value::as_str);
    let queued_jobs = jobs.and_then(|j| j.get("queued_jobs")).and_then(Value::as_array).map(|a| a.as_slice()).unwrap_or(&[]);
    let last_job = jobs.and_then(|j| j.get("last_completed")).and_then(Value::as_object);
    let last_job_failed = last_job.and_then(|j| j.get("state")).and_then(Value::as_str) == Some("Failed");
    let last_job_error = last_job.and_then(|j| j.get("error")).and_then(Value::as_str);
    let last_job_id = last_job.and_then(|j| j.get("id")).and_then(Value::as_str);

    let c = term::color_stdout();
    if c {
        if pending_removal {
            println!(
                "{} {}",
                term::bold_word(c, pack_path),
                term::warn_words(c, "removing...")
            );
        } else if pending_add {
            let id = active_job_id.unwrap_or("?");
            println!(
                "{} {} {}",
                term::bold_word(c, pack_path),
                term::dimmed_word(c, id),
                term::warn_words(c, "...pending")
            );
        } else if active_job {
            let id = active_job_id.unwrap_or("?");
            println!(
                "{} {} {}",
                term::bold_word(c, pack_path),
                term::dimmed_word(c, id),
                term::warn_words(c, "...pending")
            );
        } else if indexed {
            println!(
                "{} successfully indexed",
                term::bold_green(c, pack_path)
            );
        } else {
            println!("{} not indexed", term::bold_yellow(c, pack_path));
        }
        println!("{}", term::sync_local_only_label(c));
        if !file_tree.is_empty() {
            println!();
            println!("{}", term::dimmed_word(c, &file_tree));
        }
        println!();
        println!(
            "{} vector entries",
            term::data_num(c, vector_count)
        );
        println!(
            "{} entities, {} relationships",
            term::data_num(c, entities),
            term::data_num(c, relationships)
        );
        for q in queued_jobs {
            if let Some(id) = q.get("id").and_then(Value::as_str) {
                println!(
                    "  {} {}",
                    term::dimmed_word(c, id),
                    term::warn_words(c, "...pending")
                );
            }
        }
        if !indexed && last_job_failed {
            if let Some(err) = last_job_error {
                let id = last_job_id.unwrap_or("job");
                println!(
                    "  {} {}",
                    term::dimmed_word(c, id),
                    term::danger_words(c, &format!("failed: {}", err))
                );
            }
        }
    } else {
        if pending_removal {
            println!("{} removing...", pack_path);
        } else if pending_add || active_job {
            let id = active_job_id.unwrap_or("?");
            println!("{} {} ...pending", pack_path, id);
        } else if indexed {
            println!("{} successfully indexed", pack_path);
        } else {
            println!("{} not indexed", pack_path);
        }
        println!("sync: local only");
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
        if !indexed && last_job_failed {
            if let Some(err) = last_job_error {
                let id = last_job_id.unwrap_or("job");
                println!("  {} failed: {}", id, err);
            }
        }
    }
}

pub async fn list(cfg: &ServerConfig, output_json: bool) -> Result<Value> {
    let _ = crate::registry::ensure_default_if_unset();
    let reg = crate::registry::load_registry().unwrap_or_default();
    if reg.packs.is_empty() {
        let data = status(cfg, None).await?;
        if !output_json {
            let pack_path = data
                .get("pack_path")
                .and_then(Value::as_str)
                .unwrap_or("?");
            let indexed = data.get("indexed").and_then(Value::as_bool).unwrap_or(false);
            let vector_count = data.get("vector_count").and_then(Value::as_u64).unwrap_or(0);
            let indexed_here = indexed && vector_count > 0;
            let local_on = indexed_here;
            let cloud_on = false;
            let active_job = data.get("jobs").and_then(|j| j.get("active"));

            let c = term::color_stdout();
            let tags = bracket_local_cloud(c, local_on, cloud_on);
            let label = "(default)";
            let line = if c {
                format!(
                    "{} {} {}",
                    term::white_word(c, pack_path),
                    tags,
                    term::cyan_label(c, label)
                )
            } else {
                format!("{} {} {}", pack_path, tags, label)
            };
            println!("{}", line);
            if let Some(obj) = active_job.and_then(Value::as_object) {
                let job_id = obj.get("id").and_then(Value::as_str).unwrap_or("?");
                if c {
                    println!(
                        "  {} {}",
                        term::dimmed_word(c, job_id),
                        term::warn_words(c, "...pending")
                    );
                } else {
                    println!("  {} ...pending", job_id);
                }
            }
        }
        return Ok(data);
    }

    if !output_json {
        let c = term::color_stdout();
        let home_canon = dirs::home_dir().and_then(|h| h.canonicalize().ok());
        let max_path_w = reg
            .packs
            .iter()
            .map(|p| pack_path_display(p, &home_canon).len())
            .max()
            .unwrap_or(0);
        for p in &reg.packs {
            let path_display = pack_path_display(p, &home_canon);
            let padded_path = format!("{:<width$}", path_display, width = max_path_w);
            let paren = pack_list_paren_label(p, &reg, &home_canon);
            if let Ok(data) = status(cfg, Some(&p.path)).await {
                let indexed = data.get("indexed").and_then(Value::as_bool).unwrap_or(false);
                let vector_count = data.get("vector_count").and_then(Value::as_u64).unwrap_or(0);
                let indexed_here = indexed && vector_count > 0;
                let local_on = indexed_here && p.local;
                let cloud_on = indexed_here && p.cloud;
                let tags = bracket_local_cloud(c, local_on, cloud_on);
                if c {
                    if let Some(ref lab) = paren {
                        println!(
                            "{} {} {}",
                            term::white_word(c, &padded_path),
                            tags,
                            term::cyan_label(c, lab)
                        );
                    } else {
                        println!("{} {}", term::white_word(c, &padded_path), tags);
                    }
                } else if let Some(ref lab) = paren {
                    println!("{} {} {}", padded_path, tags, lab);
                } else {
                    println!("{} {}", padded_path, tags);
                }
                let active_job = data.get("jobs").and_then(|j| j.get("active"));
                if let Some(obj) = active_job.and_then(Value::as_object) {
                    let job_id = obj.get("id").and_then(Value::as_str).unwrap_or("?");
                    if c {
                        println!(
                            "  {} {}",
                            term::dimmed_word(c, job_id),
                            term::warn_words(c, "...pending")
                        );
                    } else {
                        println!("  {} ...pending", job_id);
                    }
                }
                let queued_jobs = data.get("jobs").and_then(|j| j.get("queued_jobs")).and_then(Value::as_array).map(|a| a.as_slice()).unwrap_or(&[]);
                for q in queued_jobs {
                    if let Some(id) = q.get("id").and_then(Value::as_str) {
                        if c {
                            println!(
                                "  {} {}",
                                term::dimmed_word(c, id),
                                term::warn_words(c, "...pending")
                            );
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
                let remove_for_pack = |j: &Value| {
                    j.get("job_type").and_then(Value::as_str) == Some("remove_pack")
                        && j.get("pack_path").and_then(Value::as_str) == Some(p.path.as_str())
                };
                let is_remove_for_active = active_obj.as_ref().map_or(false, |o| {
                    o.get("job_type").and_then(Value::as_str) == Some("remove_pack")
                        && o.get("pack_path").and_then(Value::as_str) == Some(p.path.as_str())
                });
                let is_remove_job_for_this_pack =
                    is_remove_for_active || queued_jobs.iter().any(remove_for_pack);
                let status_line = if is_remove_job_for_this_pack {
                    Some("removing...".to_string())
                } else if let Some(ref obj) = active_obj {
                    let id = obj.get("id").and_then(Value::as_str).unwrap_or("?");
                    Some(format!("indexing ({}) — {}", id, counts_suffix))
                } else if !indexed {
                    Some(format!("not indexed ({})", counts_suffix))
                } else {
                    None
                };
                if let Some(ref line) = status_line {
                    if c {
                        if active_obj.is_some() {
                            println!("  {}", term::warn_words(c, line));
                        } else {
                            println!("  {}", term::dimmed_word(c, line));
                        }
                    } else {
                        println!("  {}", line);
                    }
                }
            } else {
                let tags = bracket_local_cloud(c, false, false);
                if c {
                    if let Some(ref lab) = paren {
                        println!(
                            "{} {} {}",
                            term::white_word(c, &padded_path),
                            tags,
                            term::cyan_label(c, lab)
                        );
                    } else {
                        println!("{} {}", term::white_word(c, &padded_path), tags);
                    }
                } else if let Some(ref lab) = paren {
                    println!("{} {} {}", padded_path, tags, lab);
                } else {
                    println!("{} {}", padded_path, tags);
                }
            }
        }
    }
    Ok(json!({"packs": reg.packs}))
}

pub async fn remove(cfg: &ServerConfig, path: &str) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/remove", cfg.base_url());
    let body = json!({ "path": path });
    let resp = client
        .post(url)
        .json(&body)
        .timeout(REQ_TIMEOUT_INDEX)
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        let msg = if body.is_empty() {
            format!("remove request failed: HTTP {} (empty response). If you recently updated, try stopping any running mk server and run the command again.", status.as_u16())
        } else {
            format!("remove request failed: {}", body)
        };
        return Err(anyhow!("{}", msg));
    }
    Ok(serde_json::from_str(&body)?)
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
    let resp = match client
        .post(url)
        .json(&body)
        .timeout(REQ_TIMEOUT_DEFAULT)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            let connection_error = msg.contains("connection closed")
                || msg.contains("connection refused")
                || msg.contains("failed to connect");
            return Err(if connection_error {
                anyhow!(e).context(
                    "Could not reach the memkit server. If it was stopped, run `mk query` or `mk serve` again.",
                )
            } else {
                anyhow!(e)
            });
        }
    };
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("query request failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
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
        .timeout(REQ_TIMEOUT_DEFAULT)
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
            let c = term::color_stdout();
            println!("{} {}", term::success_words(c, "Published to"), uri);
        }
    }
    Ok(out)
}

pub async fn add(cfg: &ServerConfig, body: &serde_json::Value) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/add", cfg.base_url());
    let resp = client
        .post(url)
        .json(body)
        .timeout(REQ_TIMEOUT_DEFAULT)
        .send()
        .await?;
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

/// Print add result: "Added N chunks." when synchronous success, or "Adding (job-N)..." when async job.
pub fn print_add_started(data: &Value, pack_path: &str) {
    let c = term::color_stdout();
    if let Some(n) = data.get("result").and_then(|r| r.get("chunks_added")).and_then(Value::as_u64) {
        println!(
            "{} {}",
            term::success_words(c, "Added"),
            term::data_num(c, &format!("{} chunks.", n))
        );
        return;
    }
    if let Some(job_id) = data.get("job").and_then(|j| j.get("id")).and_then(Value::as_str) {
        println!(
            "{} ({}). Run 'mk status {}' to check progress.",
            term::success_words(c, "Adding"),
            job_id,
            pack_path
        );
    }
}
