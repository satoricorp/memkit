mod add_docs;
mod cli_client;
#[cfg(feature = "helix")]
mod helix_store;
mod extract;
mod file_tree;
mod memkit_txt;
mod registry;
mod validate;
mod embed;
mod term;
#[cfg(feature = "lance-falkor")]
mod falkor_store;
mod google;
mod indexer;
#[cfg(feature = "lance-falkor")]
mod lancedb_store;
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
use crate::pack::{
    add_source_root, copy_dir_into_sources, copy_file_into_sources, scrub_pack_from_dir,
};
use crate::registry::{load_registry, pack_dir_for_path, remove_pack, remove_pack_by_path, resolve_pack_by_name_or_path, set_default};
use crate::server::run_server;

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
    Remove { dir: Option<String> },
    Status { dir: Option<String> },
    List,
    Index { dir: String, name: Option<String> },
    Graph { pack: Option<String> },
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
    Unregister { pack: Option<String> },
    Help,
}

/// Packs to pass to the server when the CLI starts it. Used only for ensure_server.
fn packs_for_command(cmd: &CliCommand) -> Result<Vec<PathBuf>> {
    let packs = match cmd {
        CliCommand::Add { pack, .. } => vec![resolve_pack_root(pack.as_deref())?],
        CliCommand::Index { dir, .. } => {
            vec![PathBuf::from(dir)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(dir))]
        }
        CliCommand::Status { dir } => match dir {
            Some(d) => vec![PathBuf::from(d)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(d))],
            None => vec![resolve_pack_root(None)?],
        },
        CliCommand::List => vec![resolve_pack_root(None)?],
        CliCommand::Graph { pack } => vec![resolve_pack_root(pack.as_deref())?],
        CliCommand::Query { pack, .. } => vec![resolve_pack_root(pack.as_deref())?],
        CliCommand::Publish { pack, .. } => vec![resolve_pack_root(pack.as_deref())?],
        CliCommand::Use { pack } => vec![resolve_pack_root(pack.as_deref())?],
        CliCommand::Remove { .. } | CliCommand::Unregister { .. } | CliCommand::Schema { .. } | CliCommand::Help => {
            vec![resolve_pack_root(None)?]
        }
    };
    Ok(packs)
}

fn resolve_pack_root(pack_arg: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = pack_arg {
        return resolve_pack_by_name_or_path(p);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.join(".memkit/manifest.json").exists() {
        return Ok(cwd);
    }
    let reg = load_registry().unwrap_or_default();
    if let Some(ref default) = reg.default_path {
        return Ok(PathBuf::from(default));
    }
    if let Some(p) = reg.packs.first() {
        return Ok(PathBuf::from(&p.path));
    }
    anyhow::bail!(
        "no memory pack found. use --pack <name-or-path> or run `mk index <dir>` first"
    )
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
            Ok(CliCommand::Remove { dir })
        }
        "status" => {
            let dir = get_str("dir");
            if let Some(ref d) = dir {
                crate::validate::validate_path(d)?;
            }
            Ok(CliCommand::Status { dir })
        }
        "list" => Ok(CliCommand::List),
        "index" => {
            let dir = get_str("dir").ok_or_else(|| anyhow!("--json must include \"dir\""))?;
            crate::validate::validate_path(&dir)?;
            Ok(CliCommand::Index { dir, name: get_str("name") })
        }
        "graph" => Ok(CliCommand::Graph { pack: get_str("pack") }),
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
        "unregister" => Ok(CliCommand::Unregister { pack: get_str("pack") }),
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
            let mut pack_from_rest = None;
            let mut i = 0usize;
            while i < rest.len() {
                if rest[i] == "--pack" && rest.get(i + 1).is_some() {
                    pack_from_rest = rest.get(i + 1).cloned();
                    i += 2;
                } else {
                    i += 1;
                }
            }
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
            if let Some(j) = json_val {
                let dir = j.get("dir").and_then(serde_json::Value::as_str).map(String::from);
                if let Some(ref d) = dir {
                    crate::validate::validate_path(d)?;
                }
                return Ok(CliCommand::Remove { dir });
            }
            let dir = rest.first().cloned();
            if let Some(ref d) = dir {
                crate::validate::validate_path(d)?;
            }
            Ok(CliCommand::Remove { dir })
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
        "index" => {
            let (json_val, rest) = extract_json_from_args(&args[1..]);
            if let Some(j) = json_val {
                let dir = j
                    .get("dir")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| anyhow!("--json must include \"dir\""))?;
                crate::validate::validate_path(&dir)?;
                return Ok(CliCommand::Index { dir, name: j.get("name").and_then(serde_json::Value::as_str).map(String::from) });
            }
            let dir = rest
                .first()
                .cloned()
                .ok_or_else(|| anyhow!("usage: mk index <dir> [--name <name>] or mk index --json '{{\"dir\":\"...\"}}'"))?;
            crate::validate::validate_path(&dir)?;
            let mut name = None;
            let mut i = 1usize;
            while i < rest.len() {
                if rest[i] == "--name" && rest.get(i + 1).is_some() {
                    name = rest.get(i + 1).cloned();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            Ok(CliCommand::Index { dir, name })
        }
        "graph" => {
            let (json_val, rest) = extract_json_from_args(&args[1..]);
            if let Some(j) = json_val {
                let pack = j.get("pack").and_then(serde_json::Value::as_str).map(String::from);
                return Ok(CliCommand::Graph { pack });
            }
            let mut pack = None;
            let mut i = 0usize;
            while i < rest.len() {
                if rest[i] == "--pack" && rest.get(i + 1).is_some() {
                    pack = rest.get(i + 1).cloned();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            Ok(CliCommand::Graph { pack })
        }
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
            let mut pack = None;
            let mut destination = None;
            let mut i = 0usize;
            while i < rest.len() {
                if rest[i] == "--pack" && rest.get(i + 1).is_some() {
                    pack = rest.get(i + 1).cloned();
                    i += 2;
                } else if rest[i] == "--destination" && rest.get(i + 1).is_some() {
                    destination = rest.get(i + 1).cloned();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            Ok(CliCommand::Publish { pack, destination })
        }
        "schema" => {
            let command = args.get(1).cloned();
            Ok(CliCommand::Schema { command })
        }
        "use" => {
            let mut pack = None;
            let mut i = 1usize;
            while i < args.len() {
                if args[i] == "--pack" && args.get(i + 1).is_some() {
                    pack = args.get(i + 1).cloned();
                    i += 2;
                } else if !args[i].starts_with('-') {
                    pack = Some(args[i].clone());
                    i += 1;
                } else {
                    i += 1;
                }
            }
            Ok(CliCommand::Use { pack })
        }
        "unregister" => {
            let mut pack = None;
            let mut i = 1usize;
            while i < args.len() {
                if args[i] == "--pack" && args.get(i + 1).is_some() {
                    pack = args.get(i + 1).cloned();
                    i += 2;
                } else if !args[i].starts_with('-') {
                    pack = Some(args[i].clone());
                    i += 1;
                } else {
                    i += 1;
                }
            }
            Ok(CliCommand::Unregister { pack })
        }
        "help" | "--help" | "-h" => Ok(CliCommand::Help),
        other => Err(anyhow!(
            "unknown command: {}. run `mk help` for usage",
            other
        )),
    }
}

const SCHEMA_COMMANDS: &[&str] = &["add", "remove", "status", "index", "graph", "query", "use", "unregister"];

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
        "index" => serde_json::json!({
            "command": "index",
            "input": {
                "type": "object",
                "properties": {
                    "dir": {"type": "string", "description": "Directory to index"},
                    "name": {"type": "string", "description": "Pack name for reference (optional; default random word)"}
                },
                "required": ["dir"]
            }
        }),
        "graph" => serde_json::json!({
            "command": "graph",
            "input": {
                "type": "object",
                "properties": {
                    "pack": {"type": "string", "description": "Pack name or path (optional)"}
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
                    "pack": {"type": "string", "description": "Pack name or path to set as default (optional; omit to show current default)"}
                }
            }
        }),
        "unregister" => serde_json::json!({
            "command": "unregister",
            "input": {
                "type": "object",
                "properties": {
                    "pack": {"type": "string", "description": "Pack name or path to remove from registry"}
                },
                "required": ["pack"]
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
        "  mk index <dir> [--name <name>]",
        "  mk graph [--pack <name-or-path>]",
        "  mk query <text> [--top-k N] [--no-rerank] [--pack <name-or-path>] [--raw]",
        "  mk publish [--pack <name-or-path>] [--destination s3://bucket/prefix]",
        "  mk use [name-or-path]",
        "  mk unregister <name-or-path>",
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

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let args: Vec<String> = env::args().skip(1).collect();
    let (args, ctx) = parse_global_flags(&args);

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
        cmd => {
            let cfg = ServerConfig::from_env();

            match &cmd {
                CliCommand::Remove { dir } => {
                    let target = dir
                        .as_deref()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
                    let target = target
                        .canonicalize()
                        .with_context(|| format!("path not found: {}", target.display()))?;
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
                    #[cfg(feature = "helix")]
                    crate::helix_store::remove_helix_for_pack(&target)?;
                    let was_in_registry = remove_pack_by_path(&target)?;
                    match scrub_pack_from_dir(&target) {
                        Ok(()) => {
                            if crate::term::color_stdout() {
                                println!("{} scrubbed from {}", "Memory pack removed".green(), target.display());
                            } else {
                                println!("Memory pack removed from {}", target.display());
                            }
                        }
                        Err(e) => {
                            if was_in_registry {
                                if crate::term::color_stdout() {
                                    println!("{} {} (no pack artifacts in directory)", "Pack removed from registry".green(), target.display());
                                } else {
                                    println!("Pack removed from registry {} (no pack artifacts in directory)", target.display());
                                }
                            } else {
                                return Err(e);
                            }
                        }
                    }
                    return Ok(());
                }
                CliCommand::Unregister { pack } => {
                    let name_or_path = pack
                        .as_deref()
                        .ok_or_else(|| anyhow!("usage: mk unregister <name-or-path>"))?;
                    let pack_root = resolve_pack_by_name_or_path(name_or_path)?;
                    #[cfg(feature = "helix")]
                    crate::helix_store::remove_helix_for_pack(&pack_root)?;
                    remove_pack(name_or_path)?;
                    if crate::term::color_stdout() {
                        println!("{} {}", "Pack removed from registry".green(), name_or_path);
                    } else {
                        println!("Pack removed from registry {}", name_or_path);
                    }
                    return Ok(());
                }
                CliCommand::Use { pack } => {
                    let reg = load_registry()?;
                    if let Some(name_or_path) = pack {
                        set_default(name_or_path)?;
                        if crate::term::color_stdout() {
                            println!("{} {}", "Default pack set to".green(), name_or_path);
                        } else {
                            println!("Default pack set to {}", name_or_path);
                        }
                    } else {
                        if let Some(ref default_path) = reg.default_path {
                            let default_pack = reg.packs.iter().find(|p| p.path == *default_path);
                            let (name, path) = default_pack
                                .map(|p| (p.name.as_deref().unwrap_or(p.path.as_str()), p.path.as_str()))
                                .unwrap_or((default_path.as_str(), default_path.as_str()));
                            if crate::term::color_stdout() {
                                println!("{} {}  {}", "Default pack:".bold(), name, path.dimmed());
                            } else {
                                println!("Default pack: {}  {}", name, path);
                            }
                        } else {
                            if crate::term::color_stdout() {
                                println!("{}", "No default pack set".yellow());
                            } else {
                                println!("No default pack set");
                            }
                        }
                    }
                    return Ok(());
                }
                _ => {}
            }

            let packs = packs_for_command(&cmd)?;
            let (guard, effective_cfg) = match &cmd {
                CliCommand::Index { .. } => {
                    let (g, c) = cli_client::ensure_server_standalone(&cfg, &packs).await?;
                    (g, c)
                }
                _ => {
                    let g = cli_client::ensure_server(&cfg, &packs).await?;
                    (g, cfg.clone())
                }
            };

            enum CommandOut {
                Done,
                Output(serde_json::Value),
            }
            let result: Result<CommandOut> = match cmd {
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
                            let pack_root = resolve_pack_root(pack.as_deref())?;
                            let mut body = body.clone();
                            if let Some(obj) = body.as_object_mut() {
                                obj.insert("path".to_string(), serde_json::Value::String(pack_root.to_string_lossy().to_string()));
                            }
                            let out = cli_client::add(&effective_cfg, &body).await?;
                            Ok(CommandOut::Output(out))
                        }
                    } else {
                        let path = local_path.as_deref().unwrap();
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
                            let pack_root = resolve_pack_root(pack.as_deref())?;
                            let pack_dir = pack_dir_for_path(&pack_root);
                            if !pack_dir.join("manifest.json").exists() {
                                anyhow::bail!(
                                    "no memory pack at {}. run `mk index {}` first",
                                    pack_root.display(),
                                    pack_root.display()
                                );
                            }
                            let (dest, pack_relative) = if source.is_dir() {
                                let name = source
                                    .file_name()
                                    .map(|s| s.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "unnamed".to_string());
                                let d = copy_dir_into_sources(&source, &pack_dir, &name)?;
                                (d, format!("sources/{}", name))
                            } else {
                                let d = copy_file_into_sources(&source, &pack_dir)?;
                                (d, "sources/_files".to_string())
                            };
                            add_source_root(&pack_dir, &pack_relative)?;
                            if crate::term::color_stdout() {
                                println!(
                                    "{} {} -> {}",
                                    "Copied".green(),
                                    source.display(),
                                    dest.display()
                                );
                            } else {
                                println!("Copied {} -> {}", source.display(), dest.display());
                            }
                            let out = cli_client::index(&effective_cfg, pack_root.to_string_lossy().as_ref(), None, false, ctx.output_format == OutputFormat::Json).await?;
                            cli_client::poll_until_index_done(&effective_cfg, pack_root.to_string_lossy().as_ref()).await?;
                            Ok(CommandOut::Output(out))
                        }
                    }
                }
                CliCommand::Status { dir } => {
                    let output_json = ctx.output_format == OutputFormat::Json;
                    if dir.is_none() {
                        let data = cli_client::list(&effective_cfg, output_json).await?;
                        if output_json {
                            let json_str = serde_json::to_string_pretty(&data)?;
                            println!("{}", json_str);
                        }
                        Ok(CommandOut::Done)
                    } else {
                        let data = cli_client::status(&effective_cfg, dir.as_deref()).await?;
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
                    let data = cli_client::list(&effective_cfg, output_json).await?;
                    if output_json {
                        println!("{}", serde_json::to_string_pretty(&data)?);
                    }
                    Ok(CommandOut::Done)
                }
                CliCommand::Index { dir, name } => {
                    let out = cli_client::index(&effective_cfg, &dir, name.as_deref(), ctx.dry_run, ctx.output_format == OutputFormat::Json).await?;
                    if !ctx.dry_run {
                        cli_client::poll_until_index_done(&effective_cfg, &dir).await?;
                    }
                    Ok(CommandOut::Output(out))
                }
                CliCommand::Graph { pack: _ } => {
                    cli_client::graph_show(&effective_cfg).await?;
                    Ok(CommandOut::Done)
                }
                CliCommand::Publish { pack, destination } => {
                    let out = cli_client::publish(
                        &effective_cfg,
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
                    let out = cli_client::query(&effective_cfg, &QueryArgs { query, top_k, use_reranker, raw }, pack.as_deref()).await?;
                    let use_formatted = !raw && ctx.output_format != OutputFormat::Json;
                    if use_formatted {
                        if let (Some(answer), Some(sources)) = (
                            out.get("answer").and_then(serde_json::Value::as_str),
                            out.get("sources").and_then(serde_json::Value::as_array),
                        ) {
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
                CliCommand::Help | CliCommand::Remove { .. } | CliCommand::Schema { .. } | CliCommand::Use { .. } | CliCommand::Unregister { .. } => unreachable!(),
            };
            guard.shutdown()?;
            let command_out = result?;
            if let CommandOut::Output(out) = command_out {
                let json_str = serde_json::to_string_pretty(&out)?;
                let output = if ctx.output_format == OutputFormat::Json || !crate::term::color_stdout() {
                    json_str
                } else {
                    to_colored_json_auto(&out).unwrap_or(json_str.clone())
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
    #[cfg(feature = "lance-falkor")]
    let falkordb_socket = env::var("FALKORDB_SOCKET").ok();
    #[cfg(not(feature = "lance-falkor"))]
    let falkordb_socket: Option<String> = None;
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
    run_server(packs, host, port, falkordb_socket).await?;
    Ok(())
}
