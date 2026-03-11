mod add_docs;
mod cli_client;
mod extract;
mod file_tree;
mod memkit_txt;
mod registry;
mod validate;
mod embed;
mod term;
mod falkor_store;
mod indexer;
mod lancedb_store;
mod ontology;
mod ontology_candle;
mod ontology_llama;
mod pack;
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
use crate::pack::{copy_file_to_pack, scrub_pack_from_dir};
use crate::registry::{load_registry, pack_dir_for_path};
use crate::server::run_server;

struct ServeConfig {
    packs: Vec<PathBuf>,
    host: String,
    port: u16,
}

enum CliCommand {
    Serve(ServeConfig),
    Add { path: String, pack: Option<String> },
    Remove { dir: Option<String> },
    Status { dir: Option<String> },
    List,
    Index { dir: String },
    Graph { pack: Option<String> },
    Query {
        query: String,
        top_k: usize,
        use_reranker: bool,
        raw: bool,
        pack: Option<String>,
    },
    Schema { command: Option<String> },
    Help,
}

fn resolve_pack_root(pack_arg: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = pack_arg {
        let path = PathBuf::from(p)
            .canonicalize()
            .with_context(|| format!("pack path not found: {}", p))?;
        if path.join(".memkit/manifest.json").exists() {
            return Ok(path);
        }
        if path.join("manifest.json").exists() {
            return Ok(path);
        }
        anyhow::bail!("no memory pack at {}", path.display());
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
        "no memory pack found. use --pack <dir> or run `mk index <dir>` first"
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
    let is_serve = args.first().map(|a| a == "serve" || a == "--headless-serve").unwrap_or(false);
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

fn parse_cli_command(args: &[String]) -> Result<CliCommand> {
    if args.is_empty() {
        return Ok(CliCommand::Help);
    }

    if let Some(cfg) = parse_serve(args)? {
        return Ok(CliCommand::Serve(cfg));
    }

    match args[0].as_str() {
        "add" => {
            let (json_val, rest) = extract_json_from_args(&args[1..]);
            if let Some(j) = json_val {
                let path = j
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from)
                    .ok_or_else(|| anyhow!("--json must include \"path\""))?;
                let pack = j.get("pack").and_then(serde_json::Value::as_str).map(String::from);
                crate::validate::validate_path(&path)?;
                return Ok(CliCommand::Add { path, pack });
            }
            let path = rest
                .first()
                .cloned()
                .ok_or_else(|| anyhow!("usage: mk add <path> [--pack <dir>] or mk add --json '{{\"path\":\"...\"}}'"))?;
            crate::validate::validate_path(&path)?;
            let mut pack = None;
            let mut i = 1usize;
            while i < rest.len() {
                if rest[i] == "--pack" && rest.get(i + 1).is_some() {
                    pack = rest.get(i + 1).cloned();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            Ok(CliCommand::Add { path, pack })
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
                return Ok(CliCommand::Index { dir });
            }
            let dir = rest
                .first()
                .cloned()
                .ok_or_else(|| anyhow!("usage: mk index <dir> or mk index --json '{{\"dir\":\"...\"}}'"))?;
            crate::validate::validate_path(&dir)?;
            Ok(CliCommand::Index { dir })
        }
        "graph" => {
            let (json_val, rest) = extract_json_from_args(&args[1..]);
            if let Some(j) = json_val {
                let pack = j.get("pack").and_then(serde_json::Value::as_str).map(String::from);
                if let Some(ref p) = pack {
                    crate::validate::validate_path(p)?;
                }
                return Ok(CliCommand::Graph { pack });
            }
            let mut pack = None;
            let mut i = 0usize;
            while i < rest.len() {
                if rest[i] == "--pack" && rest.get(i + 1).is_some() {
                    pack = rest.get(i + 1).cloned();
                    crate::validate::validate_path(pack.as_ref().unwrap())?;
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
                if let Some(ref p) = pack {
                    crate::validate::validate_path(p)?;
                }
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
                    "usage: mk query <text> [--top-k N] [--no-rerank] [--pack <dir>] [--raw] or mk query --json '{{\"query\":\"...\"}}'"
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
                        if let Some(ref p) = pack {
                            crate::validate::validate_path(p)?;
                        }
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
        "schema" => {
            let command = args.get(1).cloned();
            Ok(CliCommand::Schema { command })
        }
        "help" | "--help" | "-h" => Ok(CliCommand::Help),
        other => Err(anyhow!(
            "unknown command: {}. run `mk help` for usage",
            other
        )),
    }
}

const SCHEMA_COMMANDS: &[&str] = &["add", "remove", "status", "index", "graph", "query"];

fn schema_for_command(cmd: &str) -> Option<serde_json::Value> {
    Some(match cmd {
        "add" => serde_json::json!({
            "command": "add",
            "input": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to file or directory to add"},
                    "pack": {"type": "string", "description": "Pack directory (optional)"}
                },
                "required": ["path"]
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
                    "dir": {"type": "string", "description": "Directory to index"}
                },
                "required": ["dir"]
            }
        }),
        "graph" => serde_json::json!({
            "command": "graph",
            "input": {
                "type": "object",
                "properties": {
                    "pack": {"type": "string", "description": "Pack directory (optional)"}
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
                    "pack": {"type": "string", "description": "Pack directory (optional)"}
                },
                "required": ["query"]
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
    println!("  Global flags: [--output json|text] [--dry-run]");
    println!();
    let commands = [
        "  mk serve [--pack <path>] [--host <host>] [--port <port>]",
        "  mk add <path> [--pack <dir>]",
        "  mk remove [dir]",
        "  mk status [dir]",
        "  mk list",
        "  mk index <dir>",
        "  mk graph [--pack <dir>]",
        "  mk query <text> [--top-k N] [--no-rerank] [--pack <dir>] [--raw]",
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
    let args: Vec<String> = env::args().skip(1).collect();
    let (args, ctx) = parse_global_flags(&args);

    match parse_cli_command(&args)? {
        CliCommand::Help => print_help(),
        CliCommand::Schema { command } => {
            print_schema(command.as_deref())?;
            return Ok(());
        }
        CliCommand::Serve(cfg) => {
            serve_with_startup(cfg.packs, cfg.host, cfg.port).await?;
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
                    scrub_pack_from_dir(&target)?;
                    if crate::term::color_stdout() {
                        println!("{} scrubbed from {}", "Memory pack removed".green(), target.display());
                    } else {
                        println!("Memory pack removed from {}", target.display());
                    }
                    return Ok(());
                }
                _ => {}
            }

            cli_client::ensure_server(&cfg).await?;
            let out = match cmd {
                CliCommand::Add { path, pack } => {
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
                        return Ok(());
                    }
                    let source = PathBuf::from(&path)
                        .canonicalize()
                        .with_context(|| format!("file not found: {}", path))?;
                    let pack_root = resolve_pack_root(pack.as_deref())?;
                    let pack_dir = pack_dir_for_path(&pack_root);
                    if !pack_dir.join("manifest.json").exists() {
                        anyhow::bail!(
                            "no memory pack at {}. run `mk index {}` first",
                            pack_root.display(),
                            pack_root.display()
                        );
                    }
                    let dest = copy_file_to_pack(&source, &pack_root)?;
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
                    cli_client::index(&cfg, pack_root.to_string_lossy().as_ref(), false, ctx.output_format == OutputFormat::Json).await?
                }
                CliCommand::Status { dir } => {
                    let output_json = ctx.output_format == OutputFormat::Json;
                    if dir.is_none() {
                        let data = cli_client::list(&cfg, output_json).await?;
                        if output_json {
                            let json_str = serde_json::to_string_pretty(&data)?;
                            println!("{}", json_str);
                        }
                        return Ok(());
                    }
                    let data = cli_client::status(&cfg, dir.as_deref()).await?;
                    if output_json {
                        println!("{}", serde_json::to_string_pretty(&data)?);
                    } else {
                        cli_client::print_status(&data);
                    }
                    return Ok(());
                }
                CliCommand::List => {
                    let output_json = ctx.output_format == OutputFormat::Json;
                    let data = cli_client::list(&cfg, output_json).await?;
                    if output_json {
                        println!("{}", serde_json::to_string_pretty(&data)?);
                    }
                    return Ok(());
                }
                CliCommand::Index { dir } => {
                    cli_client::index(&cfg, &dir, ctx.dry_run, ctx.output_format == OutputFormat::Json).await?
                }
                CliCommand::Graph { pack: _ } => {
                    cli_client::graph_show(&cfg).await?;
                    return Ok(());
                }
                CliCommand::Query { query, top_k, use_reranker, raw, pack } => {
                    let out = cli_client::query(&cfg, &QueryArgs { query, top_k, use_reranker, raw }, pack.as_deref()).await?;
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
                            return Ok(());
                        }
                    }
                    out
                }
                CliCommand::Help | CliCommand::Serve(_) | CliCommand::Remove { .. } | CliCommand::Schema { .. } => unreachable!(),
            };
            let json_str = serde_json::to_string_pretty(&out)?;
            let output = if ctx.output_format == OutputFormat::Json || !crate::term::color_stdout() {
                json_str
            } else {
                to_colored_json_auto(&out).unwrap_or(json_str.clone())
            };
            println!("{}", output);
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
    let falkordb_socket = env::var("FALKORDB_SOCKET").ok();
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
