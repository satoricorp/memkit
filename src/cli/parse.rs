use std::env;
use std::path::PathBuf;

use anyhow::{Result, anyhow};

use crate::cli::types::{CliCommand, ServeConfig, UseField, UseSpec};

const USE_COMMAND_USAGE: &str = "usage: mk use pack <name-or-path> | mk use model <model-id> | mk use cloud <url|default> — run `mk list` for current defaults";

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
        _ => Err(anyhow!(
            "use: pack, model, and cloud_url must be null or a string"
        )),
    }
}

fn cli_use_from_json(j: &serde_json::Value) -> Result<CliCommand> {
    let obj = j
        .as_object()
        .ok_or_else(|| anyhow!("--json must be a JSON object"))?;
    let has_pack = obj.contains_key("pack");
    let has_model = obj.contains_key("model");
    let has_cloud_url = obj.contains_key("cloud_url");
    if !has_pack && !has_model && !has_cloud_url {
        return Ok(CliCommand::Use(UseSpec {
            pack: UseField::Show,
            model: UseField::Show,
            cloud_url: UseField::Show,
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
        cloud_url: if has_cloud_url {
            parse_use_field_json(obj.get("cloud_url"))?
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

pub fn parse_headless_start(args: &[String]) -> Result<Option<ServeConfig>> {
    let is_headless = args
        .first()
        .map(|a| a == "--headless-start")
        .unwrap_or(false);
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
    let obj = j
        .as_object()
        .ok_or_else(|| anyhow!("--json must be a JSON object"))?;
    let get_str = |k: &str| {
        obj.get(k)
            .and_then(serde_json::Value::as_str)
            .map(String::from)
    };
    let has_docs = j
        .get("documents")
        .and_then(serde_json::Value::as_array)
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    let has_conv = j
        .get("conversation")
        .and_then(serde_json::Value::as_array)
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if has_docs || has_conv {
        for doc in j
            .get("documents")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(v) = doc.get("value").and_then(serde_json::Value::as_str) {
                crate::validate::reject_control_chars(v)?;
            }
        }
        return Ok(CliCommand::Add {
            local_path: None,
            pack: pack_override
                .or_else(|| get_str("pack"))
                .or_else(|| get_str("path")),
            api_request: Some(j.clone()),
        });
    }
    let path = get_str("path").ok_or_else(|| {
        anyhow!(
            "--json must include \"path\" (local path) or \"documents\"/\"conversation\" (API add)"
        )
    })?;
    crate::validate::validate_path(&path)?;
    Ok(CliCommand::Add {
        local_path: Some(path),
        pack: pack_override.or_else(|| get_str("pack")),
        api_request: None,
    })
}

fn cli_command_from_json(cmd: &str, j: &serde_json::Value) -> Result<CliCommand> {
    let obj = j
        .as_object()
        .ok_or_else(|| anyhow!("--json must be a JSON object"))?;
    let get_str = |k: &str| {
        obj.get(k)
            .and_then(serde_json::Value::as_str)
            .map(String::from)
    };
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
            let cloud = get_bool("cloud").unwrap_or(false);
            Ok(CliCommand::Query {
                query,
                top_k,
                use_reranker,
                raw,
                pack,
                cloud,
            })
        }
        "publish" => Ok(CliCommand::Publish {
            pack: get_str("pack").or_else(|| get_str("path")),
            pack_uri: get_str("pack_uri")
                .or_else(|| get_str("uri"))
                .or_else(|| get_str("destination")),
            cloud_pack_id: get_str("cloud_pack_id").or_else(|| get_str("new_pack_id")),
            overwrite: get_bool("overwrite").unwrap_or(false),
        }),
        "login" => Ok(CliCommand::Login),
        "logout" => Ok(CliCommand::Logout),
        "whoami" => Ok(CliCommand::WhoAmI),
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
        other => Err(anyhow!(
            "unknown command: {}. run `mk help` for usage",
            other
        )),
    }
}

fn parse_remove_cli(rest: &[String]) -> Result<CliCommand> {
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

fn parse_status_cli(rest: &[String]) -> Result<CliCommand> {
    let dir = rest.first().cloned();
    if let Some(ref d) = dir {
        crate::validate::validate_path(d)?;
    }
    Ok(CliCommand::Status { dir })
}

fn parse_query_cli(rest: &[String]) -> Result<CliCommand> {
    if rest.is_empty() {
        return Err(anyhow!(
            "usage: mk query <text> [--top-k N] [--no-rerank] [--pack <name|path|pack-id|memkit-uri>] [--cloud] [--raw] — or mk --json '{{\"command\":\"query\",\"query\":\"...\"}}'"
        ));
    }
    let query = rest[0].clone();
    crate::validate::reject_control_chars(&query)?;
    let mut top_k = 8usize;
    let mut use_reranker = true;
    let mut raw = false;
    let mut pack = None;
    let mut cloud = false;
    let mut i = 1usize;
    while i < rest.len() {
        match rest[i].as_str() {
            "--no-rerank" => {
                use_reranker = false;
                i += 1;
            }
            "--top-k" => {
                i += 1;
                let v = rest
                    .get(i)
                    .ok_or_else(|| anyhow!("missing value for --top-k"))?;
                top_k = v
                    .parse::<usize>()
                    .map_err(|_| anyhow!("invalid --top-k value: {}", v))?;
            }
            "--pack" => {
                i += 1;
                pack = rest.get(i).cloned();
            }
            "--cloud" => cloud = true,
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
        cloud,
    })
}

fn parse_publish_cli(rest: &[String]) -> CliCommand {
    let pack = flag_value(rest, "--pack");
    let pack_uri = flag_value(rest, "--pack-uri")
        .or_else(|| flag_value(rest, "--uri"))
        .or_else(|| flag_value(rest, "--destination"));
    let cloud_pack_id =
        flag_value(rest, "--cloud-pack-id").or_else(|| flag_value(rest, "--new-pack-id"));
    let overwrite = rest
        .iter()
        .any(|arg| matches!(arg.as_str(), "--overwrite" | "--force"));
    CliCommand::Publish {
        pack,
        pack_uri,
        cloud_pack_id,
        overwrite,
    }
}

fn parse_schema_cli(rest: &[String]) -> Result<CliCommand> {
    let mut format = crate::cli::schema::SchemaFormat::Memkit;
    let mut i = 0usize;
    while i + 1 < rest.len() && rest[i] == "--format" {
        format = crate::cli::schema::schema_format_from_str(&rest[i + 1])?;
        i += 2;
    }
    let command = rest.get(i).cloned();
    Ok(CliCommand::Schema { command, format })
}

fn parse_use_cli(rest: &[String]) -> Result<CliCommand> {
    if rest.is_empty() || (rest.len() == 1 && matches!(rest[0].as_str(), "--help" | "-h")) {
        return Ok(CliCommand::Help);
    }
    if rest.len() != 2 {
        return Err(anyhow!(USE_COMMAND_USAGE));
    }
    match rest[0].as_str() {
        "pack" => Ok(CliCommand::Use(UseSpec {
            pack: UseField::Set(rest[1].clone()),
            model: UseField::Absent,
            cloud_url: UseField::Absent,
        })),
        "model" => Ok(CliCommand::Use(UseSpec {
            pack: UseField::Absent,
            model: UseField::Set(rest[1].clone()),
            cloud_url: UseField::Absent,
        })),
        "cloud" => Ok(CliCommand::Use(UseSpec {
            pack: UseField::Absent,
            model: UseField::Absent,
            cloud_url: UseField::Set(rest[1].clone()),
        })),
        _ => Err(anyhow!(USE_COMMAND_USAGE)),
    }
}

fn parse_start_cli(rest: &[String]) -> Result<CliCommand> {
    let pack = flag_value(rest, "--pack");
    let host = flag_value(rest, "--host");
    let port = flag_value(rest, "--port")
        .map(|v| {
            v.parse::<u16>()
                .map_err(|_| anyhow!("invalid --port value"))
        })
        .transpose()?;
    let foreground = has_any_flag(rest, &["--foreground"]);
    Ok(CliCommand::Serve {
        pack,
        host,
        port,
        foreground,
    })
}

fn parse_stop_cli(rest: &[String]) -> Result<CliCommand> {
    let port = flag_value(rest, "--port")
        .map(|v| {
            v.parse::<u16>()
                .map_err(|_| anyhow!("invalid --port value"))
        })
        .transpose()?;
    Ok(CliCommand::Stop { port })
}

pub fn parse_cli_command(args: &[String]) -> Result<CliCommand> {
    if args.is_empty() {
        return Ok(CliCommand::Help);
    }

    if args.len() == 1 {
        match args[0].as_str() {
            "--version" | "-V" | "version" => return Ok(CliCommand::Version),
            _ => {}
        }
    }

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
        "remove" => parse_remove_cli(&args[1..]),
        "status" => parse_status_cli(&args[1..]),
        "query" => parse_query_cli(&args[1..]),
        "publish" => Ok(parse_publish_cli(&args[1..])),
        "login" => Ok(CliCommand::Login),
        "logout" => Ok(CliCommand::Logout),
        "whoami" => Ok(CliCommand::WhoAmI),
        "schema" => parse_schema_cli(&args[1..]),
        "use" => parse_use_cli(&args[1..]),
        "list" => Ok(CliCommand::List),
        "doctor" => Ok(CliCommand::Doctor),
        "start" => parse_start_cli(&args[1..]),
        "stop" => parse_stop_cli(&args[1..]),
        "help" | "--help" | "-h" => Ok(CliCommand::Help),
        other => Err(anyhow!(
            "unknown command: {}. run `mk help` for usage",
            other
        )),
    }
}
