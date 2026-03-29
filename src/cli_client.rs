use std::collections::BTreeMap;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use serde_json::{Value, json};
use urlencoding::encode;

use crate::cloud::{CloudPackSummary, is_memkit_uri};
use crate::pack::{load_manifest, resolve_pack_dir};
use crate::publish::{build_cloud_publish_archive, build_cloud_publish_archive_with_pack_id};
use crate::registry::{Registry, RegistryPack, resolve_pack_by_name_or_path};
use crate::term;

/// True if registry path and job `pack_path` refer to the same directory (canonicalize when possible).
fn registry_job_pack_paths_match(registry_path: &str, job_pack_path: &str) -> bool {
    if registry_path == job_pack_path {
        return true;
    }
    match (
        PathBuf::from(registry_path).canonicalize(),
        PathBuf::from(job_pack_path).canonicalize(),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Inner text for `[…]` in server banner (name, `default`, or directory name).
/// Pack to show for the server banner: explicit `default_path`, or a pack with `default`, or the sole pack.
fn resolve_default_pack<'a>(reg: &'a Registry) -> Option<&'a RegistryPack> {
    if reg.packs.is_empty() {
        return None;
    }
    if let Some(ref dp) = reg.default_path {
        if let Some(pack) = reg.packs.iter().find(|p| p.local_path() == Some(dp.as_str())) {
            return Some(pack);
        }
    }
    if let Some(p) = reg.packs.iter().find(|p| p.default) {
        return Some(p);
    }
    let locals: Vec<_> = reg.packs.iter().filter(|p| p.local_path().is_some()).collect();
    if locals.len() == 1 {
        return locals.first().copied();
    }
    if reg.packs.len() == 1 {
        return reg.packs.first();
    }
    None
}

fn pack_bracket_inner(p: &RegistryPack, reg: &Registry, home_canon: &Option<PathBuf>) -> String {
    if let Some(ref n) = p.name {
        return n.clone();
    }
    let path_is_home = p
        .local_path()
        .and_then(|path| PathBuf::from(path).canonicalize().ok())
        .as_ref()
        == home_canon.as_ref();
    let is_default_pack = p.default
        || reg.default_path.as_deref() == p.local_path()
        || reg.packs.iter().filter(|pack| pack.local_path().is_some()).count() == 1
        || (path_is_home && reg.default_path.is_none());
    if is_default_pack {
        return "default".to_string();
    }
    if let Some(local_path) = p.local_path() {
        return PathBuf::from(local_path)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "pack".to_string());
    }
    "pack".to_string()
}

/// Plain-text output for [`list`]: bare `mk status` (banner only) vs `mk list` (banner + packs).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListOutputKind {
    Status,
    Full,
}

#[derive(Clone, Debug, Serialize)]
struct PackListEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pack_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cloud_name: Option<String>,
    #[serde(default)]
    default: bool,
    #[serde(default)]
    local: bool,
    #[serde(default)]
    cloud: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cloud_uri: Option<String>,
}

#[derive(Clone, Debug)]
struct MergedPackView {
    entry: PackListEntry,
}

impl MergedPackView {
    fn display_pack_id(&self) -> String {
        self.entry
            .pack_id
            .clone()
            .or_else(|| self.entry.cloud_name.clone())
            .or_else(|| self.entry.local_name.clone())
            .unwrap_or_else(|| "unknown-pack".to_string())
    }

    fn local_path(&self) -> Option<&str> {
        self.entry.local_path.as_deref()
    }

    fn cloud_uri(&self) -> Option<&str> {
        self.entry.cloud_uri.as_deref()
    }
}

fn registry_pack_id(pack: &RegistryPack) -> Option<String> {
    let local_path = pack.local_path()?;
    let pack_dir = resolve_pack_dir(&PathBuf::from(local_path));
    load_manifest(&pack_dir).ok().map(|manifest| manifest.pack_id)
}

fn build_pack_views(reg: &Registry, cloud_packs: &[CloudPackSummary]) -> Vec<MergedPackView> {
    let mut merged: BTreeMap<String, MergedPackView> = BTreeMap::new();

    for pack in &reg.packs {
        let Some(local_path) = pack.local_path() else {
            continue;
        };
        let pack_id = registry_pack_id(pack);
        let key = pack_id
            .clone()
            .unwrap_or_else(|| format!("local:{}", local_path));
        let view = merged.entry(key).or_insert_with(|| MergedPackView {
            entry: PackListEntry {
                pack_id: pack_id.clone(),
                local_name: None,
                cloud_name: None,
                default: false,
                local: false,
                cloud: false,
                local_path: None,
                cloud_uri: None,
            },
        });
        if view.entry.pack_id.is_none() {
            view.entry.pack_id = pack_id;
        }
        view.entry.local = true;
        view.entry.local_path = Some(local_path.to_string());
        view.entry.local_name = pack.name.clone();
        view.entry.default =
            pack.default || reg.default_path.as_deref() == Some(local_path);
    }

    for summary in cloud_packs {
        let view = merged
            .entry(summary.pack_id.clone())
            .or_insert_with(|| MergedPackView {
                entry: PackListEntry {
                    pack_id: Some(summary.pack_id.clone()),
                    local_name: None,
                    cloud_name: None,
                    default: false,
                    local: false,
                    cloud: false,
                    local_path: None,
                    cloud_uri: None,
                },
            });
        if view.entry.pack_id.is_none() {
            view.entry.pack_id = Some(summary.pack_id.clone());
        }
        view.entry.cloud = true;
        view.entry.cloud_uri = Some(summary.pack_uri.clone());
        view.entry.cloud_name = summary.display_name.clone();
    }

    let mut views: Vec<_> = merged.into_values().collect();
    views.sort_by(|a, b| {
        b.entry
            .default
            .cmp(&a.entry.default)
            .then_with(|| b.entry.local.cmp(&a.entry.local))
            .then_with(|| a.display_pack_id().cmp(&b.display_pack_id()))
    });
    views
}

fn resolve_default_pack_view<'a>(views: &'a [MergedPackView], reg: &Registry) -> Option<&'a MergedPackView> {
    if let Some(ref default_path) = reg.default_path {
        if let Some(view) = views
            .iter()
            .find(|view| view.local_path() == Some(default_path.as_str()))
        {
            return Some(view);
        }
    }
    views.iter().find(|view| view.entry.default)
}

fn maybe_name_line(name: Option<&str>, pack_id: Option<&str>) -> Option<String> {
    let name = name?.trim();
    if name.is_empty() || Some(name) == pack_id {
        return None;
    }
    Some(name.to_string())
}

fn print_pack_metadata_lines(c: bool, view: &MergedPackView, local_path_display: Option<&str>) {
    if let Some(path) = local_path_display {
        if c {
            println!("    - {}", term::white_word(c, path));
        } else {
            println!("    - {}", path);
        }
    }
    if let Some(cloud_uri) = view.cloud_uri() {
        if c {
            println!("    - {}", term::white_word(c, cloud_uri));
        } else {
            println!("    - {}", cloud_uri);
        }
    }
    if let Some(local_name) = maybe_name_line(
        view.entry.local_name.as_deref(),
        view.entry.pack_id.as_deref(),
    ) {
        let line = format!("local name: {}", local_name);
        if c {
            println!("    - {}", term::dimmed_word(c, &line));
        } else {
            println!("    - {}", line);
        }
    }
    if let Some(cloud_name) = maybe_name_line(
        view.entry.cloud_name.as_deref(),
        view.entry.pack_id.as_deref(),
    ) {
        if Some(cloud_name.as_str()) != view.entry.local_name.as_deref() {
            let line = format!("cloud name: {}", cloud_name);
            if c {
                println!("    - {}", term::dimmed_word(c, &line));
            } else {
                println!("    - {}", line);
            }
        }
    }
}

fn bracket_local_cloud(c: bool, local_on: bool, cloud_on: bool) -> String {
    let local = term::bracket_tag_cyan_when_on(c, local_on, "local");
    let cloud = term::bracket_tag_cyan_when_on(c, cloud_on, "cloud");
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

fn cloud_base_url() -> String {
    crate::config::resolve_cloud_url()
}

fn cloud_request(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let mut builder = req;
    if let Ok(cfg) = crate::config::load_config() {
        if let Some(auth) = cfg.auth {
            if let Some(user_id) = auth.profile.user_id {
                builder = builder.header("x-memkit-user-id", user_id.to_string());
            }
            if let Some(org_id) = auth.profile.org_id {
                builder = builder.header("x-memkit-org-id", org_id.to_string());
            }
            if !auth.jwt.trim().is_empty() {
                builder = builder.bearer_auth(auth.jwt);
            }
        }
    }
    builder
}

fn preferred_cloud_tenant(
) -> Result<(crate::cloud::CloudTenantKind, String), anyhow::Error> {
    let cfg = crate::config::load_config()?;
    let auth = cfg
        .auth
        .ok_or_else(|| anyhow!("login required before publishing to the cloud"))?;
    let prefer_org = matches!(
        std::env::var("MEMKIT_CLOUD_TENANT").ok().as_deref(),
        Some("org" | "orgs")
    );
    if prefer_org {
        if let Some(org_id) = auth.profile.org_id {
            return Ok((crate::cloud::CloudTenantKind::Orgs, org_id.to_string()));
        }
    }
    if let Some(user_id) = auth.profile.user_id {
        return Ok((crate::cloud::CloudTenantKind::Users, user_id.to_string()));
    }
    if let Some(org_id) = auth.profile.org_id {
        return Ok((crate::cloud::CloudTenantKind::Orgs, org_id.to_string()));
    }
    Err(anyhow!("login required before publishing to the cloud"))
}

async fn fetch_cloud_packs() -> Result<Vec<CloudPackSummary>> {
    let auth = crate::config::load_config().ok().and_then(|cfg| cfg.auth);
    if auth.is_none() {
        return Ok(Vec::new());
    }
    let client = http_client()?;
    let req = cloud_request(client.get(format!("{}/packs", cloud_base_url())));
    let resp = req.timeout(REQ_TIMEOUT_DEFAULT).send().await?;
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body = resp.text().await?;
    let data: Value = serde_json::from_str(&body)?;
    let summaries = serde_json::from_value(
        data.get("packs")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )?;
    Ok(summaries)
}

fn cloud_pack_id_conflict<'a>(
    cloud_packs: &'a [CloudPackSummary],
    pack_id: &str,
) -> Option<&'a CloudPackSummary> {
    cloud_packs.iter().find(|summary| summary.pack_id == pack_id)
}

fn default_cloud_pack_uri_for_id(pack_id: &str) -> Result<String> {
    let (tenant_kind, tenant_id) = preferred_cloud_tenant()?;
    Ok(format!(
        "memkit://{}/{}/packs/{}",
        tenant_kind.as_str(),
        tenant_id,
        pack_id
    ))
}

fn default_cloud_pack_uri(manifest: &crate::types::Manifest) -> Result<String> {
    default_cloud_pack_uri_for_id(&manifest.pack_id)
}

enum QueryTarget {
    Local(Option<String>),
    Cloud(String),
}

#[derive(Clone)]
struct LocalQueryPack {
    path: String,
    pack_id: Option<String>,
}

fn resolve_local_pack_path_by_id(pack_id: &str) -> Result<Option<String>> {
    let reg = crate::registry::load_registry().unwrap_or_default();
    for pack in &reg.packs {
        if registry_pack_id(pack).as_deref() == Some(pack_id) {
            if let Some(local_path) = pack.local_path() {
                let canonical = PathBuf::from(local_path)
                    .canonicalize()
                    .with_context(|| format!("pack path no longer exists: {}", local_path))?;
                return Ok(Some(canonical.to_string_lossy().to_string()));
            }
        }
    }
    Ok(None)
}

fn resolve_local_query_pack(selector: &str) -> Result<Option<LocalQueryPack>> {
    if let Ok(path) = resolve_pack_by_name_or_path(selector) {
        let pack_id = load_manifest(&resolve_pack_dir(&path)).ok().map(|m| m.pack_id);
        return Ok(Some(LocalQueryPack {
            path: path.to_string_lossy().to_string(),
            pack_id,
        }));
    }
    if let Some(local_path) = resolve_local_pack_path_by_id(selector)? {
        return Ok(Some(LocalQueryPack {
            path: local_path,
            pack_id: Some(selector.to_string()),
        }));
    }
    Ok(None)
}

async fn resolve_cloud_pack_summary(selector: &str) -> Result<Option<CloudPackSummary>> {
    let cloud_packs = fetch_cloud_packs().await?;
    if let Some(summary) = cloud_packs
        .iter()
        .find(|summary| summary.pack_id == selector)
        .cloned()
    {
        return Ok(Some(summary));
    }
    let mut name_matches = cloud_packs
        .into_iter()
        .filter(|summary| summary.display_name.as_deref() == Some(selector));
    let first = name_matches.next();
    if name_matches.next().is_some() {
        anyhow::bail!(
            "multiple cloud packs are named {}. Use a pack_id or memkit:// URI",
            selector
        );
    }
    Ok(first)
}

async fn resolve_query_target(pack: Option<&str>, prefer_cloud: bool) -> Result<QueryTarget> {
    let Some(raw) = pack else {
        if prefer_cloud {
            anyhow::bail!("--cloud requires --pack");
        }
        return Ok(QueryTarget::Local(None));
    };
    if is_memkit_uri(raw) {
        return Ok(QueryTarget::Cloud(raw.to_string()));
    }
    let local_pack = resolve_local_query_pack(raw)?;
    if prefer_cloud {
        if let Some(ref local_pack) = local_pack {
            if let Some(ref pack_id) = local_pack.pack_id {
                if let Some(summary) = resolve_cloud_pack_summary(pack_id).await? {
                    return Ok(QueryTarget::Cloud(summary.pack_uri));
                }
            }
        }
        if let Some(summary) = resolve_cloud_pack_summary(raw).await? {
            return Ok(QueryTarget::Cloud(summary.pack_uri));
        }
        anyhow::bail!(
            "no cloud pack found for {}. Use a pack_id or memkit:// URI if the name is ambiguous",
            raw
        );
    }
    if let Some(local_pack) = local_pack {
        return Ok(QueryTarget::Local(Some(local_pack.path)));
    }
    if let Some(summary) = resolve_cloud_pack_summary(raw).await? {
        return Ok(QueryTarget::Cloud(summary.pack_uri));
    }
    anyhow::bail!(
        "no local or cloud pack found for {}. Use a local pack name/path, a pack_id, or a memkit:// URI",
        raw
    )
}

fn resolve_local_publish_root(pack: Option<&str>) -> Result<(RegistryPack, PathBuf, PathBuf)> {
    let registry_pack = crate::registry::resolve_registry_pack(pack)?;
    let local_path = registry_pack
        .local_path()
        .ok_or_else(|| anyhow!("publish requires a local pack"))?;
    let pack_root = PathBuf::from(local_path)
        .canonicalize()
        .context("local pack path no longer exists")?;
    let pack_dir = resolve_pack_dir(&pack_root);
    Ok((registry_pack, pack_root, pack_dir))
}

#[derive(Clone)]
pub struct QueryArgs {
    pub query: String,
    pub top_k: usize,
    pub use_reranker: bool,
    pub raw: bool,
    pub cloud: bool,
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

/// Fail if `/health` is not OK — does **not** start a background server (use for `mk status` / `mk list`).
pub async fn require_server_running(cfg: &ServerConfig) -> Result<()> {
    if server_is_up(cfg).await {
        return Ok(());
    }
    Err(anyhow!(
        "memkit server is not running at {}. Start it with `mk start` or `mk start --foreground`.",
        cfg.base_url()
    ))
}

fn tcp_listen_pids(port: u16) -> Result<Vec<String>> {
    let output = std::process::Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-t"])
        .output()
        .context("lsof failed")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.trim().split_whitespace().map(String::from).collect())
}

/// SIGTERM listeners on `port`, then SIGKILL any still listening after a short wait.
pub fn stop_server_on_port(port: u16) -> Result<bool> {
    let mut pids = tcp_listen_pids(port)?;
    if pids.is_empty() {
        return Ok(false);
    }
    for pid in &pids {
        let _ = std::process::Command::new("kill").arg(pid).status();
    }
    thread::sleep(Duration::from_millis(400));
    pids = tcp_listen_pids(port)?;
    for pid in &pids {
        let _ = std::process::Command::new("kill")
            .args(["-9", pid])
            .status();
    }
    Ok(true)
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
        "memkit server did not become ready at {} within {}s (check port {} or run `mk start --foreground` for logs)",
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
    cmd.arg("start")
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

/// Two-line hint on stderr: green `⏺ Server` + `[url]` (magenta brackets, dimmed URL), green `⏺ Pack` + cyan `[name]` + cyan `[local]`/`[cloud]` when on.
pub async fn print_server_note_running(cfg: &ServerConfig, output_json: bool) {
    if output_json {
        return;
    }
    let c = term::color_stderr();
    let server_up = server_is_up(cfg).await;

    let _ = crate::registry::ensure_default_if_unset();
    let reg = crate::registry::load_registry().unwrap_or_default();
    let home_canon = dirs::home_dir().and_then(|h| h.canonicalize().ok());
    let default_pack = resolve_default_pack(&reg);
    let pack_ok = server_up && default_pack.is_some();

    let server_prefix = term::bullet_green_word(c, server_up, "Server");
    let url_bracket = term::bracket_url_line(c, server_up, &cfg.base_url());
    eprintln!("{} {}", server_prefix, url_bracket);

    let pack_prefix = term::bullet_green_word(c, pack_ok, "Pack");
    if let Some(p) = default_pack {
        let inner = pack_bracket_inner(p, &reg, &home_canon);
        let name_bracket = term::bracketed_cyan(c, &inner);
        let tags = if let Some(local_path) = p.local_path() {
            if server_up {
                if let Ok(data) = status(cfg, Some(local_path)).await {
                    let indexed = data
                        .get("indexed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let vector_count = data
                        .get("vector_count")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let indexed_here = indexed && vector_count > 0;
                    bracket_local_cloud(c, indexed_here, false)
                } else {
                    bracket_local_cloud(c, false, false)
                }
            } else {
                bracket_local_cloud(c, false, false)
            }
        } else {
            bracket_local_cloud(c, false, false)
        };
        eprintln!("{} {} {}", pack_prefix, name_bracket, tags);
    } else {
        eprintln!(
            "{} {}",
            pack_prefix,
            term::dimmed_word(c, "(no default pack)")
        );
    }
}

/// One-line hint on stderr for `mk doctor`: port status if up, else how to start `mk start`.
pub async fn print_server_note_doctor(cfg: &ServerConfig, output_json: bool) {
    if output_json {
        return;
    }
    let c = term::color_stderr();
    if server_is_up(cfg).await {
        return;
    }
    let hint = format!("mk start --host {} --port {}", cfg.host, cfg.port);
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
        let busy = data
            .get("pack_indexing_busy")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                data.get("jobs")
                    .and_then(|j| j.get("active"))
                    .map(|v| !v.is_null())
                    .unwrap_or(false)
            });
        if !busy {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(anyhow!(
        "index job did not complete within {}s",
        MAX_WAIT.as_secs()
    ))
}

/// After polling, replace `out["job"]` with `jobs.last_completed` when its `id` matches (POST /add
/// returns a snapshot taken at enqueue time; `last_completed` is the final job record).
pub fn merge_job_into_add_output_after_poll(out: &mut Value, status: &Value) {
    let Some(enqueued_id) = out
        .get("job")
        .and_then(|j| j.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return;
    };
    let Some(last) = status
        .get("jobs")
        .and_then(|j| j.get("last_completed"))
        .filter(|v| !v.is_null())
    else {
        return;
    };
    if last.get("id").and_then(Value::as_str) != Some(enqueued_id.as_str()) {
        return;
    }
    if let Some(obj) = out.as_object_mut() {
        obj.insert("job".to_string(), last.clone());
    }
}

pub async fn status(cfg: &ServerConfig, dir: Option<&str>) -> Result<Value> {
    let client = http_client()?;
    let mut url = format!("{}/status", cfg.base_url());
    if let Some(d) = dir {
        url.push_str(&format!("?path={}", encode(d)));
    }
    let resp = client.get(url).timeout(REQ_TIMEOUT_DEFAULT).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("status request failed: {}", body));
    }
    Ok(serde_json::from_str(&body)?)
}

/// Prints one line per path in `job.indexing_sources`. Returns true if any line was printed.
fn print_indexing_lines_from_job(c: bool, job: &Value, label: &str) -> bool {
    let bold = label == "indexing...";
    let Some(sources) = job.get("indexing_sources").and_then(Value::as_array) else {
        return false;
    };
    if sources.is_empty() {
        return false;
    }
    let mut printed = 0usize;
    for s in sources {
        if let Some(path) = s.as_str() {
            let suffix = if bold {
                term::bold_green(c, label)
            } else {
                term::dimmed_word(c, label)
            };
            println!("    - {} {}", term::white_word(c, path), suffix);
            printed += 1;
        }
    }
    printed > 0
}

/// Two-line stdout banner for `mk status` / `mk list` (replaces stderr `print_server_note_running` for those commands).
async fn print_cli_list_banner(
    cfg: &ServerConfig,
    c: bool,
    default_pack: Option<&MergedPackView>,
) {
    let server_up = server_is_up(cfg).await;
    let url_inner = cfg.base_url();
    let server_prefix = term::bullet_green_word(c, server_up, "Server");
    let url_bracket = term::bracket_url_line(c, server_up, &url_inner);
    println!("{} {}", server_prefix, url_bracket);

    let pack_ok = server_up && default_pack.is_some();
    let pack_prefix = term::bullet_green_word(c, pack_ok, "Pack");
    if let Some(p) = default_pack {
        let name_bracket = term::bracketed_cyan(c, &p.display_pack_id());
        let tags = if let Some(local_path) = p.local_path() {
            if let Ok(data) = status(cfg, Some(local_path)).await {
                let indexed = data
                    .get("indexed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let vector_count = data
                    .get("vector_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let indexed_here = indexed && vector_count > 0;
                let local_on = indexed_here;
                let cloud_on = p.cloud_uri().is_some();
                bracket_local_cloud(c, local_on, cloud_on)
            } else {
                bracket_local_cloud(c, false, p.cloud_uri().is_some())
            }
        } else {
            bracket_local_cloud(c, false, p.cloud_uri().is_some())
        };
        println!("{} {} {}", pack_prefix, name_bracket, tags);
    } else {
        println!(
            "{} {}",
            pack_prefix,
            term::dimmed_word(c, "(no default pack)")
        );
    }
}

pub fn print_status(data: &Value) {
    let pack_path = data.get("pack_path").and_then(Value::as_str).unwrap_or("?");
    let indexed = data
        .get("indexed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let vector_count = data
        .get("vector_count")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let entities = data.get("entities").and_then(Value::as_u64).unwrap_or(0) as usize;
    let relationships = data
        .get("relationships")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let file_tree_raw = data.get("file_tree").and_then(Value::as_str).unwrap_or("");
    let file_tree = user_facing_file_tree(file_tree_raw);
    let pending_removal = data
        .get("pending_removal")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let pending_add = data
        .get("pending_add")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let jobs = data.get("jobs").and_then(Value::as_object);
    let active_val = jobs.and_then(|j| {
        if j.contains_key("active_for_this_pack") {
            j.get("active_for_this_pack").filter(|v| !v.is_null())
        } else {
            j.get("active").filter(|v| !v.is_null())
        }
    });
    let active_job_id = active_val
        .and_then(Value::as_object)
        .and_then(|o| o.get("id"))
        .and_then(Value::as_str);
    let queued_for_pack = jobs
        .and_then(|j| {
            if j.contains_key("queued_jobs_for_this_pack") {
                j.get("queued_jobs_for_this_pack").and_then(Value::as_array)
            } else {
                j.get("queued_jobs").and_then(Value::as_array)
            }
        })
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    let queued_jobs = jobs
        .and_then(|j| j.get("queued_jobs"))
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    let last_job = jobs
        .and_then(|j| j.get("last_completed"))
        .and_then(Value::as_object);
    let last_job_failed = last_job
        .and_then(|j| j.get("state"))
        .and_then(Value::as_str)
        == Some("Failed");
    let last_job_error = last_job
        .and_then(|j| j.get("error"))
        .and_then(Value::as_str);
    let last_job_id = last_job.and_then(|j| j.get("id")).and_then(Value::as_str);
    let source_root_paths: Vec<String> = data
        .get("source_root_paths")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let index_warnings: Vec<String> = data
        .get("index_warnings")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let c = term::color_stdout();
    if c {
        if pending_removal {
            println!(
                "{} {}",
                term::bold_word(c, pack_path),
                term::warn_words(c, "removing...")
            );
        } else if pending_add {
            println!("{}", term::bold_word(c, pack_path));
            let mut printed_indexing = false;
            if let Some(av) = active_val {
                if print_indexing_lines_from_job(c, av, "indexing...") {
                    printed_indexing = true;
                } else {
                    let id = active_job_id.unwrap_or("?");
                    println!(
                        "  {} {}",
                        term::dimmed_word(c, id),
                        term::bold_green(c, "indexing...")
                    );
                    printed_indexing = true;
                }
            }
            for q in queued_for_pack {
                if print_indexing_lines_from_job(c, q, "queued...") {
                    printed_indexing = true;
                }
            }
            if !printed_indexing {
                let id = active_job_id.unwrap_or("?");
                println!(
                    "  {} {}",
                    term::dimmed_word(c, id),
                    term::warn_words(c, "...pending")
                );
            }
        } else if indexed {
            println!("{} successfully indexed", term::bold_green(c, pack_path));
        } else {
            println!("{} not indexed", term::bold_yellow(c, pack_path));
        }
        println!("{}", term::sync_local_only_label(c));
        if !file_tree.is_empty() {
            println!();
            println!("{}", term::dimmed_word(c, &file_tree));
        }
        println!();
        println!("{} vector entries", term::data_num(c, vector_count));
        println!(
            "{} entities, {} relationships",
            term::data_num(c, entities),
            term::data_num(c, relationships)
        );
        if !pending_add {
            for q in queued_jobs {
                if let Some(id) = q.get("id").and_then(Value::as_str) {
                    println!(
                        "  {} {}",
                        term::dimmed_word(c, id),
                        term::warn_words(c, "...pending")
                    );
                }
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
        for w in &index_warnings {
            println!("  {}", term::danger_words(c, w));
        }
        if !pending_add {
            for s in &source_root_paths {
                println!("  {}", term::dimmed_word(c, s));
            }
        }
    } else {
        if pending_removal {
            println!("{} removing...", pack_path);
        } else if pending_add {
            println!("{}", pack_path);
            let mut printed = false;
            if let Some(av) = active_val {
                if print_indexing_lines_from_job(c, av, "indexing...") {
                    printed = true;
                } else {
                    let id = active_job_id.unwrap_or("?");
                    println!("  {} indexing...", id);
                    printed = true;
                }
            }
            for q in queued_for_pack {
                if print_indexing_lines_from_job(c, q, "queued...") {
                    printed = true;
                }
            }
            if !printed {
                let id = active_job_id.unwrap_or("?");
                println!("  {} ...pending", id);
            }
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
        if !pending_add {
            for q in queued_jobs {
                if let Some(id) = q.get("id").and_then(Value::as_str) {
                    println!("  {} ...pending", id);
                }
            }
        }
        if !indexed && last_job_failed {
            if let Some(err) = last_job_error {
                let id = last_job_id.unwrap_or("job");
                println!("  {} failed: {}", id, err);
            }
        }
        for w in &index_warnings {
            println!("  {}", w);
        }
        if !pending_add {
            for s in &source_root_paths {
                println!("  {}", s);
            }
        }
    }
}

pub async fn list(cfg: &ServerConfig, output_json: bool, kind: ListOutputKind) -> Result<Value> {
    let _ = crate::registry::ensure_default_if_unset();
    let reg = crate::registry::load_registry().unwrap_or_default();
    let home_canon = dirs::home_dir().and_then(|h| h.canonicalize().ok());
    let cloud_packs = fetch_cloud_packs().await.unwrap_or_default();
    let views = build_pack_views(&reg, &cloud_packs);
    let default_pack = resolve_default_pack_view(&views, &reg);
    let serialized_packs: Vec<PackListEntry> = views.iter().map(|view| view.entry.clone()).collect();

    if views.is_empty() {
        if !output_json {
            let c = term::color_stdout();
            print_cli_list_banner(cfg, c, default_pack).await;
            match kind {
                ListOutputKind::Status => {
                    println!();
                    println!();
                }
                ListOutputKind::Full => {
                    println!();
                    println!("{}", term::section_title(c, "Packs"));
                    let msg = "No memory packs. Run `mk add <filename>` to add data to your default memory pack.";
                    if c {
                        println!("{}", term::dimmed_word(c, msg));
                    } else {
                        println!("{}", msg);
                    }
                }
            }
        }
        return Ok(json!({ "packs": serialized_packs }));
    }

    if !output_json {
        let c = term::color_stdout();
        print_cli_list_banner(cfg, c, default_pack).await;
        if kind == ListOutputKind::Status {
            println!();
            println!();
            return Ok(json!({ "packs": serialized_packs }));
        }
        println!();
        println!("{}", term::section_title(c, "Packs"));
        for view in &views {
            let local_path_display = view.local_path().map(|local_path| {
                let path_is_home =
                    PathBuf::from(local_path).canonicalize().ok().as_ref() == home_canon.as_ref();
                if path_is_home {
                    "~/.memkit".to_string()
                } else {
                    local_path.to_string()
                }
            });
            let line_prefix = if view.entry.default { "* " } else { "  " };
            let name_bracket = term::bracketed_cyan(c, &view.display_pack_id());
            if let Some(local_path) = view.local_path() {
                if let Ok(data) = status(cfg, Some(local_path)).await {
                    let indexed = data
                        .get("indexed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let vector_count = data
                        .get("vector_count")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let indexed_here = indexed && vector_count > 0;
                    let local_on = indexed_here;
                    let cloud_on = view.entry.cloud;
                    let tags = bracket_local_cloud(c, local_on, cloud_on);
                    println!("{}{} {}", line_prefix, name_bracket, tags);
                    let pending_add = data
                        .get("pending_add")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let jobs = data.get("jobs").and_then(Value::as_object);
                    let active_val = jobs.and_then(|j| {
                        if j.contains_key("active_for_this_pack") {
                            j.get("active_for_this_pack").filter(|v| !v.is_null())
                        } else {
                            j.get("active").filter(|v| !v.is_null())
                        }
                    });
                    let queued_for_pack = jobs
                        .and_then(|j| {
                            if j.contains_key("queued_jobs_for_this_pack") {
                                j.get("queued_jobs_for_this_pack").and_then(Value::as_array)
                            } else {
                                j.get("queued_jobs").and_then(Value::as_array)
                            }
                        })
                        .map(|a| a.as_slice())
                        .unwrap_or(&[]);
                    let queued_jobs = jobs
                        .and_then(|j| j.get("queued_jobs"))
                        .and_then(Value::as_array)
                        .map(|a| a.as_slice())
                        .unwrap_or(&[]);
                    let active_obj_early = active_val.and_then(Value::as_object);
                    let pack_path_str = local_path;
                    let mut show_removing = data
                        .get("pending_removal")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if !show_removing {
                        let remove_for_pack = |j: &Value| {
                            j.get("job_type").and_then(Value::as_str) == Some("remove_pack")
                                && j.get("pack_path")
                                    .and_then(Value::as_str)
                                    .map(|jp| registry_job_pack_paths_match(pack_path_str, jp))
                                    .unwrap_or(false)
                        };
                        let is_remove_for_active = active_obj_early.as_ref().map_or(false, |o| {
                            o.get("job_type").and_then(Value::as_str) == Some("remove_pack")
                                && o.get("pack_path")
                                    .and_then(Value::as_str)
                                    .map(|jp| registry_job_pack_paths_match(pack_path_str, jp))
                                    .unwrap_or(false)
                        });
                        show_removing = is_remove_for_active || queued_jobs.iter().any(remove_for_pack);
                    }
                    if show_removing {
                        if c {
                            println!(
                                "    - {} {}",
                                term::white_word(c, local_path_display.as_deref().unwrap_or(local_path)),
                                term::warn_words(c, "removing...")
                            );
                        } else {
                            println!(
                                "    - {} removing...",
                                local_path_display.as_deref().unwrap_or(local_path)
                            );
                        }
                    }
                    let indexed = data
                        .get("indexed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let vector_count = data
                        .get("vector_count")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    let entities = data.get("entities").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let relationships = data
                        .get("relationships")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    let counts_suffix = format!(
                        "{} vectors, {} entities, {} relationships",
                        vector_count, entities, relationships
                    );
                    let source_root_paths: Vec<String> = data
                        .get("source_root_paths")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    let index_warnings: Vec<String> = data
                        .get("index_warnings")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    if !show_removing {
                        print_pack_metadata_lines(c, view, local_path_display.as_deref());
                        if pending_add {
                            let mut printed = false;
                            if let Some(av) = active_val {
                                if print_indexing_lines_from_job(c, av, "indexing...") {
                                    printed = true;
                                } else {
                                    let id = av.get("id").and_then(Value::as_str).unwrap_or("?");
                                    if c {
                                        println!(
                                            "    - {} {}",
                                            term::dimmed_word(c, id),
                                            term::bold_green(c, "indexing...")
                                        );
                                    } else {
                                        println!("    - {} indexing...", id);
                                    }
                                    printed = true;
                                }
                            }
                            for q in queued_for_pack {
                                if print_indexing_lines_from_job(c, q, "queued...") {
                                    printed = true;
                                }
                            }
                            if !printed {
                                let id = active_val
                                    .and_then(|v| v.get("id"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("?");
                                if c {
                                    println!(
                                        "    - {} {}",
                                        term::dimmed_word(c, id),
                                        term::warn_words(c, "...pending")
                                    );
                                } else {
                                    println!("    - {} ...pending", id);
                                }
                            }
                            if c {
                                println!("    {}", term::dimmed_word(c, &counts_suffix));
                            } else {
                                println!("    {}", counts_suffix);
                            }
                        } else if !indexed {
                            let line = format!("not indexed ({})", counts_suffix);
                            if c {
                                println!("    {}", term::dimmed_word(c, &line));
                            } else {
                                println!("    {}", line);
                            }
                        } else if c {
                            println!("    {}", term::dimmed_word(c, &counts_suffix));
                        } else {
                            println!("    {}", counts_suffix);
                        }
                    }
                    if !show_removing {
                        for w in &index_warnings {
                            if c {
                                println!("    {}", term::danger_words(c, w));
                            } else {
                                println!("    {}", w);
                            }
                        }
                        if !pending_add {
                            for s in &source_root_paths {
                                if c {
                                    println!("    - {}", term::dimmed_word(c, s));
                                } else {
                                    println!("    - {}", s);
                                }
                            }
                        }
                    }
                } else {
                    let tags = bracket_local_cloud(c, false, view.entry.cloud);
                    println!("{}{} {}", line_prefix, name_bracket, tags);
                    print_pack_metadata_lines(c, view, local_path_display.as_deref());
                }
            } else {
                let tags = bracket_local_cloud(c, false, true);
                println!("{}{} {}", line_prefix, name_bracket, tags);
                print_pack_metadata_lines(c, view, None);
            }
        }
    }
    Ok(json!({ "packs": serialized_packs }))
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
            format!(
                "remove request failed: HTTP {} (empty response). If you recently updated, try stopping any running mk server and run the command again.",
                status.as_u16()
            )
        } else {
            format!("remove request failed: {}", body)
        };
        return Err(anyhow!("{}", msg));
    }
    Ok(serde_json::from_str(&body)?)
}

pub async fn query(cfg: &ServerConfig, args: &QueryArgs, pack: Option<&str>) -> Result<Value> {
    match resolve_query_target(pack, args.cloud).await? {
        QueryTarget::Cloud(pack_uri) => {
            let client = http_client()?;
            let url = format!("{}/query", cloud_base_url());
            let body = json!({
                "query": args.query,
                "top_k": args.top_k,
                "use_reranker": args.use_reranker,
                "raw": args.raw,
                "pack_uri": pack_uri,
            });
            let resp = cloud_request(client.post(url).json(&body))
                .timeout(REQ_TIMEOUT_DEFAULT)
                .send()
                .await?;
            let status = resp.status();
            let body = resp.text().await?;
            if !status.is_success() {
                return Err(anyhow!("cloud query request failed: {}", body));
            }
            let mut out: Value = serde_json::from_str(&body)?;
            if let Some(obj) = out.as_object_mut() {
                obj.insert("pack_origin".to_string(), json!("cloud"));
            }
            Ok(out)
        }
        QueryTarget::Local(local_pack) => {
            ensure_server(cfg).await?;
            let client = http_client()?;
            let url = format!("{}/query", cfg.base_url());
            let mut body = json!({
                "query": args.query,
                "top_k": args.top_k,
                "use_reranker": args.use_reranker,
                "raw": args.raw
            });
            if let Some(local_path) = local_pack {
                body["pack"] = json!(local_path);
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
                            "Could not reach the memkit server. If it was stopped, run `mk query` or `mk start` again.",
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
            let mut out: Value = serde_json::from_str(&body)?;
            if let Some(obj) = out.as_object_mut() {
                obj.insert("pack_origin".to_string(), json!("local"));
            }
            Ok(out)
        }
    }
}

pub async fn publish(
    _cfg: &ServerConfig,
    pack: Option<&str>,
    pack_uri: Option<&str>,
    cloud_pack_id: Option<&str>,
    overwrite: bool,
    output_json: bool,
) -> Result<Value> {
    let (registry_pack, _pack_root, pack_dir) = resolve_local_publish_root(pack)?;
    let manifest = load_manifest(&pack_dir)?;
    let scratch_root = std::env::temp_dir().join(format!(
        "memkit-publish-cli-{}-{}",
        manifest.pack_id,
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let cloud_pack_id = cloud_pack_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            uuid::Uuid::parse_str(value)
                .map(|parsed| parsed.to_string())
                .map_err(|_| anyhow!("--cloud-pack-id must be a UUID"))
        })
        .transpose()?;
    if let Some(ref override_pack_id) = cloud_pack_id {
        let cloud_packs = fetch_cloud_packs().await?;
        if let Some(existing) = cloud_pack_id_conflict(&cloud_packs, override_pack_id) {
            anyhow::bail!(
                "cloud pack id {} already exists at {}. Use `mk publish` without --cloud-pack-id to update that pack, or choose a new UUID to split into a distinct cloud pack",
                override_pack_id,
                existing.pack_uri
            );
        }
    }
    let prepared = if let Some(ref override_pack_id) = cloud_pack_id {
        build_cloud_publish_archive_with_pack_id(&pack_dir, &scratch_root, Some(override_pack_id))?
    } else {
        build_cloud_publish_archive(&pack_dir, &scratch_root)?
    };
    let selected_pack_uri = if let Some(pack_uri) = pack_uri {
        pack_uri.to_string()
    } else if let Some(ref override_pack_id) = cloud_pack_id {
        default_cloud_pack_uri_for_id(override_pack_id)?
    } else {
        default_cloud_pack_uri(&prepared.manifest)?
    };
    let selected_pack = crate::cloud::parse_cloud_pack_uri(&selected_pack_uri)?;
    if selected_pack.pack_id != prepared.manifest.pack_id {
        let _ = tokio::fs::remove_dir_all(&scratch_root).await;
        anyhow::bail!(
            "publish target {} does not match local pack_id {}",
            selected_pack_uri,
            prepared.manifest.pack_id
        );
    }

    let client = http_client()?;
    let url = format!("{}/publish", cloud_base_url());
    let body_bytes = tokio::fs::read(&prepared.archive_path)
        .await
        .with_context(|| format!("failed to read {}", prepared.archive_path.display()))?;
    let mut request = client
        .post(url)
        .header("x-memkit-pack-uri", &selected_pack_uri)
        .header("x-memkit-sha256", &prepared.sha256)
        .header("x-memkit-overwrite", if overwrite { "true" } else { "false" })
        .body(body_bytes);
    if let Some(ref name) = registry_pack.name {
        request = request.header("x-memkit-pack-name", name);
    }
    let resp = cloud_request(request)
        .timeout(REQ_TIMEOUT_INDEX)
        .send()
        .await?;
    let status = resp.status();
    let resp_body = resp.text().await?;
    let _ = tokio::fs::remove_dir_all(&scratch_root).await;
    if !status.is_success() {
        return Err(anyhow!("publish request failed: {}", resp_body));
    }
    let out: Value = serde_json::from_str(&resp_body)?;
    if !output_json {
        if let Some(uri) = out.get("pack_uri").and_then(Value::as_str) {
            let c = term::color_stdout();
            if let Some(revision) = out.get("revision").and_then(Value::as_str) {
                println!(
                    "{} {} {}",
                    term::success_words(c, "Published"),
                    uri,
                    term::dimmed_word(c, &format!("({})", revision))
                );
            } else {
                println!("{} {}", term::success_words(c, "Published"), uri);
            }
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
            if let Some(msg) = err_json
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
            {
                return Err(anyhow!("add request failed: {}", msg));
            }
        }
        if resp_body.is_empty() {
            return Err(anyhow!(
                "add request failed: HTTP {} (empty response)",
                status.as_u16()
            ));
        }
        return Err(anyhow!("add request failed: {}", resp_body));
    }
    serde_json::from_str(&resp_body).context("parse add response")
}

/// Print add result: "Added N chunks." when synchronous success, or "Adding (job-N)..." when async job.
pub fn print_add_started(data: &Value, pack_path: &str) {
    let c = term::color_stdout();
    if let Some(n) = data
        .get("result")
        .and_then(|r| r.get("chunks_added"))
        .and_then(Value::as_u64)
    {
        println!(
            "{} {}",
            term::success_words(c, "Added"),
            term::data_num(c, &format!("{} chunks.", n))
        );
        return;
    }
    if let Some(job_id) = data
        .get("job")
        .and_then(|j| j.get("id"))
        .and_then(Value::as_str)
    {
        println!(
            "{} ({}). Run 'mk status {}' to check progress.",
            term::success_words(c, "Adding"),
            job_id,
            pack_path
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::cloud::CloudTenantKind;
    use crate::pack::init_pack;

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("{}-{}", prefix, uuid::Uuid::new_v4()))
    }

    #[test]
    fn build_pack_views_merges_local_and_cloud_by_pack_id() {
        let root = unique_temp_dir("memkit-cli-pack-view");
        let pack_dir = root.join(".memkit");
        fs::create_dir_all(&root).expect("create root");
        init_pack(
            &pack_dir,
            false,
            "fastembed",
            "BAAI/bge-small-en-v1.5",
            384,
        )
        .expect("init pack");
        let manifest = load_manifest(&pack_dir).expect("manifest");
        let cloud_uri = format!("memkit://users/user-1/packs/{}", manifest.pack_id);
        let reg = Registry {
            packs: vec![RegistryPack {
                path: root.to_string_lossy().to_string(),
                name: Some("local-label".to_string()),
                default: true,
            }],
            default_path: Some(root.to_string_lossy().to_string()),
        };
        let cloud = vec![CloudPackSummary {
            pack_uri: cloud_uri.clone(),
            pack_id: manifest.pack_id.clone(),
            tenant_type: CloudTenantKind::Users,
            tenant_id: "user-1".to_string(),
            display_name: Some("cloud-label".to_string()),
            source_pack_id: None,
            current_revision: Some("rev-1".to_string()),
            published_at: None,
        }];

        let views = build_pack_views(&reg, &cloud);
        assert_eq!(views.len(), 1);
        let entry = &views[0].entry;
        assert_eq!(entry.pack_id.as_deref(), Some(manifest.pack_id.as_str()));
        assert!(entry.local);
        assert!(entry.cloud);
        assert!(entry.default);
        assert_eq!(
            entry.local_path.as_deref(),
            Some(root.to_string_lossy().as_ref())
        );
        assert_eq!(entry.cloud_uri.as_deref(), Some(cloud_uri.as_str()));
        assert_eq!(entry.local_name.as_deref(), Some("local-label"));
        assert_eq!(entry.cloud_name.as_deref(), Some("cloud-label"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cloud_pack_id_conflict_matches_existing_pack_id() {
        let packs = vec![CloudPackSummary {
            pack_uri: "memkit://users/user-1/packs/550e8400-e29b-41d4-a716-446655440000"
                .to_string(),
            pack_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
            tenant_type: CloudTenantKind::Users,
            tenant_id: "user-1".to_string(),
            display_name: Some("cloud-pack".to_string()),
            source_pack_id: None,
            current_revision: Some("rev-1".to_string()),
            published_at: None,
        }];

        let conflict = cloud_pack_id_conflict(
            &packs,
            "550e8400-e29b-41d4-a716-446655440000",
        )
        .expect("expected conflict");
        assert_eq!(
            conflict.pack_uri,
            "memkit://users/user-1/packs/550e8400-e29b-41d4-a716-446655440000"
        );
        assert!(cloud_pack_id_conflict(
            &packs,
            "8ee56b36-8b2f-420e-a3f9-4014c01c8225"
        )
        .is_none());
    }
}
