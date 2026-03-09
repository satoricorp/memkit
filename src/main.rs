mod cli_client;
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
mod tui;
mod types;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Result, anyhow};
use colored_json::to_colored_json_auto;
use owo_colors::OwoColorize;

use crate::cli_client::{DaemonConfig, QueryArgs};
use crate::indexer::run_index;
use crate::pack::load_manifest;
use crate::server::run_server;

struct HeadlessServeConfig {
    pack: PathBuf,
    host: String,
    port: u16,
}

enum CliCommand {
    Status,
    Index,
    GraphShow,
    Pack,
    Query {
        query: String,
        mode: String,
        top_k: usize,
        raw: bool,
    },
    SourcesList,
    SourcesAdd {
        path: String,
    },
    SourcesRemove {
        path: String,
    },
    JobsList,
    JobsStatus {
        id: String,
    },
    OntologyList,
    OntologyShow {
        source: String,
    },
    OntologyExport {
        source: String,
        out: Option<String>,
    },
    Help,
    Tui,
}

fn parse_headless_serve(args: &[String]) -> Result<Option<HeadlessServeConfig>> {
    if args.is_empty() {
        return Ok(None);
    }

    if args.first().is_none_or(|a| a != "--headless-serve") {
        return Ok(None);
    }

    let mut pack = PathBuf::from(
        env::var("MEMORY_PACK_PATH")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "./memory-pack".to_string()),
    );
    let mut host = "127.0.0.1".to_string();
    let mut port = 7821u16;

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

    Ok(Some(HeadlessServeConfig { pack, host, port }))
}

fn parse_cli_command(args: &[String]) -> Result<CliCommand> {
    if args.is_empty() {
        return Ok(CliCommand::Help);
    }

    match args[0].as_str() {
        "status" => Ok(CliCommand::Status),
        "index" => Ok(CliCommand::Index),
        "graph" => {
            if args.get(1).map(|s| s.as_str()) == Some("show") {
                Ok(CliCommand::GraphShow)
            } else {
                Err(anyhow!("usage: satori graph show"))
            }
        }
        "pack" => Ok(CliCommand::Pack),
        "query" => {
            if args.len() < 2 {
                return Err(anyhow!(
                    "usage: satori query <text> [--mode hybrid|vector] [--top-k N] [--raw]"
                ));
            }
            let query = args[1].clone();
            let mut mode = "hybrid".to_string();
            let mut top_k = 8usize;
            let mut raw = false;
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
                    "--raw" => raw = true,
                    other => return Err(anyhow!("unsupported query argument: {}", other)),
                }
                i += 1;
            }
            Ok(CliCommand::Query { query, mode, top_k, raw })
        }
        "sources" => {
            if args.len() < 2 {
                return Err(anyhow!("usage: satori sources <list|add|remove> [path]"));
            }
            match args[1].as_str() {
                "list" => Ok(CliCommand::SourcesList),
                "add" => {
                    let path = args
                        .get(2)
                        .cloned()
                        .ok_or_else(|| anyhow!("usage: satori sources add <path>"))?;
                    Ok(CliCommand::SourcesAdd { path })
                }
                "remove" => {
                    let path = args
                        .get(2)
                        .cloned()
                        .ok_or_else(|| anyhow!("usage: satori sources remove <path>"))?;
                    Ok(CliCommand::SourcesRemove { path })
                }
                _ => Err(anyhow!("usage: satori sources <list|add|remove> [path]")),
            }
        }
        "jobs" => {
            if args.len() < 2 {
                return Err(anyhow!("usage: satori jobs <list|status> [id]"));
            }
            match args[1].as_str() {
                "list" => Ok(CliCommand::JobsList),
                "status" => {
                    let id = args
                        .get(2)
                        .cloned()
                        .ok_or_else(|| anyhow!("usage: satori jobs status <job-id>"))?;
                    Ok(CliCommand::JobsStatus { id })
                }
                _ => Err(anyhow!("usage: satori jobs <list|status> [id]")),
            }
        }
        "ontology" => {
            if args.len() < 2 {
                return Err(anyhow!(
                    "usage: satori ontology <list|show|export> [--source <path>] [--out <file>]"
                ));
            }
            match args[1].as_str() {
                "list" => Ok(CliCommand::OntologyList),
                "show" => {
                    if args.len() < 4 || args[2] != "--source" {
                        return Err(anyhow!("usage: satori ontology show --source <path>"));
                    }
                    Ok(CliCommand::OntologyShow {
                        source: args[3].clone(),
                    })
                }
                "export" => {
                    if args.len() < 4 || args[2] != "--source" {
                        return Err(anyhow!(
                            "usage: satori ontology export --source <path> [--out <file>]"
                        ));
                    }
                    let source = args[3].clone();
                    let mut out = None;
                    let mut i = 4usize;
                    while i < args.len() {
                        match args[i].as_str() {
                            "--out" => {
                                i += 1;
                                out = Some(
                                    args.get(i)
                                        .cloned()
                                        .ok_or_else(|| anyhow!("missing value for --out"))?,
                                );
                            }
                            other => {
                                return Err(anyhow!("unsupported ontology argument: {}", other));
                            }
                        }
                        i += 1;
                    }
                    Ok(CliCommand::OntologyExport { source, out })
                }
                _ => Err(anyhow!(
                    "usage: satori ontology <list|show|export> [--source <path>] [--out <file>]"
                )),
            }
        }
        "tui" => Ok(CliCommand::Tui),
        "help" | "--help" | "-h" => Ok(CliCommand::Help),
        other => Err(anyhow!(
            "unknown command: {}. run `satori help` for usage",
            other
        )),
    }
}

fn print_help() {
    let color = crate::term::color_stdout();
    let title = if color {
        "satori command-first CLI".bold().cyan().to_string()
    } else {
        "satori command-first CLI".to_string()
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
        "  satori status",
        "  satori pack",
        "  satori graph show",
        "  satori query <text> [--mode hybrid|vector] [--top-k N] [--raw]",
        "  satori index",
        "  satori sources list",
        "  satori sources add <path>",
        "  satori sources remove <path>",
        "  satori jobs list",
        "  satori jobs status <job-id>",
        "  satori ontology list",
        "  satori ontology show --source <path>",
        "  satori ontology export --source <path> [--out <file>]",
        "  satori tui",
        "  satori --headless-serve --pack <path> [--host <host> --port <port>]",
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
    if let Some(cfg) = parse_headless_serve(&args)? {
        serve_with_startup(cfg.pack, cfg.host, cfg.port).await?;
        return Ok(());
    }

    match parse_cli_command(&args)? {
        CliCommand::Help => print_help(),
        CliCommand::Tui => {
            tui::app::run_tui().await?;
            return Ok(());
        }
        cmd => {
            let cfg = DaemonConfig::from_env();
            cli_client::ensure_daemon(&cfg).await?;
            let out = match cmd {
                CliCommand::Status => cli_client::status(&cfg).await?,
                CliCommand::Index => cli_client::index(&cfg).await?,
                CliCommand::GraphShow => {
                    cli_client::graph_show(&cfg).await?;
                    return Ok(());
                }
                CliCommand::Pack => {
                    cli_client::pack(&cfg).await?;
                    return Ok(());
                }
                CliCommand::Query { query, mode, top_k, raw } => {
                    let out = cli_client::query(&cfg, &QueryArgs { query, mode, top_k, raw }).await?;
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
                                for s in sources.iter().take(3) {
                                    let path = s
                                        .get("path")
                                        .and_then(serde_json::Value::as_str)
                                        .unwrap_or("?");
                                    let score = s
                                        .get("score")
                                        .and_then(serde_json::Value::as_f64)
                                        .unwrap_or(0.0);
                                    let pct = (score.min(1.0) * 100.0) as u32;
                                    println!("  {} ({}%)", path, pct);
                                }
                            }
                            return Ok(());
                        }
                    }
                    out
                }
                CliCommand::SourcesList => cli_client::sources_list(&cfg).await?,
                CliCommand::SourcesAdd { path } => cli_client::sources_add(&cfg, &path).await?,
                CliCommand::SourcesRemove { path } => {
                    cli_client::sources_remove(&cfg, &path).await?
                }
                CliCommand::JobsList => cli_client::jobs_list(&cfg).await?,
                CliCommand::JobsStatus { id } => cli_client::jobs_status(&cfg, &id).await?,
                CliCommand::OntologyList => cli_client::ontology_list(&cfg).await?,
                CliCommand::OntologyShow { source } => {
                    cli_client::ontology_show(&cfg, &source).await?
                }
                CliCommand::OntologyExport { source, out } => {
                    let artifact = cli_client::ontology_show(&cfg, &source).await?;
                    let output_path = out.unwrap_or_else(|| {
                        format!(
                            "./ontology-{}.json",
                            source
                                .chars()
                                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                                .collect::<String>()
                        )
                    });
                    fs::write(&output_path, serde_json::to_vec_pretty(&artifact)?)
                        .map_err(|e| anyhow!("failed to write {}: {}", output_path, e))?;
                    serde_json::json!({
                        "status":"ok",
                        "source": source,
                        "export_path": output_path,
                        "artifact": artifact
                    })
                }
                CliCommand::Help | CliCommand::Tui => unreachable!(),
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
    let manifest = load_manifest(&pack)?;
    let sources: Vec<PathBuf> = manifest
        .sources
        .iter()
        .map(|s| PathBuf::from(&s.root_path))
        .collect();
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
