mod cli_client;
mod file_tree;
mod memkit_txt;
mod registry;
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
mod server;
mod types;

use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Result, anyhow};
use colored_json::to_colored_json_auto;
use owo_colors::OwoColorize;

use crate::cli_client::{ServerConfig, QueryArgs};
use crate::indexer::run_index;
use crate::pack::load_manifest;
use crate::server::run_server;

struct ServeConfig {
    pack: PathBuf,
    host: String,
    port: u16,
}

enum CliCommand {
    Serve(ServeConfig),
    Status { dir: Option<String> },
    List,
    Index { dir: String },
    Graph { pack: Option<String> },
    Query {
        query: String,
        mode: String,
        top_k: usize,
        raw: bool,
        pack: Option<String>,
    },
    Help,
}

fn parse_serve(args: &[String]) -> Result<Option<ServeConfig>> {
    let is_serve = args.first().map(|a| a == "serve" || a == "--headless-serve").unwrap_or(false);
    if !is_serve {
        return Ok(None);
    }

    let mut pack = PathBuf::from(
        env::var("MEMKIT_PACK_PATH")
            .or_else(|_| env::var("MEMORY_PACK_PATH"))
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "./memory-pack".to_string()),
    );
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
                pack = PathBuf::from(v);
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

    Ok(Some(ServeConfig { pack, host, port }))
}

fn parse_cli_command(args: &[String]) -> Result<CliCommand> {
    if args.is_empty() {
        return Ok(CliCommand::Help);
    }

    if let Some(cfg) = parse_serve(args)? {
        return Ok(CliCommand::Serve(cfg));
    }

    match args[0].as_str() {
        "status" => {
            let dir = args.get(1).cloned();
            Ok(CliCommand::Status { dir })
        }
        "list" => Ok(CliCommand::List),
        "index" => {
            let dir = args
                .get(1)
                .cloned()
                .ok_or_else(|| anyhow!("usage: mk index <dir>"))?;
            Ok(CliCommand::Index { dir })
        }
        "graph" => {
            let mut pack = None;
            let mut i = 1usize;
            while i < args.len() {
                if args[i] == "--pack" && args.get(i + 1).is_some() {
                    pack = args.get(i + 1).cloned();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            Ok(CliCommand::Graph { pack })
        }
        "query" => {
            if args.len() < 2 {
                return Err(anyhow!(
                    "usage: mk query <text> [--mode hybrid|vector] [--top-k N] [--pack <dir>] [--raw]"
                ));
            }
            let query = args[1].clone();
            let mut mode = "hybrid".to_string();
            let mut top_k = 8usize;
            let mut raw = false;
            let mut pack = None;
            let mut i = 2usize;
            while i < args.len() {
                match args[i].as_str() {
                    "--mode" => {
                        i += 1;
                        mode = args
                            .get(i)
                            .cloned()
                            .ok_or_else(|| anyhow!("missing value for --mode"))?;
                    }
                    "--top-k" => {
                        i += 1;
                        let v = args
                            .get(i)
                            .ok_or_else(|| anyhow!("missing value for --top-k"))?;
                        top_k = v
                            .parse::<usize>()
                            .map_err(|_| anyhow!("invalid --top-k value: {}", v))?;
                    }
                    "--pack" => {
                        i += 1;
                        pack = args.get(i).cloned();
                    }
                    "--raw" => raw = true,
                    other => return Err(anyhow!("unsupported query argument: {}", other)),
                }
                i += 1;
            }
            Ok(CliCommand::Query { query, mode, top_k, raw, pack })
        }
        "help" | "--help" | "-h" => Ok(CliCommand::Help),
        other => Err(anyhow!(
            "unknown command: {}. run `mk help` for usage",
            other
        )),
    }
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
    let commands = [
        "  mk serve [--pack <path>] [--host <host>] [--port <port>]",
        "  mk status [dir]",
        "  mk list",
        "  mk index <dir>",
        "  mk graph [--pack <dir>]",
        "  mk query <text> [--mode hybrid|vector] [--top-k N] [--pack <dir>] [--raw]",
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

    match parse_cli_command(&args)? {
        CliCommand::Help => print_help(),
        CliCommand::Serve(cfg) => {
            serve_with_startup(cfg.pack, cfg.host, cfg.port).await?;
        }
        cmd => {
            let cfg = ServerConfig::from_env();
            cli_client::ensure_server(&cfg).await?;
            let out = match cmd {
                CliCommand::Status { dir } => {
                    if dir.is_none() {
                        cli_client::list(&cfg).await?;
                        return Ok(());
                    }
                    let data = cli_client::status(&cfg, dir.as_deref()).await?;
                    cli_client::print_status(&data);
                    return Ok(());
                }
                CliCommand::List => {
                    cli_client::list(&cfg).await?;
                    return Ok(());
                }
                CliCommand::Index { dir } => cli_client::index(&cfg, &dir).await?,
                CliCommand::Graph { pack: _ } => {
                    cli_client::graph_show(&cfg).await?;
                    return Ok(());
                }
                CliCommand::Query { query, mode, top_k, raw, pack } => {
                    let out = cli_client::query(&cfg, &QueryArgs { query, mode, top_k, raw }, pack.as_deref()).await?;
                    if !raw {
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
                CliCommand::Help | CliCommand::Serve(_) => unreachable!(),
            };
            let json_str = serde_json::to_string_pretty(&out)?;
            let output = if crate::term::color_stdout() {
                to_colored_json_auto(&out).unwrap_or(json_str.clone())
            } else {
                json_str
            };
            println!("{}", output);
        }
    }

    Ok(())
}

pub(crate) async fn serve_with_startup(pack: PathBuf, host: String, port: u16) -> Result<()> {
    let manifest = load_manifest(&pack).ok();
    let sources: Vec<PathBuf> = manifest
        .as_ref()
        .map(|m| m.sources.iter().map(|s| PathBuf::from(&s.root_path)).collect())
        .unwrap_or_default();
    let color = crate::term::color_stdout();
    if !sources.is_empty() {
        let (scanned, updated, chunks) = run_index(&pack, &sources)?;
        if color {
            println!(
                "{} scanned={} updated_files={} chunks={}",
                "startup index complete:".green(),
                scanned.to_string().cyan(),
                updated.to_string().cyan(),
                chunks.to_string().cyan()
            );
        } else {
            println!(
                "startup index complete: scanned={} updated_files={} chunks={}",
                scanned, updated, chunks
            );
        }
    } else {
        if color {
            println!("{}", "startup index skipped: no sources configured in manifest".yellow());
        } else {
            println!("startup index skipped: no sources configured in manifest");
        }
    }
    let port = env::var("API_PORT")
        .ok()
        .and_then(|p| u16::from_str(&p).ok())
        .unwrap_or(port);
    let falkordb_socket = env::var("FALKORDB_SOCKET").ok();
    if color {
        println!(
            "{} {} {} {}:{}",
            "serving pack".cyan(),
            pack.display().to_string().bold(),
            "on".cyan(),
            host.cyan(),
            port.to_string().cyan()
        );
    } else {
        println!("serving pack {} on {}:{}", pack.display(), host, port);
    }
    run_server(pack, host, port, falkordb_socket).await?;
    Ok(())
}
