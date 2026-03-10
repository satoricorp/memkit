use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use owo_colors::OwoColorize;
use serde_json::{Value, json};

use crate::term;

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

async fn server_is_up(cfg: &ServerConfig) -> bool {
    let Ok(client) = http_client() else {
        return false;
    };
    let url = format!("{}/health", cfg.base_url());
    match client.get(url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

fn candidate_server_start_script() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("MEMKIT_SERVER_START_SCRIPT") {
        let p = PathBuf::from(custom);
        if p.exists() {
            return Some(p);
        }
    }

    let from_cwd = PathBuf::from("scripts/local-start.sh");
    if from_cwd.exists() {
        return Some(from_cwd);
    }

    let from_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .map(|p| p.join("../../scripts/local-start.sh"))
        .map(|p| p.canonicalize().unwrap_or(p));
    from_exe.filter(|p| p.exists())
}

fn run_server_start() -> Result<()> {
    let script = candidate_server_start_script().ok_or_else(|| {
        anyhow!(
            "could not locate server start script. run `./scripts/local-start.sh` or `mk serve` manually"
        )
    })?;
    let status = Command::new("sh")
        .arg(&script)
        .status()
        .with_context(|| format!("failed to execute {}", script.display()))?;
    if !status.success() {
        return Err(anyhow!(
            "server auto-start failed (exit {}). run `./scripts/local-start.sh` or `mk serve` manually",
            status
        ));
    }
    Ok(())
}

pub async fn ensure_server(cfg: &ServerConfig) -> Result<()> {
    if server_is_up(cfg).await {
        return Ok(());
    }
    run_server_start()?;

    for _ in 0..12 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if server_is_up(cfg).await {
            return Ok(());
        }
    }

    Err(anyhow!(
        "server did not become healthy after auto-start on {}:{}",
        cfg.host,
        cfg.port
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

    if term::color_stdout() {
        if active_job {
            println!("{} indexing...", pack_path.bold());
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
    } else {
        if active_job {
            println!("{} indexing...", pack_path);
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
    }
}

pub async fn list(cfg: &ServerConfig) -> Result<Value> {
    let reg = crate::registry::load_registry().unwrap_or_default();
    if reg.packs.is_empty() {
        let data = status(cfg, None).await?;
        let pack_path = data
            .get("pack_path")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let sources = data.get("sources").and_then(Value::as_array).cloned().unwrap_or_default();
        let jobs = data.get("jobs").and_then(Value::as_object);
        let active_job = jobs.and_then(|j| j.get("active")).map(|v| !v.is_null()).unwrap_or(false);

        if term::color_stdout() {
            println!("{} [local] [cloud]", pack_path.bold());
            for s in sources.iter().take(10) {
                let path = s.get("root_path").and_then(Value::as_str).unwrap_or("?");
                println!("  {}", path.dimmed());
            }
            if active_job {
                println!("{}", "  (indexing...)".yellow());
            }
        } else {
            println!("{} [local] [cloud]", pack_path);
            for s in sources.iter().take(10) {
                let path = s.get("root_path").and_then(Value::as_str).unwrap_or("?");
                println!("  {}", path);
            }
            if active_job {
                println!("  (indexing...)");
            }
        }
        return Ok(data);
    }

    for p in &reg.packs {
        let default_marker = if p.default { " (default)" } else { "" };
        if term::color_stdout() {
            let cloud = if p.cloud { format!("{}", "[cloud]".cyan()) } else { format!("{}", "cloud".dimmed()) };
            println!(
                "{} [local] {} {}",
                p.path.bold(),
                cloud,
                default_marker.dimmed()
            );
        } else {
            let cloud = if p.cloud { "[cloud]" } else { "cloud" };
            println!("{} [local] {} {}", p.path, cloud, default_marker);
        }
    }
    Ok(json!({"packs": reg.packs}))
}

pub async fn index(cfg: &ServerConfig, path: &str) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/index", cfg.base_url());
    let resp = client.post(url).json(&json!({"path": path})).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("index request failed: {}", body));
    }
    let out: Value = serde_json::from_str(&body)?;
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
    Ok(out)
}

pub async fn query(cfg: &ServerConfig, args: &QueryArgs, pack: Option<&str>) -> Result<Value> {
    let client = http_client()?;
    let url = format!("{}/query", cfg.base_url());
    let mut body = json!({
        "query": args.query,
        "mode": args.mode,
        "top_k": args.top_k,
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
