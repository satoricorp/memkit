mod add_docs;
mod cli;
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
#[cfg(feature = "llama-embedded")]
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

use crate::cli::types::{CliCommand, ServeConfig, UseField, UseSpec};
use crate::cli_client::{QueryArgs, ServerConfig};
use crate::pack::{has_manifest_at, init_pack};
use crate::registry::{
    ensure_registered, load_registry, pack_dir_for_path, resolve_pack_by_name_or_path,
    resolve_remove_pack_target, set_default,
};
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

/// Terminal width for wrapping query answers (`COLUMNS` or 80).
fn terminal_columns() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(80)
        .max(40)
}

/// Word-wrap `text` to at most `max_chars` Unicode scalar values per line.
fn wrap_paragraph(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let text = text.trim_end();
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if word_len > max_chars {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            let mut chunk = String::new();
            for ch in word.chars() {
                if chunk.chars().count() >= max_chars {
                    lines.push(std::mem::take(&mut chunk));
                }
                chunk.push(ch);
            }
            if !chunk.is_empty() {
                cur = chunk;
            }
            continue;
        }
        let add = if cur.is_empty() {
            word_len
        } else {
            cur.chars().count() + 1 + word_len
        };
        if cur.is_empty() {
            cur.push_str(word);
        } else if add <= max_chars {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Physical lines for the query answer: wrap to fit beside `❯ `, preserve explicit newlines.
fn query_answer_physical_lines(answer: &str, width: usize) -> Vec<String> {
    const PREFIX_CHARS: usize = 2; // "❯ "
    let content_width = width.saturating_sub(PREFIX_CHARS).max(8);
    let mut out = Vec::new();
    for line in answer.lines() {
        out.extend(wrap_paragraph(line, content_width));
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

/// Whether the resolved pack is registered as cloud (skip `file://` links for local CLI).
fn pack_cloud_for_cli(pack_arg: Option<&str>) -> bool {
    let Ok(root) = ensure_pack_root(pack_arg) else {
        return false;
    };
    let norm = root.canonicalize().unwrap_or(root);
    let reg = load_registry().unwrap_or_default();
    for p in &reg.packs {
        let Ok(entry_path) = PathBuf::from(&p.path).canonicalize() else {
            continue;
        };
        if entry_path == norm {
            return p.cloud;
        }
    }
    false
}

fn parse_pack_paths(value: &str) -> Vec<PathBuf> {
    value
        .split(',')
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

fn is_agent_json_flag(s: &str) -> bool {
    matches!(s, "--json" | "-j")
}

fn parse_use_field_json(v: Option<&serde_json::Value>) -> Result<UseField> {
    match v {
        None => Ok(UseField::Absent),
        Some(serde_json::Value::Null) => Ok(UseField::Show),
        Some(serde_json::Value::String(s)) => Ok(UseField::Set(s.clone())),
        _ => Err(anyhow!("use: pack and model must be null or a string")),
    }
}

/// `{"command":"use"}` with optional `"pack"` / `"model"` (null = show, string = set).
fn cli_use_from_json(j: &serde_json::Value) -> Result<CliCommand> {
    let obj = j.as_object().ok_or_else(|| anyhow!("--json must be a JSON object"))?;
    let has_pack = obj.contains_key("pack");
    let has_model = obj.contains_key("model");
    if !has_pack && !has_model {
        return Ok(CliCommand::Use(UseSpec {
            pack: UseField::Show,
            model: UseField::Show,
        }));
    }
    Ok(CliCommand::Use(UseSpec {
        pack: if has_pack {
            parse_use_field_json(obj.get("pack"))?
        } else {
            UseField::Absent
        },
        model: if has_model {
            parse_use_field_json(obj.get("model"))?
        } else {
            UseField::Absent
        },
    }))
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

fn parse_headless_start(args: &[String]) -> Result<Option<ServeConfig>> {
    let is_headless = args.first().map(|a| a == "--headless-start").unwrap_or(false);
    if !is_headless {
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
        "schema" => {
            let format = match get_str("format") {
                Some(f) => crate::cli::schema::schema_format_from_str(&f)?,
                None => crate::cli::schema::SchemaFormat::Memkit,
            };
            Ok(CliCommand::Schema {
                command: get_str("schema"),
                format,
            })
        }
        "start" => {
            let pack = get_str("pack");
            let host = get_str("host");
            let port = get_u64("port")
                .map(|p| u16::try_from(p).map_err(|_| anyhow!("invalid port value")))
                .transpose()?;
            let foreground = get_bool("foreground").unwrap_or(false);
            Ok(CliCommand::Serve {
                pack,
                host,
                port,
                foreground,
            })
        }
        "stop" => {
            let port = get_u64("port")
                .map(|p| u16::try_from(p).map_err(|_| anyhow!("invalid port value")))
                .transpose()?;
            Ok(CliCommand::Stop { port })
        }
        "use" => cli_use_from_json(j),
        "list" => Ok(CliCommand::List),
        "doctor" => Ok(CliCommand::Doctor),
        "version" => Ok(CliCommand::Version),
        "help" | "--help" | "-h" => Ok(CliCommand::Help),
        other => Err(anyhow!("unknown command: {}. run `mk help` for usage", other)),
    }
}

fn parse_cli_command(args: &[String]) -> Result<CliCommand> {
    if args.is_empty() {
        return Ok(CliCommand::Help);
    }

    if args.len() == 1 {
        match args[0].as_str() {
            "--version" | "-V" | "version" => return Ok(CliCommand::Version),
            _ => {}
        }
    }

    // Agent mode: mk --json | -j '<one JSON object with "command">'
    if args.len() >= 2 && is_agent_json_flag(&args[0]) {
        let j: serde_json::Value = serde_json::from_str(&args[1])
            .map_err(|e| anyhow!("invalid JSON after {}: {}", args[0], e))?;
        let command = j
            .get("command")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("--json must include \"command\""))?;
        return cli_command_from_json(command, &j);
    }

    match args[0].as_str() {
        "add" => {
            let rest = &args[1..];
            let pack_from_rest = flag_value(rest, "--pack");
            let arg = rest
                .first()
                .cloned()
                .ok_or_else(|| anyhow!("usage: mk add <path-or-url> [--pack <dir>] — or mk --json '{{\"command\":\"add\",...}}'"))?;
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
            let rest = &args[1..];
            let mut yes = false;
            let mut dir = None::<String>;
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
            let rest = &args[1..];
            let dir = rest.first().cloned();
            if let Some(ref d) = dir {
                crate::validate::validate_path(d)?;
            }
            Ok(CliCommand::Status { dir })
        }
        "query" => {
            let rest = &args[1..];
            if rest.is_empty() {
                return Err(anyhow!(
                    "usage: mk query <text> [--top-k N] [--no-rerank] [--pack <name-or-path>] [--raw] — or mk --json '{{\"command\":\"query\",\"query\":\"...\"}}'"
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
            let rest = &args[1..];
            let pack = flag_value(rest, "--pack");
            let destination = flag_value(rest, "--destination");
            Ok(CliCommand::Publish { pack, destination })
        }
        "schema" => {
            let rest = &args[1..];
            let mut format = crate::cli::schema::SchemaFormat::Memkit;
            let mut i = 0usize;
            while i + 1 < rest.len() && rest[i] == "--format" {
                format = crate::cli::schema::schema_format_from_str(&rest[i + 1])?;
                i += 2;
            }
            let command = rest.get(i).cloned();
            Ok(CliCommand::Schema { command, format })
        }
        "use" => {
            let rest = &args[1..];
            if rest.is_empty()
                || (rest.len() == 1 && matches!(rest[0].as_str(), "--help" | "-h"))
            {
                return Ok(CliCommand::Help);
            }
            if rest.len() != 2 {
                return Err(anyhow!(
                    "usage: mk use pack <name-or-path> | mk use model <model-id> — run `mk list` for current defaults"
                ));
            }
            match rest[0].as_str() {
                "pack" => Ok(CliCommand::Use(UseSpec {
                    pack: UseField::Set(rest[1].clone()),
                    model: UseField::Absent,
                })),
                "model" => Ok(CliCommand::Use(UseSpec {
                    pack: UseField::Absent,
                    model: UseField::Set(rest[1].clone()),
                })),
                _ => Err(anyhow!(
                    "usage: mk use pack <name-or-path> | mk use model <model-id> — run `mk list` for current defaults"
                )),
            }
        }
        "list" => Ok(CliCommand::List),
        "doctor" => Ok(CliCommand::Doctor),
        "start" => {
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

fn run_use(spec: &UseSpec) -> Result<()> {
    match &spec.pack {
        UseField::Set(name) => {
            set_default(name)?;
            println!(
                "{} {}",
                crate::term::style_stdout("Default pack set to", |s| s.green().to_string()),
                name
            );
        }
        UseField::Show => {
            let reg = load_registry()?;
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
        UseField::Absent => {}
    }
    match &spec.model {
        UseField::Set(id) => {
            if !config::is_supported_model(id) {
                anyhow::bail!(
                    "unknown model '{}'. run `mk list` to see supported models.",
                    id
                );
            }
            config::set_model(id)?;
            println!(
                "{} {}",
                crate::term::style_stdout("Default model set to", |s| s.green().to_string()),
                id
            );
        }
        UseField::Show => {
            let cfg = config::load_config().unwrap_or_default();
            if let Some(ref m) = cfg.model {
                println!(
                    "{} {}",
                    crate::term::style_stdout("Default model:", |s| s.bold().to_string()),
                    m
                );
            } else {
                println!(
                    "{}",
                    crate::term::style_stdout("No default model set", |s| s.yellow().to_string())
                );
            }
        }
        UseField::Absent => {}
    }
    Ok(())
}

fn models_json_value() -> serde_json::Value {
    let cfg = config::load_config().unwrap_or_default();
    let supported = config::supported_models();
    serde_json::json!({
        "current": cfg.model,
        "supported": supported.iter().map(|(id, desc)| serde_json::json!({"id": id, "description": desc})).collect::<Vec<_>>()
    })
}

fn print_models_section() {
    let cfg = config::load_config().unwrap_or_default();
    let supported = config::supported_models();
    let c = crate::term::color_stdout();
    println!();
    println!("{}", crate::term::section_title(c, "Available Models:"));
    for (id, desc) in &supported {
        let mark = if cfg.model.as_deref() == Some(*id) {
            "[*]"
        } else {
            "[ ]"
        };
        if c {
            println!(
                "  {}  {}  {}",
                crate::term::magenta_words(c, mark),
                crate::term::data_num(c, *id),
                crate::term::dimmed_word(c, desc)
            );
        } else {
            println!("  {}  {}  {}", mark, id, desc);
        }
    }
    println!();
    println!(
        "  {}",
        crate::term::dimmed_word(
            c,
            "Run 'mk use model <id>' to set a default model."
        )
    );
}

/// `tail` includes any leading spaces after the subcommand (e.g. ` " <args>…"` or ` "   (note)"`).
fn print_help_cmd_line_grouped(c: bool, sub: &str, tail: &str) {
    println!(
        "    {} {}{}",
        crate::term::mk_binary(c),
        crate::term::bold_word(c, sub),
        crate::term::dimmed_word(c, tail)
    );
}

fn print_help_section(c: bool, title: &str, first: bool) {
    if !first {
        println!();
    }
    println!("  {}", crate::term::section_title(c, title));
}

fn print_help() {
    let c = crate::term::color_stdout();
    println!("{}", crate::term::title_app(c));
    println!(
        "{}",
        crate::term::dimmed_word(c, &format!("version {}", crate::term::PKG_VERSION))
    );
    println!();
    println!("{}", crate::term::dimmed_word(c, "Usage:"));
    println!(
        "  {} {} {}{}",
        crate::term::dimmed_word(c, "Agent JSON:"),
        crate::term::mk_binary(c),
        crate::term::bold_word(c, "-j"),
        crate::term::dimmed_word(c, " '<JSON>'  (same as --json / --mjson; object must include \"command\")")
    );
    println!(
        "  {}",
        crate::term::dimmed_word(c, "Global flags: [--output json|text] [--dry-run] [--version | -V]")
    );
    println!();
    println!("{}", crate::term::dimmed_word(c, "Commands:"));
    println!();
    print_help_section(c, "Storage", true);
    print_help_cmd_line_grouped(c, "add", " <path-or-url> [--pack <name-or-path>]");
    print_help_cmd_line_grouped(c, "remove", " [dir]");
    print_help_cmd_line_grouped(
        c,
        "status",
        " [dir]   (omit dir to list all registered packs)",
    );
    print_help_cmd_line_grouped(
        c,
        "publish",
        " [--pack <name-or-path>] [--destination s3://bucket/prefix]",
    );
    print_help_section(c, "Search", false);
    print_help_cmd_line_grouped(
        c,
        "query",
        " <text> [--top-k N] [--no-rerank] [--pack <name-or-path>] [--raw]",
    );
    print_help_section(c, "Defaults", false);
    print_help_cmd_line_grouped(
        c,
        "list",
        "   (registered packs and current/supported models)",
    );
    print_help_cmd_line_grouped(c, "use pack", " <name-or-path>   (set default pack)");
    print_help_cmd_line_grouped(c, "use model", " <model-id>   (set default model)");
    print_help_section(c, "Server", false);
    print_help_cmd_line_grouped(c, "start", " [--pack <path>] [--host H] [--port P] [--foreground]");
    print_help_cmd_line_grouped(c, "stop", " [--port P]");
    print_help_section(c, "Diagnostics & Schemas", false);
    print_help_cmd_line_grouped(c, "doctor", "   (config path + server /health reachability)");
    print_help_cmd_line_grouped(c, "schema", " [--format json|json-schema] [command]");
}

fn print_version() {
    println!("memkit {}", crate::term::PKG_VERSION);
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
async fn main() {
    if let Err(e) = run().await {
        crate::term::error(&e);
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    dotenvy::dotenv().ok();
    load_memkit_google_json_from_dotenv_fallback();
    let args: Vec<String> = env::args().skip(1).collect();
    let (args, ctx) = parse_global_flags(&args);

    config::ensure_config_exists().context("failed to create config (e.g. ~/.config/memkit/memkit.json)")?;

    if let Some(cfg) = parse_headless_start(&args)? {
        serve_with_startup(cfg.packs, cfg.host, cfg.port).await?;
        return Ok(());
    }

    match parse_cli_command(&args)? {
        CliCommand::Version => print_version(),
        CliCommand::Help => print_help(),
        CliCommand::Schema { command, format } => {
            crate::cli::schema::print_schema(command.as_deref(), format)?;
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
                let cfg = ServerConfig::for_cli_serve(host.clone(), port);
                cli_client::wait_for_server_ready(&cfg).await?;
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
                crate::registry::default_serve_pack_paths()?
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
            if cli_client::stop_server_on_port(port)? {
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
                    let target = resolve_remove_pack_target(dir.as_deref())?;
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
                CliCommand::Use(spec) => {
                    run_use(&spec)?;
                    return Ok(());
                }
                _ => {}
            }

            let commands_need_server = !matches!(
                cmd,
                CliCommand::Help
                    | CliCommand::Version
                    | CliCommand::Schema { .. }
                    | CliCommand::Use(_)
                    | CliCommand::Doctor
                    | CliCommand::Serve { .. }
                    | CliCommand::Stop { .. }
            );
            if matches!(&cmd, CliCommand::Doctor) {
                cli_client::print_server_note_doctor(&cfg, ctx.output_format == OutputFormat::Json)
                    .await;
            }
            if commands_need_server {
                let readonly_no_autostart =
                    matches!(&cmd, CliCommand::Status { .. } | CliCommand::List);
                if readonly_no_autostart {
                    cli_client::require_server_running(&cfg).await?;
                } else {
                    cli_client::ensure_server(&cfg).await?;
                }
                cli_client::print_server_note_running(&cfg, ctx.output_format == OutputFormat::Json);
            }

            enum CommandOut {
                Done,
                Output(serde_json::Value),
            }
            let result: Result<CommandOut> = match cmd {
                CliCommand::Remove { dir, yes: _ } => {
                    let target = resolve_remove_pack_target(dir.as_deref())?;
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
                            let mut out = cli_client::add(&cfg, &body).await?;
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
                                let data = cli_client::status(&cfg, Some(&pack_path)).await?;
                                if ctx.output_format != OutputFormat::Json {
                                    cli_client::print_status(&data);
                                }
                                cli_client::merge_job_into_add_output_after_poll(&mut out, &data);
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
                    let pack_data = cli_client::list(&cfg, output_json).await?;
                    if output_json {
                        let merged = serde_json::json!({
                            "packs": pack_data,
                            "models": models_json_value(),
                        });
                        println!("{}", serde_json::to_string_pretty(&merged)?);
                    } else {
                        print_models_section();
                    }
                    Ok(CommandOut::Done)
                }
                CliCommand::Doctor => {
                    let data = cli_client::doctor(&cfg).await?;
                    if ctx.output_format == OutputFormat::Json {
                        println!("{}", serde_json::to_string_pretty(&data)?);
                    } else {
                        let c = crate::term::color_stdout();
                        let url = data["server_url"].as_str().unwrap_or("");
                        let reachable = data["server_reachable"].as_bool().unwrap_or(false);
                        if reachable {
                            print!("Server is reachable ");
                            println!("{}", crate::term::bracketed_cyan(c, url));
                        } else {
                            print!("Server is ");
                            print!("{}", crate::term::danger_words(c, "not reachable"));
                            print!(" ");
                            println!("{}", crate::term::bracketed_cyan(c, url));
                        }
                        let path = data["config_path"].as_str().unwrap_or("");
                        let config_ok = data["config_exists"].as_bool().unwrap_or(false);
                        if config_ok {
                            print!("Configuration is valid ");
                            println!("{}", crate::term::bracketed_cyan(c, path));
                        } else {
                            print!("Configuration file missing ");
                            println!("{}", crate::term::bracketed_cyan(c, path));
                        }
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
                    let pack_cloud = pack_cloud_for_cli(pack.as_deref());
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
                                        let path_shown =
                                            crate::google::cli_source_link(path, pack_cloud);
                                        let content = r.get("content").and_then(serde_json::Value::as_str).unwrap_or("");
                                        let preview = if content.len() > 120 { format!("{}...", &content[..120]) } else { content.to_string() };
                                        println!(
                                            "  {}. {} {}",
                                            i + 1,
                                            crate::term::style_stdout(&path_shown, |s| s.dimmed().to_string()),
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
                                        let path_shown =
                                            crate::google::cli_source_link(path, pack_cloud);
                                        let score = r.get("score").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
                                        let content = r.get("content").and_then(serde_json::Value::as_str).unwrap_or("");
                                        println!(
                                            "  {}. {} score={:.3}",
                                            i + 1,
                                            crate::term::style_stdout(&path_shown, |s| s.dimmed().to_string()),
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
                                if !model.is_empty() {
                                    let c = crate::term::color_stdout();
                                    println!(
                                        "{}{}",
                                        crate::term::magenta_words(c, &format!("model: {}", model)),
                                        crate::term::dimmed_word(
                                            c,
                                            " (run `mk use model` to change model)",
                                        )
                                    );
                                    println!();
                                }
                            }
                            let c = crate::term::color_stdout();
                            let physical = query_answer_physical_lines(answer, terminal_columns());
                            for (i, line) in physical.iter().enumerate() {
                                if i == 0 {
                                    println!(
                                        "{} {}",
                                        crate::term::dimmed_word(c, "❯"),
                                        line
                                    );
                                } else {
                                    println!("  {}", line);
                                }
                            }
                            if !sources.is_empty() {
                                println!();
                                println!("Sources:");
                                for s in sources.iter().take(5) {
                                    let path = s
                                        .get("path")
                                        .and_then(serde_json::Value::as_str)
                                        .unwrap_or("?");
                                    let shown =
                                        crate::google::cli_source_link(path, pack_cloud);
                                    println!("  {}", shown);
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
                CliCommand::Help
                | CliCommand::Version
                | CliCommand::Schema { .. }
                | CliCommand::Use(_)
                | CliCommand::Serve { .. }
                | CliCommand::Stop { .. } => unreachable!(),
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
            crate::term::dimmed_word(color, "serving pack"),
            crate::term::bold_word(color, &pack_display),
            crate::term::dimmed_word(color, "on"),
            crate::term::data_num(color, &host),
            crate::term::data_num(color, &port.to_string())
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
