use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use owo_colors::OwoColorize;
use serde_json::{Value, json};

use crate::term;

pub struct DaemonConfig {
    pub host: String,
    pub port: u16,
}

impl DaemonConfig {
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
    pub mode: String,
    pub top_k: usize,
    pub raw: bool,
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("failed to build HTTP client")
}

async fn daemon_is_up(cfg: &DaemonConfig) -> bool {
    let Ok(client) = http_client() else {
        return false;
    };
    let url = format!("{}/health", cfg.base_url());
    match client.get(url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

fn candidate_daemon_start_script() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("SATORI_DAEMON_START_SCRIPT") {
        let p = PathBuf::from(custom);
        if p.exists() {
            return Some(p);
        }
    }

    let from_cwd = PathBuf::from("scripts/daemon-start.sh");
    if from_cwd.exists() {
        return Some(from_cwd);
    }

    let from_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .map(|p| p.join("../../scripts/daemon-start.sh"))
        .map(|p| p.canonicalize().unwrap_or(p));
    from_exe.filter(|p| p.exists())
}

fn run_daemon_start() -> Result<()> {
    let script = candidate_daemon_start_script().ok_or_else(|| {
        anyhow!(
            "could not locate daemon-start script; tried scripts/daemon-start.sh. run `bun run daemon:start` manually"
        )
    })?;
    let status = Command::new("sh")
        .arg(&script)
        .status()
        .with_context(|| format!("failed to execute {}", script.display()))?;
    if !status.success() {
        return Err(anyhow!(
            "daemon auto-start failed (exit {}). run `bun run daemon:start` manually",
            status
        ));
    }
    Ok(())
}

pub async fn ensure_daemon(cfg: &DaemonConfig) -> Result<()> {
    if daemon_is_up(cfg).await {
        return Ok(());
    }
    run_daemon_start()?;

    for _ in 0..12 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if daemon_is_up(cfg).await {
            return Ok(());
        }
    }

    Err(anyhow!(
        "daemon did not become healthy after auto-start on {}:{}",
        cfg.host,
        cfg.port
    ))
}

pub async fn status(cfg: &DaemonConfig) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/status", cfg.base_url());
    let resp = client.get(url).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("status request failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}

pub async fn index(cfg: &DaemonConfig) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/index", cfg.base_url());
    let resp = client.post(url).json(&json!({})).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("index request failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}

pub async fn query(cfg: &DaemonConfig, args: &QueryArgs) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/query", cfg.base_url());
    let resp = client
        .post(url)
        .json(&json!({
            "query": args.query,
            "mode": args.mode,
            "top_k": args.top_k,
            "raw": args.raw
        }))
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("query request failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}

pub async fn sources_list(cfg: &DaemonConfig) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/sources", cfg.base_url());
    let resp = client.get(url).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("sources list failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}

pub async fn sources_add(cfg: &DaemonConfig, path: &str) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/sources", cfg.base_url());
    let resp = client.post(url).json(&json!({"path": path})).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("sources add failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}

pub async fn sources_remove(cfg: &DaemonConfig, path: &str) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/sources", cfg.base_url());
    let resp = client.delete(url).json(&json!({"path": path})).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("sources remove failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}

pub async fn ontology_list(cfg: &DaemonConfig) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/ontology/sources", cfg.base_url());
    let resp = client.get(url).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("ontology list failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}

pub async fn ontology_show(cfg: &DaemonConfig, source: &str) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/ontology/source", cfg.base_url());
    let resp = client.get(url).query(&[("path", source)]).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        if let Ok(parsed) = serde_json::from_str::<Value>(&body) {
            let code = parsed
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN");
            let message = parsed
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("ontology lookup failed");
            let mut details = format!("ontology show failed [{code}]: {message}");
            let suggestions = parsed
                .get("error")
                .and_then(|e| e.get("suggestions"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if !suggestions.is_empty() {
                details.push_str("\nSuggestions:");
                for (idx, item) in suggestions.iter().take(5).enumerate() {
                    if let Some(source_path) = item.get("source_path").and_then(Value::as_str) {
                        details.push_str(&format!(
                            "\n  {}. {}",
                            idx + 1,
                            source_path
                        ));
                        details.push_str(&format!(
                            "\n     try: satori ontology show --source \"{}\"",
                            source_path
                        ));
                    }
                }
            }
            return Err(anyhow!(details));
        }
        return Err(anyhow!("ontology show failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}

pub async fn graph_show(cfg: &DaemonConfig) -> Result<()> {
    let url = format!("{}/graph/view", cfg.base_url());
    opener::open(url).context("failed to open graph view in browser")?;
    Ok(())
}

pub async fn pack(cfg: &DaemonConfig) -> Result<()> {
    let data = status(cfg).await?;
    let pack_path = data
        .get("pack_path")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let indexed = data.get("indexed").and_then(Value::as_bool).unwrap_or(false);
    let status_str = if indexed { "indexed" } else { "not indexed" };
    if term::color_stdout() {
        println!("{} {}", "pack:".dimmed(), pack_path.bold());
        if indexed {
            println!("{} {}", "status:".dimmed(), "indexed".green());
        } else {
            println!("{} {}", "status:".dimmed(), "not indexed".yellow());
        }
    } else {
        println!("pack: {}", pack_path);
        println!("status: {}", status_str);
    }
    Ok(())
}

pub async fn jobs_list(cfg: &DaemonConfig) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/jobs", cfg.base_url());
    let resp = client.get(url).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("jobs list failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}

pub async fn jobs_status(cfg: &DaemonConfig, id: &str) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/jobs/{}", cfg.base_url(), id);
    let resp = client.get(url).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("job status failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}
