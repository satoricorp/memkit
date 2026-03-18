mod add_docs;
mod cli_client;
mod config;
#[cfg(feature = "helix")]
mod helix_store;
mod extract;
mod file_tree;
mod memkit_txt;
mod registry;
mod validate;
mod embed;
mod term;
mod google;
mod indexer;
mod ontology;
mod ontology_candle;
mod ontology_llama;
mod pack;
mod pack_location;
mod publish;
mod query;
mod query_synth;
mod rerank;
mod server;
mod types;

use std::env;
use std::path::PathBuf;
use std::str::FromStr;
use anyhow::{Context, Result, anyhow};

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Json,
    Text,
}

#[derive(Clone)]
struct CliContext {
    output_format: OutputFormat,
    dry_run: bool,
}

fn parse_global_flags(args: &[String]) -> (Vec<String>, CliContext) {
    let mut filtered = Vec::with_capacity(args.len());
    let mut output_format = OutputFormat::Text;
    let mut dry_run = false;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--output" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    if v == "json" {
                        output_format = OutputFormat::Json;
                    }
                    i += 1;
                }
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            other => {
                filtered.push(other.to_string());
                i += 1;
            }
        }
    }
    if let Ok(fmt) = env::var("OUTPUT_FORMAT") {
        if fmt == "json" {
            output_format = OutputFormat::Json;
        }
    }
    (
        filtered,
        CliContext {
            output_format,
            dry_run,
        },
    )
}
use colored_json::to_colored_json_auto;
use owo_colors::OwoColorize;

use crate::cli_client::{ServerConfig, QueryArgs};
use crate::pack::{has_manifest_at, init_pack};
use crate::registry::{ensure_registered, load_registry, pack_dir_for_path, resolve_pack_by_name_or_path, set_default};
use crate::server::run_server;

/// Wrap a single line at `width` chars; newlines in `s` are collapsed to space. Truncate to `max_chars` first.
fn wrap_retrieval_preview(s: &str, width: usize, max_chars: usize) -> String {
    let flat = s.replace('\n', " ");
    let truncated = if flat.len() > max_chars {
        format!("{}...", flat.chars().take(max_chars).collect::<String>())
    } else {
        flat
    };
    if truncated.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let mut remaining = truncated.as_str();
    let indent = "    ";
    while !remaining.is_empty() {
        let take = remaining.chars().take(width).count();
        if take == 0 {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(indent);
        out.push_str(&remaining.chars().take(width).collect::<String>());
        remaining = &remaining[remaining.char_indices().nth(take).map(|(i, _)| i).unwrap_or(remaining.len())..];
    }
    out
}

/// For add responses, replace full "content" in job.add_payload.items with a short placeholder so stdout isn't flooded.
fn trim_add_response_content(out: &serde_json::Value) -> serde_json::Value {
    let mut v = out.clone();
    if let Some(items) = v
        .get_mut("job")
        .and_then(|j| j.get_mut("add_payload"))
        .and_then(|p| p.get_mut("items"))
        .and_then(|a| a.as_array_mut())
    {
        for item in items {
            if let Some(obj) = item.as_object_mut() {
                if let Some(content) = obj.get("content").and_then(serde_json::Value::as_str) {
                    let len = content.len();
                    obj.insert(
                        "content".to_string(),
                        serde_json::Value::String(format!("({} characters)", len)),
                    );
                }
            }
        }
    }
    v
}

struct ServeConfig {
    packs: Vec<PathBuf>,
    host: String,
    port: u16,
}

enum CliCommand {
    Add {
        local_path: Option<String>,
        pack: Option<String>,
        api_request: Option<serde_json::Value>,
    },
    Remove { dir: Option<String>, yes: bool },
    Status { dir: Option<String> },
    List,
    Query {
        query: String,
        top_k: usize,
        use_reranker: bool,
        raw: bool,
        pack: Option<String>,
    },
    Schema { command: Option<String> },
    Publish {
        pack: Option<String>,
        destination: Option<String>,
    },
    Use { pack: Option<String> },
    Models,
    Serve {
        pack: Option<String>,
        host: Option<String>,
        port: Option<u16>,
        foreground: bool,
    },
    Stop { port: Option<u16> },
    Help,
}

fn resolve_pack_root(pack_arg: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = pack_arg {
        return resolve_pack_by_name_or_path(p);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if has_manifest_at(&cwd) {
        return Ok(cwd);
    }
    let reg = load_registry().unwrap_or_default();
    if let Some(ref default) = reg.default_path {
        return Ok(PathBuf::from(default));
    }
    if let Some(p) = reg.packs.first() {
        return Ok(PathBuf::from(&p.path));
    }
    if let Some(home) = dirs::home_dir() {
        if has_manifest_at(&home) {
            return Ok(home);
        }
    }
    anyhow::bail!(
        "no memory pack found. use --pack <name-or-path> or run `mk add <path>` first"
    )
}

/// Create a default memory pack in the home directory (~/.memkit) with a generated name (e.g. for first-time add).
fn create_default_pack() -> Result<PathBuf> {
    let home = dirs::home_dir().context("home directory not available")?;
    let pack_dir = pack_dir_for_path(&home);
    init_pack(&pack_dir, false, "fastembed", "BAAI/bge-small-en-v1.5", 384)
        .context("failed to init default pack")?;
    let normalized = home
        .canonicalize()
        .context("home directory path invalid")?
        .to_string_lossy()
        .to_string();
    let reg = load_registry().unwrap_or_default();
    ensure_registered(&normalized, Some("default".to_string()), reg.packs.is_empty())?;
    Ok(home)
}

/// Resolve pack root; if none exists and pack_arg is None, create a default pack in ~ and return it.
fn ensure_pack_root(pack_arg: Option<&str>) -> Result<PathBuf> {
    if pack_arg.is_none() {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let home_has_pack = dirs::home_dir()
            .as_ref()
            .map(|h| has_manifest_at(h))
            .unwrap_or(false);
        let reg = load_registry().unwrap_or_default();
        if reg.packs.is_empty() && !has_manifest_at(&cwd) && !home_has_pack {
            create_default_pack()?;
        }
    }
    resolve_pack_root(pack_arg)
}

fn parse_pack_paths(value: &str) -> Vec<PathBuf> {
    value
        .split(',')
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

fn extract_json_from_args(args: &[String]) -> (Option<serde_json::Value>, Vec<String>) {
    let mut filtered = Vec::new();
    let mut json_value = None;
    let mut i = 0;
    while i < args.len() {
        if args.get(i).map(|s| s.as_str()) == Some("--json") && i + 1 < args.len() {
            if let Ok(v) = serde_json::from_str(args[i + 1].as_str()) {
                json_value = Some(v);
            }
            i += 2;
        } else {
            filtered.push(args[i].clone());
            i += 1;
        }
    }
    (json_value, filtered)
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut i = 0usize;
    while i < args.len() {
        if args[i] == flag {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}

fn has_any_flag(args: &[String], flags: &[&str]) -> bool {
    args.iter().any(|a| flags.iter().any(|f| a == f))
}

fn parse_serve(args: &[String]) -> Result<Option<ServeConfig>> {
    let is_serve = args.first().map(|a| a == "--headless-serve").unwrap_or(false);
    if !is_serve {
        return Ok(None);
    }

    let mut packs: Vec<PathBuf> = env::var("MEMKIT_PACK_PATHS")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| parse_pack_paths(&v))
        .or_else(|| {
            env::var("MEMKIT_PACK_PATH")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(|v| vec![PathBuf::from(v)])
        })
        .unwrap_or_else(|| vec![PathBuf::from("./memory-pack")]);
    let mut host = "127.0.0.1".to_string();
    let mut port = 4242u16;

    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--pack" => {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or_else(|| anyhow!("missing value for --pack"))?;
                packs = parse_pack_paths(v);
            }
            "--host" => {
                i += 1;
                host = args
                    .get(i)
                    .cloned()
                    .ok_or_else(|| anyhow!("missing value for --host"))?;
            }
            "--port" => {
                i += 1;
                let raw = args
                    .get(i)
                    .ok_or_else(|| anyhow!("missing value for --port"))?;
                port = raw
                    .parse::<u16>()
                    .map_err(|_| anyhow!("invalid --port value: {}", raw))?;
            }
            other => {
                return Err(anyhow!("unsupported argument: {}", other));
            }
        }
        i += 1;
    }

    if packs.is_empty() {
        return Err(anyhow!("at least one pack path required"));
    }
    Ok(Some(ServeConfig { packs, host, port }))
}

fn doc_type_for_url(url: &str) -> &'static str {
    if url.contains("docs.google.com/spreadsheets/") {
        "google_sheet"
    } else if url.contains("docs.google.com/document/") {
        "google_doc"
    } else {
        "url"
    }
}

fn parse_add_command(j: &serde_json::Value, pack_override: Option<String>) -> Result<CliCommand> {
    let obj = j.as_object().ok_or_else(|| anyhow!("--json must be a JSON object"))?;
    let get_str = |k: &str| obj.get(k).and_then(serde_json::Value::as_str).map(String::from);
    let has_docs = j.get("documents").and_then(serde_json::Value::as_array).map(|a| !a.is_empty()).unwrap_or(false);
    let has_conv = j.get("conversation").and_then(serde_json::Value::as_array).map(|a| !a.is_empty()).unwrap_or(false);
    if has_docs || has_conv {
        for doc in j.get("documents").and_then(serde_json::Value::as_array).into_iter().flatten() {
            if let Some(v) = doc.get("value").and_then(serde_json::Value::as_str) {
                crate::validate::reject_control_chars(v)?;
            }
        }
        return Ok(CliCommand::Add {
            local_path: None,
            pack: pack_override.or_else(|| get_str("pack")).or_else(|| get_str("path")),
            api_request: Some(j.clone()),
        });
    }
    let path = get_str("path").ok_or_else(|| anyhow!("--json must include \"path\" (local path) or \"documents\"/\"conversation\" (API add)"))?;
    crate::validate::validate_path(&path)?;
    Ok(CliCommand::Add {
        local_path: Some(path),
        pack: pack_override.or_else(|| get_str("pack")),
        api_request: None,
    })
}

/// Build a CliCommand from a single JSON object (used for `mk --json '{...}'`).
fn cli_command_from_json(cmd: &str, j: &serde_json::Value) -> Result<CliCommand> {
    let obj = j.as_object().ok_or_else(|| anyhow!("--json must be a JSON object"))?;
    let get_str = |k: &str| obj.get(k).and_then(serde_json::Value::as_str).map(String::from);
    let get_u64 = |k: &str| obj.get(k).and_then(serde_json::Value::as_u64);
    let get_bool = |k: &str| obj.get(k).and_then(serde_json::Value::as_bool);

    match cmd {
        "add" => parse_add_command(j, None),
        "remove" => {
            let dir = get_str("dir");
            if let Some(ref d) = dir {
                crate::validate::validate_path(d)?;
            }
            let yes = get_bool("confirm").unwrap_or(false);
            Ok(CliCommand::Remove { dir, yes })
        }
        "status" => {
            let dir = get_str("dir");
            if let Some(ref d) = dir {
                crate::validate::validate_path(d)?;
            }
            Ok(CliCommand::Status { dir })
        }
        "list" => Ok(CliCommand::List),
        "query" => {
            let query = get_str("query").ok_or_else(|| anyhow!("--json must include \"query\""))?;
            crate::validate::reject_control_chars(&query)?;
            let top_k = get_u64("top_k").unwrap_or(8) as usize;
            let use_reranker = get_bool("use_reranker").unwrap_or(true);
            let raw = get_bool("raw").unwrap_or(false);
            let pack = get_str("pack");
            Ok(CliCommand::Query { query, top_k, use_reranker, raw, pack })
        }
        "publish" => Ok(CliCommand::Publish {
            pack: get_str("pack").or_else(|| get_str("path")),
            destination: get_str("destination"),
        }),
        "schema" => Ok(CliCommand::Schema { command: get_str("schema") }),
        "use" => Ok(CliCommand::Use { pack: get_str("pack") }),
        "models" => Ok(CliCommand::Models),
        "help" | "--help" | "-h" => Ok(CliCommand::Help),
        other => Err(anyhow!("unknown command: {}. run `mk help` for usage", other)),
    }
}

fn parse_cli_command(args: &[String]) -> Result<CliCommand> {
    if args.is_empty() {
        return Ok(CliCommand::Help);
    }

    // Top-level mk --json '{...}' — object must include "command" and command-specific fields
    if args[0] == "--json" && args.len() >= 2 {
        let j: serde_json::Value = serde_json::from_str(&args[1])
            .map_err(|e| anyhow!("invalid --json: {}", e))?;
        let command = j
            .get("command")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("--json must include \"command\""))?;
        return cli_command_from_json(command, &j);
    }

    match args[0].as_str() {
        "add" => {
            let (json_val, rest) = extract_json_from_args(&args[1..]);
            let pack_from_rest = flag_value(&rest, "--pack");
            if let Some(j) = json_val {
                return parse_add_command(&j, pack_from_rest);
            }
            let arg = rest
                .first()
                .cloned()
                .ok_or_else(|| anyhow!("usage: mk add <path-or-url> [--pack <dir>] or mk add --json '{{\"path\":\"...\"}}' or mk add --json '{{\"documents\":[...]}}'"))?;
            if arg.starts_with("http://") || arg.starts_with("https://") {
                let doc_type = doc_type_for_url(&arg);
                let api_request = serde_json::json!({
                    "documents": [{ "type": doc_type, "value": arg }]
                });
                return Ok(CliCommand::Add {
                    local_path: None,
                    pack: pack_from_rest,
                    api_request: Some(api_request),
                });
            }
            crate::validate::validate_path(&arg)?;
            Ok(CliCommand::Add {
                local_path: Some(arg),
                pack: pack_from_rest,
                api_request: None,
            })
        }
        "remove" => {
            let (json_val, rest) = extract_json_from_args(&args[1..]);
            let mut yes = false;
            let mut dir = None::<String>;
            if let Some(j) = json_val {
                dir = j.get("dir").and_then(serde_json::Value::as_str).map(String::from);
                if let Some(ref d) = dir {
                    crate::validate::validate_path(d)?;
                }
                yes = j.get("confirm").and_then(serde_json::Value::as_bool).unwrap_or(false);
                return Ok(CliCommand::Remove { dir, yes });
            }
            let mut i = 0usize;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--yes" | "-y" => {
                        yes = true;
                        i += 1;
                    }
                    _ => {
                        if dir.is_none() {
                            dir = Some(rest[i].clone());
                            if let Some(ref d) = dir {
                                crate::validate::validate_path(d)?;
                            }
                        }
                        i += 1;
                    }
                }
            }
            Ok(CliCommand::Remove { dir, yes })
        }
        "status" => {
            let (json_val, rest) = extract_json_from_args(&args[1..]);
            if let Some(j) = json_val {
                let dir = j.get("dir").and_then(serde_json::Value::as_str).map(String::from);
                if let Some(ref d) = dir {
                    crate::validate::validate_path(d)?;
                }
                return Ok(CliCommand::Status { dir });
            }
            let dir = rest.first().cloned();
            if let Some(ref d) = dir {
                crate::validate::validate_path(d)?;
            }
            Ok(CliCommand::Status { dir })
        }
        "list" => Ok(CliCommand::List),
        "query" => {
            let (json_val, rest) = extract_json_from_args(&args[1..]);
            if let Some(j) = json_val {
                let query = j
                    .get("query")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| anyhow!("--json must include \"query\""))?;
                crate::validate::reject_control_chars(&query)?;
                let top_k = j
                    .get("top_k")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(8) as usize;
                let use_reranker =
                    j.get("use_reranker").and_then(serde_json::Value::as_bool).unwrap_or(true);
                let raw = j.get("raw").and_then(serde_json::Value::as_bool).unwrap_or(false);
                let pack = j.get("pack").and_then(serde_json::Value::as_str).map(String::from);
                return Ok(CliCommand::Query {
                    query,
                    top_k,
                    use_reranker,
                    raw,
                    pack,
                });
            }
            if rest.is_empty() {
                return Err(anyhow!(
                    "usage: mk query <text> [--top-k N] [--no-rerank] [--pack <name-or-path>] [--raw] or mk query --json '{{\"query\":\"...\"}}'"
                ));
            }
            let query = rest[0].clone();
            crate::validate::reject_control_chars(&query)?;
            let mut top_k = 8usize;
            let mut use_reranker = true;
            let mut raw = false;
            let mut pack = None;
            let mut i = 1usize;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--no-rerank" => {
                        use_reranker = false;
                        i += 1;
                    }
                    "--top-k" => {
                        i += 1;
                        let v = rest.get(i).ok_or_else(|| anyhow!("missing value for --top-k"))?;
                        top_k = v
                            .parse::<usize>()
                            .map_err(|_| anyhow!("invalid --top-k value: {}", v))?;
                    }
                    "--pack" => {
                        i += 1;
                        pack = rest.get(i).cloned();
                    }
                    "--raw" => raw = true,
                    other => return Err(anyhow!("unsupported query argument: {}", other)),
                }
                i += 1;
            }
            Ok(CliCommand::Query {
                query,
                top_k,
                use_reranker,
                raw,
                pack,
            })
        }
        "publish" => {
            let (json_val, rest) = extract_json_from_args(&args[1..]);
            if let Some(j) = json_val {
                let path = j.get("path").and_then(serde_json::Value::as_str).map(String::from);
                let destination = j.get("destination").and_then(serde_json::Value::as_str).map(String::from);
                return Ok(CliCommand::Publish {
                    pack: path,
                    destination,
                });
            }
            let pack = flag_value(&rest, "--pack");
            let destination = flag_value(&rest, "--destination");
            Ok(CliCommand::Publish { pack, destination })
        }
        "schema" => {
            let command = args.get(1).cloned();
            Ok(CliCommand::Schema { command })
        }
        "use" => {
            let mut pack = flag_value(&args[1..], "--pack");
            if pack.is_none() {
                pack = args[1..].iter().find(|a| !a.starts_with('-')).cloned();
            }
            Ok(CliCommand::Use { pack })
        }
        "models" => Ok(CliCommand::Models),
        "serve" => {
            let pack = flag_value(&args[1..], "--pack");
            let host = flag_value(&args[1..], "--host");
            let port = flag_value(&args[1..], "--port")
                .map(|v| v.parse::<u16>().map_err(|_| anyhow!("invalid --port value")))
                .transpose()?;
            let foreground = has_any_flag(&args[1..], &["--foreground"]);
            Ok(CliCommand::Serve { pack, host, port, foreground })
        }
        "stop" => {
            let port = flag_value(&args[1..], "--port")
                .map(|v| v.parse::<u16>().map_err(|_| anyhow!("invalid --port value")))
                .transpose()?;
            Ok(CliCommand::Stop { port })
        }
        "help" | "--help" | "-h" => Ok(CliCommand::Help),
        other => Err(anyhow!(
            "unknown command: {}. run `mk help` for usage",
            other
        )),
    }
}

const SCHEMA_COMMANDS: &[&str] = &["add", "remove", "status", "query", "use", "models"];

fn schema_for_command(cmd: &str) -> Option<serde_json::Value> {
    Some(match cmd {
        "add" => serde_json::json!({
            "command": "add",
            "input": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Local path to add, or pack path when using documents/conversation"},
                    "pack": {"type": "string", "description": "Pack name or path (optional)"},
                    "documents": {
                        "type": "array",
                        "description": "API add: list of { type: url|content|google_doc|google_sheet, value: string }"
                    },
                    "conversation": {
                        "type": "array",
                        "description": "API add: list of { role, content }"
                    }
                }
            }
        }),
        "remove" => serde_json::json!({
            "command": "remove",
            "input": {
                "type": "object",
                "properties": {
                    "dir": {"type": "string", "description": "Directory to remove pack from (optional)"}
                }
            }
        }),
        "status" => serde_json::json!({
            "command": "status",
            "input": {
                "type": "object",
                "properties": {
                    "dir": {"type": "string", "description": "Pack directory (optional)"}
                }
            }
        }),
        "query" => serde_json::json!({
            "command": "query",
            "input": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Natural language query"},
                    "top_k": {"type": "integer", "default": 8},
                    "use_reranker": {"type": "boolean", "default": true},
                    "raw": {"type": "boolean", "default": false},
                    "pack": {"type": "string", "description": "Pack name or path (optional)"}
                },
                "required": ["query"]
            }
        }),
        "use" => serde_json::json!({
            "command": "use",
            "input": {
                "type": "object",
                "properties": {
                    "pack": {"type": "string", "description": "Pack name or path, or model id (e.g. openai:gpt-4o-mini) to set as default (optional; omit to show current default pack)"}
                }
            }
        }),
        "models" => serde_json::json!({
            "command": "models",
            "input": {},
            "output": {
                "current": "string | null",
                "supported": [{"id": "string", "description": "string"}]
            }
        }),
        _ => return None,
    })
}

fn print_schema(cmd: Option<&str>) -> Result<()> {
    match cmd {
        None => {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "commands": SCHEMA_COMMANDS,
                "usage": "mk schema <command>"
            }))?);
        }
        Some(c) => {
            if let Some(schema) = schema_for_command(c) {
                println!("{}", serde_json::to_string_pretty(&schema)?);
            } else {
                anyhow::bail!("unknown schema: {}. available: {}", c, SCHEMA_COMMANDS.join(", "));
            }
        }
    }
    Ok(())
}

fn print_help() {
    let color = crate::term::color_stdout();
    let title = if color {
        "memkit CLI".bold().cyan().to_string()
    } else {
        "memkit CLI".to_string()
    };
    println!("{}", title);
    println!();
    let usage = if color {
        "Usage:".dimmed().to_string()
    } else {
        "Usage:".to_string()
    };
    println!("{}", usage);
    println!("  mk --json '{{\"command\":\"<cmd>\", ...}}'  (any command with flags as object fields)");
    println!("  Global flags: [--output json|text] [--dry-run]");
    println!();
    let commands = [
        "  mk --json '{\"command\":\"<cmd>\", ...}'",
        "  mk add <path-or-url> [--pack <name-or-path>]",
        "  mk add --json '{\"documents\":[{\"type\":\"url\",\"value\":\"...\"}],\"path\":\"...\"}'",
        "  mk remove [dir]",
        "  mk status [dir]",
        "  mk list",
        "  mk query <text> [--top-k N] [--no-rerank] [--pack <name-or-path>] [--raw]",
        "  mk publish [--pack <name-or-path>] [--destination s3://bucket/prefix]",
        "  mk use [name-or-path|model-name]",
        "  mk models  (list current model and supported models; use 'mk use <model-name>' to set)",
        "  mk serve [--pack <path>] [--host H] [--port P] [--foreground]  (run server in background; use --foreground to attach)",
        "  mk stop [--port P]  (stop the background server)",
        "  mk schema [command]",
    ];
    for cmd in commands {
        if color {
            println!("{}", cmd.cyan());
        } else {
            println!("{}", cmd);
        }
    }
}

/// If dotenvy failed to set MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON (e.g. value has newlines), try loading
/// it from .env manually: find MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON= and parse the value (stop at next
/// KEY= or end of file; strip surrounding quotes). Tries current_dir(), then executable's directory
/// and parents, then ~/.memkit/.env.
fn load_memkit_google_json_from_dotenv_fallback() {
    if std::env::var("MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON").is_ok() {
        return;
    }
    const PREFIX: &str = "MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON=";

    /// Extract value after PREFIX: stop at next line that looks like VAR= so we don't pull in the next env entry.
    fn extract_value(content: &str, after_prefix: usize) -> &str {
        let rest = content[after_prefix..].trim_start();
        if rest.is_empty() {
            return rest;
        }
        let lines: Vec<&str> = rest.split('\n').collect();
        let mut value_end = rest.len();
        let mut offset = 0usize;
        for (i, line) in lines.iter().enumerate() {
            if i >= 1 {
                let trimmed = line.trim_start();
                if !trimmed.is_empty() {
                    if let Some(first) = trimmed.chars().next() {
                        if first.is_ascii_alphabetic() || first == '_' {
                            let key_len = trimmed
                                .chars()
                                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                                .count();
                            if key_len > 0 && trimmed.chars().nth(key_len) == Some('=') {
                                value_end = offset;
                                break;
                            }
                        }
                    }
                }
            }
            offset += line.len() + 1;
        }
        rest[..value_end.min(rest.len())].trim_end()
    }

    let try_env_file = |path: &std::path::Path| -> bool {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return false,
        };
        let idx = match content.find(PREFIX) {
            Some(i) => i,
            None => return false,
        };
        let mut value = extract_value(&content, idx + PREFIX.len()).to_string();
        if value.is_empty() || !value.starts_with('{') {
            if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
                value = value[1..value.len() - 1].replace("\\\"", "\"");
            } else if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
                value = value[1..value.len() - 1].to_string();
            }
        }
        if value.starts_with('{') {
            // SAFETY: single-threaded at startup; no other thread reads this var yet.
            unsafe { std::env::set_var("MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON", value); }
            true
        } else {
            false
        }
    };

    if let Ok(cwd) = std::env::current_dir() {
        if try_env_file(&cwd.join(".env")) {
            return;
        }
    }
    let mut dir = match std::env::current_exe() {
        Ok(exe) => exe.parent().map(|p| p.to_path_buf()),
        Err(_) => None,
    };
    for _ in 0..10 {
        let Some(d) = dir.as_ref() else { break };
        if try_env_file(&d.join(".env")) {
            return;
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    if let Some(home) = dirs::home_dir() {
        if try_env_file(&home.join(".memkit").join(".env")) {
            return;
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    load_memkit_google_json_from_dotenv_fallback();
    let args: Vec<String> = env::args().skip(1).collect();
    let (args, ctx) = parse_global_flags(&args);

    config::ensure_config_exists().context("failed to create config (e.g. ~/.config/memkit/memkit.json)")?;

    if let Some(cfg) = parse_serve(&args)? {
        serve_with_startup(cfg.packs, cfg.host, cfg.port).await?;
        return Ok(());
    }

    match parse_cli_command(&args)? {
        CliCommand::Help => print_help(),
        CliCommand::Schema { command } => {
            print_schema(command.as_deref())?;
            return Ok(());
        }
        CliCommand::Serve { pack, host, port, foreground } => {
            let run_server = env::var("MEMKIT_SERVE_FOREGROUND").is_ok() || foreground;
            if !run_server {
                let exe = std::env::current_exe()
                    .map_err(|e| anyhow!("current exe: {}", e))?;
                let child_args: Vec<String> = std::env::args().skip(1).collect();
                let mut cmd = std::process::Command::new(&exe);
                cmd.args(&child_args).env("MEMKIT_SERVE_FOREGROUND", "1");
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());
                cmd.spawn().map_err(|e| anyhow!("failed to start server process: {}", e))?;
                std::thread::sleep(std::time::Duration::from_secs(2));
                println!(
                    "{}",
                    crate::term::style_stdout(
                        "Server started in background. Use 'mk status' to check.",
                        |s| s.green().to_string(),
                    )
                );
                return Ok(());
            }
            let packs: Vec<PathBuf> = if let Some(ref p) = pack {
                vec![resolve_pack_by_name_or_path(p)?]
            } else {
                let _ = crate::registry::ensure_default_if_unset();
                let reg = load_registry().unwrap_or_default();
                if reg.packs.is_empty() {
                    anyhow::bail!("no packs registered. Add a pack first (e.g. mk add <path>) or run with --pack <path>");
                }
                reg.packs.iter().map(|p| PathBuf::from(&p.path)).collect()
            };
            let host = host.unwrap_or_else(|| "127.0.0.1".to_string());
            let port = port.unwrap_or(4242);
            serve_with_startup(packs, host, port).await?;
            return Ok(());
        }
        CliCommand::Stop { port } => {
            let port = port
                .or_else(|| env::var("API_PORT").ok().and_then(|v| v.parse::<u16>().ok()))
                .unwrap_or(4242);
            let output = std::process::Command::new("lsof")
                .args(["-ti", &format!(":{}", port)])
                .output()
                .context("lsof failed")?;
            if output.status.success() && !output.stdout.is_empty() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for pid in stdout.trim().split_whitespace() {
                    let _ = std::process::Command::new("kill").arg(pid).status();
                }
                println!(
                    "{}",
                    crate::term::style_stdout("Server stopped.", |s| s.green().to_string())
                );
            } else {
                let msg = format!("No server running on port {}.", port);
                println!(
                    "{}",
                    crate::term::style_stdout(&msg, |s| s.dimmed().to_string())
                );
            }
            return Ok(());
        }
        cmd => {
            let cfg = ServerConfig::from_env();

            match &cmd {
                CliCommand::Remove { dir, yes } => {
                    let target = if let Some(name_or_path) = dir.as_deref() {
                        resolve_pack_by_name_or_path(name_or_path)?
                    } else {
                        std::env::current_dir()
                            .unwrap_or_else(|_| PathBuf::from("."))
                            .canonicalize()
                            .with_context(|| "path not found: current directory")?
                    };
                    if ctx.dry_run {
                        let out = serde_json::json!({
                            "dry_run": true,
                            "would": "remove",
                            "dir": target.display().to_string(),
                            "status": "skipped"
                        });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                        return Ok(());
                    }
                    if !*yes {
                        use std::io::IsTerminal;
                        if !std::io::stdin().is_terminal() {
                            anyhow::bail!("not a TTY; pass --yes to remove without confirmation");
                        }
                        print!("Remove pack at {}? [y/N] ", target.display());
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                        let mut line = String::new();
                        std::io::stdin().read_line(&mut line).context("read confirmation")?;
                        let confirmed = line.trim().eq_ignore_ascii_case("y") || line.trim().eq_ignore_ascii_case("yes");
                        if !confirmed {
                            return Ok(());
                        }
                    }
                    // Fall through to use server remove job (POST /remove).
                }
                CliCommand::Use { pack } => {
                    if let Some(name_or_path) = pack.as_ref() {
                        if name_or_path.contains(':') {
                            if config::is_supported_model(name_or_path) {
                                config::set_model(name_or_path)?;
                                println!(
                                    "{} {}",
                                    crate::term::style_stdout(
                                        "Default model set to",
                                        |s| s.green().to_string()
                                    ),
                                    name_or_path
                                );
                                return Ok(());
                            } else {
                                anyhow::bail!(
                                    "unknown model '{}'. run `mk models` to see supported models.",
                                    name_or_path
                                );
                            }
                        }
                    }
                    let reg = load_registry()?;
                    if let Some(name_or_path) = pack {
                        set_default(name_or_path)?;
                        println!(
                            "{} {}",
                            crate::term::style_stdout("Default pack set to", |s| s.green().to_string()),
                            name_or_path
                        );
                    } else {
                        if let Some(ref default_path) = reg.default_path {
                            let default_pack = reg.packs.iter().find(|p| p.path == *default_path);
                            let (name, path) = default_pack
                                .map(|p| (p.name.as_deref().unwrap_or(p.path.as_str()), p.path.as_str()))
                                .unwrap_or((default_path.as_str(), default_path.as_str()));
                            println!(
                                "{} {}  {}",
                                crate::term::style_stdout("Default pack:", |s| s.bold().to_string()),
                                name,
                                crate::term::style_stdout(path, |s| s.dimmed().to_string())
                            );
                        } else {
                            println!(
                                "{}",
                                crate::term::style_stdout("No default pack set", |s| s.yellow().to_string())
                            );
                        }
                    }
                    return Ok(());
                }
                CliCommand::Models => {
                    let cfg = config::load_config().unwrap_or_default();
                    let supported = config::supported_models();
                    if ctx.output_format == OutputFormat::Json {
                        let out = serde_json::json!({
                            "current": cfg.model,
                            "supported": supported.iter().map(|(id, desc)| serde_json::json!({"id": id, "description": desc})).collect::<Vec<_>>()
                        });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        if crate::term::color_stdout() {
                            println!("{}", "Models".bold().cyan());
                            println!();
                            if let Some(ref m) = cfg.model {
                                println!("  {} {}", "Current:".bold(), m);
                            } else {
                                println!("  {} (none set)", "Current:".bold());
                            }
                            println!();
                            println!("  {}", "Supported:".bold());
                            for (id, desc) in &supported {
                                println!("    {}  {}", id.cyan(), desc.dimmed());
                            }
                            println!();
                            println!("  {}", "Run 'mk use <model-name>' to use one of the available models.".dimmed());
                        } else {
                            if let Some(ref m) = cfg.model {
                                println!("Current: {}", m);
                            } else {
                                println!("Current: (none set)");
                            }
                            println!("Supported:");
                            for (id, desc) in &supported {
                                println!("  {}  {}", id, desc);
                            }
                            println!();
                            println!("Run 'mk use <model-name>' to use one of the available models.");
                        }
                    }
                    return Ok(());
                }
                _ => {}
            }

            let commands_need_server = !matches!(cmd, CliCommand::Help | CliCommand::Schema { .. } | CliCommand::Use { .. } | CliCommand::Models | CliCommand::Serve { .. } | CliCommand::Stop { .. });
            if commands_need_server {
                cli_client::require_server(&cfg).await?;
            }

            enum CommandOut {
                Done,
                Output(serde_json::Value),
            }
            let result: Result<CommandOut> = match cmd {
                CliCommand::Remove { dir, yes: _ } => {
                    let target = if let Some(name_or_path) = dir.as_deref() {
                        resolve_pack_by_name_or_path(name_or_path)?
                    } else {
                        std::env::current_dir()
                            .unwrap_or_else(|_| PathBuf::from("."))
                            .canonicalize()
                            .with_context(|| "path not found: current directory")?
                    };
                    let path_str = target.display().to_string();
                    let out = cli_client::remove(&cfg, &path_str).await?;
                    if ctx.output_format != OutputFormat::Json {
                        if let Some(job_id) = out.get("job").and_then(|j| j.get("id")).and_then(|v| v.as_str()) {
                            println!(
                                "{} ({}). Run 'mk status' to check progress.",
                                crate::term::style_stdout("Removal started", |s| s.green().to_string()),
                                job_id
                            );
                        }
                    }
                    Ok(CommandOut::Output(out))
                }
                CliCommand::Add { local_path, pack, api_request } => {
                    if let Some(ref body) = api_request {
                        if ctx.dry_run {
                            let out = serde_json::json!({
                                "dry_run": true,
                                "would": "POST /add",
                                "body": body,
                                "status": "skipped"
                            });
                            println!("{}", serde_json::to_string_pretty(&out)?);
                            Ok(CommandOut::Done)
                        } else {
                            let pack_root = ensure_pack_root(pack.as_deref())?;
                            let mut body = body.clone();
                            if let Some(obj) = body.as_object_mut() {
                                obj.insert("path".to_string(), serde_json::Value::String(pack_root.to_string_lossy().to_string()));
                            }
                            let out = cli_client::add(&cfg, &body).await?;
                            if ctx.output_format != OutputFormat::Json {
                                cli_client::print_add_started(&out, pack_root.to_string_lossy().as_ref());
                            }
                            Ok(CommandOut::Output(out))
                        }
                    } else {
                        let path = local_path
                            .as_deref()
                            .ok_or_else(|| anyhow!("missing add path"))?;
                        if ctx.dry_run {
                            let pack_display = pack.as_deref().unwrap_or("(default)");
                            let out = serde_json::json!({
                                "dry_run": true,
                                "would": "add",
                                "path": path,
                                "pack": pack_display,
                                "status": "skipped"
                            });
                            println!("{}", serde_json::to_string_pretty(&out)?);
                            Ok(CommandOut::Done)
                        } else {
                            let source = PathBuf::from(path)
                                .canonicalize()
                                .with_context(|| format!("path not found: {}", path))?;
                            let pack_root = ensure_pack_root(pack.as_deref())?;
                            let home = dirs::home_dir().context("home directory not available")?;
                            let home_canon = home.canonicalize().unwrap_or(home.clone());
                            if source == home_canon {
                                anyhow::bail!(
                                    "Cannot add home directory as a source. Add specific directories (e.g. mk add ~/Documents/...) instead."
                                );
                            }
                            let path_str = source.to_string_lossy().to_string();
                            let mut body = serde_json::json!({ "path": path_str });
                            if pack.is_some() {
                                body["pack"] = serde_json::json!(pack_root.to_string_lossy().to_string());
                            }
                            let out = cli_client::add(&cfg, &body).await?;
                            if ctx.output_format != OutputFormat::Json {
                                if let Some(job_id) = out.get("job").and_then(|j| j.get("id")).and_then(serde_json::Value::as_str) {
                                    println!(
                                        "{} ({}). Waiting for indexing to finish...",
                                        crate::term::style_stdout("Adding", |s| s.green().to_string()),
                                        job_id
                                    );
                                }
                            }
                            if out.get("job").is_some() && !ctx.dry_run {
                                let pack_path = pack_root.to_string_lossy().to_string();
                                cli_client::poll_until_index_done(&cfg, &pack_path).await?;
                                if ctx.output_format != OutputFormat::Json {
                                    let data = cli_client::status(&cfg, Some(&pack_path)).await?;
                                    cli_client::print_status(&data);
                                }
                            }
                            Ok(CommandOut::Output(out))
                        }
                    }
                }
                CliCommand::Status { dir } => {
                    let output_json = ctx.output_format == OutputFormat::Json;
                    if dir.is_none() {
                        let data = cli_client::list(&cfg, output_json).await?;
                        if output_json {
                            let json_str = serde_json::to_string_pretty(&data)?;
                            println!("{}", json_str);
                        }
                        Ok(CommandOut::Done)
                    } else {
                        let data = cli_client::status(&cfg, dir.as_deref()).await?;
                        if output_json {
                            println!("{}", serde_json::to_string_pretty(&data)?);
                        } else {
                            cli_client::print_status(&data);
                        }
                        Ok(CommandOut::Done)
                    }
                }
                CliCommand::List => {
                    let output_json = ctx.output_format == OutputFormat::Json;
                    let data = cli_client::list(&cfg, output_json).await?;
                    if output_json {
                        println!("{}", serde_json::to_string_pretty(&data)?);
                    }
                    Ok(CommandOut::Done)
                }
                CliCommand::Publish { pack, destination } => {
                    let out = cli_client::publish(
                        &cfg,
                        pack.as_deref(),
                        destination.as_deref(),
                        ctx.output_format == OutputFormat::Json,
                    )
                    .await?;
                    if ctx.output_format == OutputFormat::Json {
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    }
                    Ok(CommandOut::Done)
                }
                CliCommand::Query { query, top_k, use_reranker, raw, pack } => {
                    let out = cli_client::query(&cfg, &QueryArgs { query, top_k, use_reranker, raw }, pack.as_deref()).await?;
                    let use_formatted = !raw && ctx.output_format != OutputFormat::Json;
                    if use_formatted {
                        if let Some(synth_err) = out.get("synthesis_error").and_then(serde_json::Value::as_str) {
                            println!(
                                "{}",
                                crate::term::style_stdout(
                                    "Retrieval succeeded; synthesis failed:",
                                    |s| s.yellow().to_string()
                                )
                            );
                            println!("  {}", synth_err);
                            if let Some(results) = out.get("results").and_then(serde_json::Value::as_array) {
                                if !results.is_empty() {
                                    println!();
                                    println!("Top results from your pack:");
                                    for (i, r) in results.iter().take(5).enumerate() {
                                        let path = r.get("file_path").and_then(serde_json::Value::as_str).unwrap_or("?");
                                        let content = r.get("content").and_then(serde_json::Value::as_str).unwrap_or("");
                                        let preview = if content.len() > 120 { format!("{}...", &content[..120]) } else { content.to_string() };
                                        println!(
                                            "  {}. {} {}",
                                            i + 1,
                                            crate::term::style_stdout(path, |s| s.dimmed().to_string()),
                                            crate::term::style_stdout(&preview, |s| s.dimmed().to_string())
                                        );
                                    }
                                }
                            }
                            if let Some(rr) = out.get("retrieval_results").and_then(serde_json::Value::as_array) {
                                if !rr.is_empty() {
                                    println!();
                                    println!("Retrieval (vector store, before rerank):");
                                    for (i, r) in rr.iter().take(10).enumerate() {
                                        let path = r.get("file_path").and_then(serde_json::Value::as_str).unwrap_or("?");
                                        let score = r.get("score").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
                                        let content = r.get("content").and_then(serde_json::Value::as_str).unwrap_or("");
                                        println!(
                                            "  {}. {} score={:.3}",
                                            i + 1,
                                            crate::term::style_stdout(path, |s| s.dimmed().to_string()),
                                            score
                                        );
                                        let wrapped = wrap_retrieval_preview(content, 72, 200);
                                        println!(
                                            "{}",
                                            crate::term::style_stdout(&wrapped, |s| s.dimmed().to_string())
                                        );
                                    }
                                }
                            }
                            Ok(CommandOut::Done)
                        } else if let (Some(answer), Some(sources)) = (
                            out.get("answer").and_then(serde_json::Value::as_str),
                            out.get("sources").and_then(serde_json::Value::as_array),
                        ) {
                            if let Some(model) = out.get("model").and_then(serde_json::Value::as_str) {
                                println!("Model: {}", model);
                                println!();
                            }
                            if let Some(provider) = out.get("provider").and_then(serde_json::Value::as_str) {
                                println!("[{}]", provider);
                                println!();
                            }
                            println!("{}", answer);
                            if !sources.is_empty() {
                                println!();
                                println!("Sources:");
                                for s in sources.iter().take(5) {
                                    let path = s
                                        .get("path")
                                        .and_then(serde_json::Value::as_str)
                                        .unwrap_or("?");
                                    println!("  {}", path);
                                }
                            }
                            Ok(CommandOut::Done)
                        } else {
                            Ok(CommandOut::Output(out))
                        }
                    } else {
                        Ok(CommandOut::Output(out))
                    }
                }
                CliCommand::Help | CliCommand::Schema { .. } | CliCommand::Use { .. } | CliCommand::Models | CliCommand::Serve { .. } | CliCommand::Stop { .. } => unreachable!(),
            };
            let command_out = result?;
            if let CommandOut::Output(out) = command_out {
                let out_display = trim_add_response_content(&out);
                let json_str = serde_json::to_string_pretty(&out_display)?;
                let output = if ctx.output_format == OutputFormat::Json || !crate::term::color_stdout() {
                    json_str
                } else {
                    to_colored_json_auto(&out_display).unwrap_or(json_str.clone())
                };
                println!("{}", output);
            }
        }
    }

    Ok(())
}

pub(crate) async fn serve_with_startup(packs: Vec<PathBuf>, host: String, port: u16) -> Result<()> {
    let color = crate::term::color_stdout();
    let port = env::var("API_PORT")
        .ok()
        .and_then(|p| u16::from_str(&p).ok())
        .unwrap_or(port);
    if color {
        let pack_display = if packs.len() == 1 {
            packs[0].display().to_string()
        } else {
            format!("{} packs", packs.len())
        };
        println!(
            "{} {} {} {}:{}",
            "serving pack".cyan(),
            pack_display.bold(),
            "on".cyan(),
            host.cyan(),
            port.to_string().cyan()
        );
    } else {
        if packs.len() == 1 {
            println!("serving pack {} on {}:{}", packs[0].display(), host, port);
        } else {
            println!("serving {} packs on {}:{}", packs.len(), host, port);
        }
    }
    run_server(packs, host, port).await?;
    Ok(())
}
