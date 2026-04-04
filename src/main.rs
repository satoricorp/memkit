mod add_docs;
mod auth;
mod cli;
mod cli_client;
mod cloud;
mod config;
mod conversation;
mod conversation_query;
mod embed;
mod extract;
mod file_tree;
mod google;
#[cfg(feature = "helix")]
mod helix_store;
mod indexer;
mod ontology;
mod ontology_llama;
mod pack;
mod pack_location;
mod publish;
mod query;
mod query_synth;
mod registry;
mod rerank;
mod server;
mod term;
mod types;
mod validate;

use anyhow::{Context, Result, anyhow};
use std::env;
use std::path::PathBuf;
use std::str::FromStr;

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

enum CommandOut {
    Done,
    Output(serde_json::Value),
}

fn output_json_enabled(ctx: &CliContext) -> bool {
    ctx.output_format == OutputFormat::Json
}

fn command_requires_server(cmd: &CliCommand) -> bool {
    !matches!(
        cmd,
        CliCommand::Help
            | CliCommand::Version
            | CliCommand::Schema { .. }
            | CliCommand::Login
            | CliCommand::Logout
            | CliCommand::WhoAmI
            | CliCommand::Use(_)
            | CliCommand::Doctor
            | CliCommand::Serve { .. }
            | CliCommand::Stop { .. }
            | CliCommand::Publish { .. }
            | CliCommand::List
            | CliCommand::Status { dir: None }
            | CliCommand::Query { .. }
    )
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

use crate::cli::parse::{parse_cli_command, parse_headless_start};
use crate::cli::types::{CliCommand, UseField, UseSpec};
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
        remaining = &remaining[remaining
            .char_indices()
            .nth(take)
            .map(|(i, _)| i)
            .unwrap_or(remaining.len())..];
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
    if let Some(p) = reg.packs.iter().find(|p| p.local_path().is_some()) {
        if let Some(local_path) = p.local_path() {
            return Ok(PathBuf::from(local_path));
        }
    }
    if let Some(home) = dirs::home_dir() {
        if has_manifest_at(&home) {
            return Ok(home);
        }
    }
    anyhow::bail!("no memory pack found. use --pack <name-or-path> or run `mk add <path>` first")
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
    ensure_registered(
        &normalized,
        Some("default".to_string()),
        reg.packs.is_empty(),
    )?;
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
                let default_pack = reg
                    .packs
                    .iter()
                    .find(|p| p.local_path() == Some(default_path.as_str()));
                let (name, path) = default_pack
                    .map(|p| {
                        (
                            p.name.as_deref().unwrap_or(p.registry_key()),
                            p.registry_key(),
                        )
                    })
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
    match &spec.cloud_url {
        UseField::Set(url) => {
            if url == "default" {
                config::set_cloud_url(None)?;
                println!(
                    "{} {}",
                    crate::term::style_stdout("Cloud URL reset to", |s| s.green().to_string()),
                    config::DEFAULT_CLOUD_URL
                );
            } else {
                config::set_cloud_url(Some(url))?;
                println!(
                    "{} {}",
                    crate::term::style_stdout("Cloud URL set to", |s| s.green().to_string()),
                    config::resolve_cloud_url()
                );
            }
        }
        UseField::Show => {
            println!(
                "{} {}",
                crate::term::style_stdout("Cloud URL:", |s| s.bold().to_string()),
                config::resolve_cloud_url()
            );
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
    println!("{}", crate::term::section_title(c, "Models"));
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
        crate::term::dimmed_word(c, "Run 'mk use model <id>' to set a default model.")
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
    crate::term::print_help_title(c);
    println!();
    println!(
        "  {}",
        crate::term::dimmed_word(
            c,
            "Global flags: [--output json|text] [--dry-run] [--version | -V]"
        )
    );
    println!();
    println!("  {}", crate::term::section_title(c, "Agents"));
    println!(
        "  {} {} {}{}",
        crate::term::dimmed_word(c, "Agent JSON:"),
        crate::term::mk_binary(c),
        crate::term::bold_word(c, "-j"),
        crate::term::dimmed_word(
            c,
            " '<JSON>'  (same as --json / --mjson; object must include \"command\")"
        )
    );
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
        " [--pack <name-or-path>] [--pack-uri memkit://users/<tenant>/packs/<pack-id>] [--cloud-pack-id <uuid>] [--overwrite]",
    );
    print_help_section(c, "Search", false);
    print_help_cmd_line_grouped(
        c,
        "query",
        " <text> [--top-k N] [--no-rerank] [--pack <name|path|pack-id|memkit-uri>] [--cloud] [--raw]",
    );
    print_help_section(c, "Defaults", false);
    print_help_cmd_line_grouped(
        c,
        "list",
        "   (registered packs and current/supported models)",
    );
    print_help_cmd_line_grouped(c, "use pack", " <name-or-path>   (set default pack)");
    print_help_cmd_line_grouped(c, "use model", " <model-id>   (set default model)");
    print_help_cmd_line_grouped(
        c,
        "use cloud",
        " <url|default>   (set cloud deployment URL)",
    );
    print_help_section(c, "Auth", false);
    print_help_cmd_line_grouped(c, "login", "   (browser sign-in via MEMKIT_AUTH_BASE_URL)");
    print_help_cmd_line_grouped(c, "logout", "   (clear local auth + revoke remote session)");
    print_help_cmd_line_grouped(c, "whoami", "   (show current auth profile + JWT status)");
    print_help_section(c, "Server", false);
    print_help_cmd_line_grouped(
        c,
        "start",
        " [--pack <path>] [--host H] [--port P] [--foreground]",
    );
    print_help_cmd_line_grouped(c, "stop", " [--port P]");
    print_help_section(c, "Diagnostics & Schemas", false);
    print_help_cmd_line_grouped(
        c,
        "doctor",
        "   (config path + server /health reachability)",
    );
    print_help_cmd_line_grouped(c, "schema", " [--format json|json-schema] [command]");
}

fn print_version() {
    println!("memkit {}", crate::term::display_version());
}

fn profile_label(profile: &crate::config::AuthProfile) -> String {
    match (&profile.name, &profile.email) {
        (Some(name), Some(email)) if !name.is_empty() && !email.is_empty() => {
            format!("{} <{}>", name, email)
        }
        (_, Some(email)) if !email.is_empty() => email.clone(),
        (Some(name), _) if !name.is_empty() => name.clone(),
        _ => "unknown user".to_string(),
    }
}

fn print_login_text(out: &serde_json::Value) {
    let c = crate::term::color_stdout();
    if let Some(profile_value) = out.get("profile") {
        if let Ok(profile) =
            serde_json::from_value::<crate::config::AuthProfile>(profile_value.clone())
        {
            println!(
                "{} {}",
                crate::term::style_stdout("Signed in as", |s| s.green().to_string()),
                profile_label(&profile)
            );
            if let Some(exp) = out.get("jwtExpiresAt").and_then(serde_json::Value::as_str) {
                println!(
                    "{} {}",
                    crate::term::style_stdout("JWT expires at", |s| s.dimmed().to_string()),
                    crate::term::bracketed_cyan(c, exp)
                );
            }
        }
    }
}

fn print_logout_text(out: &serde_json::Value) {
    let had_session = out
        .get("logged_out")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if had_session {
        println!(
            "{}",
            crate::term::style_stdout("Signed out.", |s| s.green().to_string())
        );
    } else {
        println!(
            "{}",
            crate::term::style_stdout("Not signed in.", |s| s.dimmed().to_string())
        );
    }
}

fn print_whoami_text(out: &crate::auth::WhoAmIResponse) {
    let c = crate::term::color_stdout();
    if out.authenticated {
        if let Some(ref profile) = out.profile {
            println!(
                "{} {}",
                crate::term::style_stdout("Signed in as", |s| s.green().to_string()),
                profile_label(profile)
            );
        } else {
            println!(
                "{}",
                crate::term::style_stdout("Signed in.", |s| s.green().to_string())
            );
        }
    } else {
        println!(
            "{}",
            crate::term::style_stdout("Not signed in.", |s| s.yellow().to_string())
        );
        if let Some(ref profile) = out.profile {
            println!(
                "{} {}",
                crate::term::style_stdout("Cached profile", |s| s.dimmed().to_string()),
                profile_label(profile)
            );
        }
    }
    if let Some(ref exp) = out.jwt_expires_at {
        println!(
            "{} {}",
            crate::term::style_stdout("JWT expires at", |s| s.dimmed().to_string()),
            crate::term::bracketed_cyan(c, exp)
        );
    }
}

async fn handle_status_command(
    cfg: &ServerConfig,
    output_json: bool,
    dir: Option<&str>,
) -> Result<CommandOut> {
    if dir.is_none() {
        let data = cli_client::list(&cfg, output_json, cli_client::ListOutputKind::Status).await?;
        if output_json {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
        return Ok(CommandOut::Done);
    }

    let data = cli_client::status(&cfg, dir).await?;
    if output_json {
        println!("{}", serde_json::to_string_pretty(&data)?);
    } else {
        cli_client::print_status(&data);
    }
    Ok(CommandOut::Done)
}

async fn handle_list_command(cfg: &ServerConfig, output_json: bool) -> Result<CommandOut> {
    let pack_data = cli_client::list(&cfg, output_json, cli_client::ListOutputKind::Full).await?;
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

fn print_doctor_summary(data: &serde_json::Value) {
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

async fn handle_doctor_command(cfg: &ServerConfig, output_json: bool) -> Result<CommandOut> {
    let data = cli_client::doctor(&cfg).await?;
    if output_json {
        println!("{}", serde_json::to_string_pretty(&data)?);
    } else {
        print_doctor_summary(&data);
    }
    Ok(CommandOut::Done)
}

/// If dotenvy fails to load a complex `.env` entry (for example a large inline JSON payload),
/// salvage the file manually so later variables such as MEMKIT_AUTH_BASE_URL still load.
/// Existing process env vars always win.
fn load_env_file_fallback() {
    fn parse_key(line: &str) -> Option<(usize, &str)> {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }

        let offset = line.len() - trimmed.len();
        let trimmed = if trimmed.strip_prefix("export ").is_some() {
            offset + "export ".len()
        } else {
            offset
        };
        let candidate = &line[trimmed..];
        let key_len = candidate
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        if key_len == 0 || candidate.chars().nth(key_len) != Some('=') {
            return None;
        }
        Some((trimmed, &candidate[..key_len]))
    }

    fn extract_multiline_value(content: &str, after_prefix: usize) -> (&str, usize) {
        let rest = content[after_prefix..].trim_start();
        if rest.is_empty() {
            return (rest, 0);
        }
        let lines: Vec<&str> = rest.split('\n').collect();
        let mut value_end = rest.len();
        let mut offset = 0usize;
        for (i, line) in lines.iter().enumerate() {
            if i >= 1 {
                let trimmed = line.trim_start();
                if let Some(first) = trimmed.chars().next() {
                    if (first.is_ascii_alphabetic() || first == '_') && parse_key(trimmed).is_some()
                    {
                        value_end = offset;
                        break;
                    }
                }
            }
            offset += line.len() + 1;
        }
        let value = rest[..value_end.min(rest.len())].trim_end();
        (value, rest[..value_end.min(rest.len())].len())
    }

    fn decode_value(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
            serde_json::from_str::<String>(trimmed)
                .unwrap_or_else(|_| trimmed[1..trimmed.len() - 1].replace("\\\"", "\""))
        } else if trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2 {
            trimmed[1..trimmed.len() - 1].to_string()
        } else {
            trimmed.to_string()
        }
    }

    fn parse_env_entries(content: &str) -> Vec<(String, String)> {
        let mut entries = Vec::new();
        let mut index = 0usize;

        while index < content.len() {
            let line_end = content[index..]
                .find('\n')
                .map(|offset| index + offset)
                .unwrap_or(content.len());
            let line = &content[index..line_end];

            if let Some((key_offset, key)) = parse_key(line) {
                let after_prefix = index + key_offset + key.len() + 1;
                let (raw_value, consumed_len) = if key == "MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON" {
                    extract_multiline_value(content, after_prefix)
                } else {
                    let value_end = content[after_prefix..]
                        .find('\n')
                        .map(|offset| after_prefix + offset)
                        .unwrap_or(content.len());
                    (
                        &content[after_prefix..value_end],
                        value_end.saturating_sub(after_prefix),
                    )
                };
                entries.push((key.to_string(), decode_value(raw_value)));
                index = after_prefix + consumed_len;
                if index < content.len() && content.as_bytes()[index] == b'\n' {
                    index += 1;
                }
                continue;
            }

            index = if line_end < content.len() {
                line_end + 1
            } else {
                content.len()
            };
        }

        entries
    }

    let try_env_file = |path: &std::path::Path| -> bool {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return false,
        };
        let mut loaded_any = false;
        for (key, value) in parse_env_entries(&content) {
            if key.is_empty() || std::env::var_os(&key).is_some() {
                continue;
            }
            // SAFETY: single-threaded at startup; no other thread reads these vars yet.
            unsafe {
                std::env::set_var(&key, value);
            }
            loaded_any = true;
        }
        loaded_any
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
        let _ = try_env_file(&home.join(".memkit").join(".env"));
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
    load_env_file_fallback();
    let args: Vec<String> = env::args().skip(1).collect();
    let (args, ctx) = parse_global_flags(&args);

    if let Some(cfg) = parse_headless_start(&args)? {
        serve_with_startup(cfg.packs, cfg.host, cfg.port).await?;
        return Ok(());
    }

    let cmd = parse_cli_command(&args)?;
    let allow_auth_refresh = !matches!(
        &cmd,
        CliCommand::Help | CliCommand::Version | CliCommand::Schema { .. } | CliCommand::Login
    );
    let auth_state = auth::load_runtime_auth(allow_auth_refresh).await?;

    match cmd {
        CliCommand::Version => print_version(),
        CliCommand::Help => print_help(),
        CliCommand::Schema { command, format } => {
            crate::cli::schema::print_schema(command.as_deref(), format)?;
            return Ok(());
        }
        CliCommand::Serve {
            pack,
            host,
            port,
            foreground,
        } => {
            let run_server = env::var("MEMKIT_SERVE_FOREGROUND").is_ok() || foreground;
            if !run_server {
                let exe = std::env::current_exe().map_err(|e| anyhow!("current exe: {}", e))?;
                let child_args: Vec<String> = std::env::args().skip(1).collect();
                let mut cmd = std::process::Command::new(&exe);
                cmd.args(&child_args).env("MEMKIT_SERVE_FOREGROUND", "1");
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());
                cmd.spawn()
                    .map_err(|e| anyhow!("failed to start server process: {}", e))?;
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
                .or_else(|| {
                    env::var("API_PORT")
                        .ok()
                        .and_then(|v| v.parse::<u16>().ok())
                })
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
                        std::io::stdin()
                            .read_line(&mut line)
                            .context("read confirmation")?;
                        let confirmed = line.trim().eq_ignore_ascii_case("y")
                            || line.trim().eq_ignore_ascii_case("yes");
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

            if matches!(&cmd, CliCommand::Doctor) {
                cli_client::print_server_note_doctor(&cfg, output_json_enabled(&ctx)).await;
            }
            if command_requires_server(&cmd) {
                let readonly_no_autostart = matches!(&cmd, CliCommand::Status { .. });
                if readonly_no_autostart {
                    cli_client::require_server_running(&cfg).await?;
                } else {
                    cli_client::ensure_server(&cfg).await?;
                }
                let skip_stderr_server_note =
                    matches!(&cmd, CliCommand::Status { dir: None } | CliCommand::List)
                        && ctx.output_format != OutputFormat::Json;
                if !skip_stderr_server_note {
                    cli_client::print_server_note_running(
                        &cfg,
                        ctx.output_format == OutputFormat::Json,
                    )
                    .await;
                }
            }
            let result: Result<CommandOut> = match cmd {
                CliCommand::Remove { dir, yes: _ } => {
                    let target = resolve_remove_pack_target(dir.as_deref())?;
                    let path_str = target.display().to_string();
                    let out = cli_client::remove(&cfg, &path_str).await?;
                    if ctx.output_format != OutputFormat::Json {
                        if let Some(job_id) = out
                            .get("job")
                            .and_then(|j| j.get("id"))
                            .and_then(|v| v.as_str())
                        {
                            println!(
                                "{} ({}). Run 'mk status' to check progress.",
                                crate::term::style_stdout("Removal started", |s| s
                                    .green()
                                    .to_string()),
                                job_id
                            );
                        }
                    }
                    Ok(CommandOut::Output(out))
                }
                CliCommand::Login => {
                    let out = auth::login(ctx.output_format == OutputFormat::Json).await?;
                    if ctx.output_format == OutputFormat::Json {
                        Ok(CommandOut::Output(out))
                    } else {
                        print_login_text(&out);
                        Ok(CommandOut::Done)
                    }
                }
                CliCommand::Logout => {
                    let out = auth::logout().await?;
                    if ctx.output_format == OutputFormat::Json {
                        Ok(CommandOut::Output(out))
                    } else {
                        print_logout_text(&out);
                        Ok(CommandOut::Done)
                    }
                }
                CliCommand::WhoAmI => {
                    let out = crate::auth::whoami_response(&auth_state);
                    if ctx.output_format == OutputFormat::Json {
                        Ok(CommandOut::Output(serde_json::to_value(out)?))
                    } else {
                        print_whoami_text(&out);
                        Ok(CommandOut::Done)
                    }
                }
                CliCommand::Add {
                    local_path,
                    pack,
                    api_request,
                } => {
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
                                obj.insert(
                                    "path".to_string(),
                                    serde_json::Value::String(
                                        pack_root.to_string_lossy().to_string(),
                                    ),
                                );
                            }
                            let out = cli_client::add(&cfg, &body).await?;
                            if ctx.output_format != OutputFormat::Json {
                                cli_client::print_add_started(
                                    &out,
                                    pack_root.to_string_lossy().as_ref(),
                                );
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
                                body["pack"] =
                                    serde_json::json!(pack_root.to_string_lossy().to_string());
                            }
                            let mut out = cli_client::add(&cfg, &body).await?;
                            if ctx.output_format != OutputFormat::Json {
                                if let Some(job_id) = out
                                    .get("job")
                                    .and_then(|j| j.get("id"))
                                    .and_then(serde_json::Value::as_str)
                                {
                                    println!(
                                        "{} ({}). Waiting for indexing to finish...",
                                        crate::term::style_stdout("Adding", |s| s
                                            .green()
                                            .to_string()),
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
                    handle_status_command(&cfg, output_json_enabled(&ctx), dir.as_deref()).await
                }
                CliCommand::List => handle_list_command(&cfg, output_json_enabled(&ctx)).await,
                CliCommand::Doctor => handle_doctor_command(&cfg, output_json_enabled(&ctx)).await,
                CliCommand::Publish {
                    pack,
                    pack_uri,
                    cloud_pack_id,
                    overwrite,
                } => {
                    let out = cli_client::publish(
                        &cfg,
                        pack.as_deref(),
                        pack_uri.as_deref(),
                        cloud_pack_id.as_deref(),
                        overwrite,
                        ctx.output_format == OutputFormat::Json,
                    )
                    .await?;
                    if ctx.output_format == OutputFormat::Json {
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    }
                    Ok(CommandOut::Done)
                }
                CliCommand::Query {
                    query,
                    top_k,
                    use_reranker,
                    raw,
                    pack,
                    cloud,
                } => {
                    let out = cli_client::query(
                        &cfg,
                        &QueryArgs {
                            query,
                            top_k,
                            use_reranker,
                            raw,
                            cloud,
                        },
                        pack.as_deref(),
                    )
                    .await?;
                    let pack_cloud =
                        out.get("pack_origin").and_then(serde_json::Value::as_str) == Some("cloud");
                    let use_formatted = !raw && ctx.output_format != OutputFormat::Json;
                    if use_formatted {
                        if let Some(synth_err) = out
                            .get("synthesis_error")
                            .and_then(serde_json::Value::as_str)
                        {
                            println!(
                                "{}",
                                crate::term::style_stdout(
                                    "Retrieval succeeded; synthesis failed:",
                                    |s| s.yellow().to_string()
                                )
                            );
                            println!("  {}", synth_err);
                            if let Some(results) =
                                out.get("results").and_then(serde_json::Value::as_array)
                            {
                                if !results.is_empty() {
                                    println!();
                                    println!("Top results from your pack:");
                                    for (i, r) in results.iter().take(5).enumerate() {
                                        let path = r
                                            .get("file_path")
                                            .and_then(serde_json::Value::as_str)
                                            .unwrap_or("?");
                                        let path_shown =
                                            crate::google::cli_source_link(path, pack_cloud);
                                        let content = r
                                            .get("content")
                                            .and_then(serde_json::Value::as_str)
                                            .unwrap_or("");
                                        let preview = if content.len() > 120 {
                                            format!("{}...", &content[..120])
                                        } else {
                                            content.to_string()
                                        };
                                        println!(
                                            "  {}. {} {}",
                                            i + 1,
                                            crate::term::style_stdout(&path_shown, |s| s
                                                .dimmed()
                                                .to_string()),
                                            crate::term::style_stdout(&preview, |s| s
                                                .dimmed()
                                                .to_string())
                                        );
                                    }
                                }
                            }
                            if let Some(rr) = out
                                .get("retrieval_results")
                                .and_then(serde_json::Value::as_array)
                            {
                                if !rr.is_empty() {
                                    println!();
                                    println!("Retrieval (vector store, before rerank):");
                                    for (i, r) in rr.iter().take(10).enumerate() {
                                        let path = r
                                            .get("file_path")
                                            .and_then(serde_json::Value::as_str)
                                            .unwrap_or("?");
                                        let path_shown =
                                            crate::google::cli_source_link(path, pack_cloud);
                                        let score = r
                                            .get("score")
                                            .and_then(serde_json::Value::as_f64)
                                            .unwrap_or(0.0);
                                        let content = r
                                            .get("content")
                                            .and_then(serde_json::Value::as_str)
                                            .unwrap_or("");
                                        println!(
                                            "  {}. {} score={:.3}",
                                            i + 1,
                                            crate::term::style_stdout(&path_shown, |s| s
                                                .dimmed()
                                                .to_string()),
                                            score
                                        );
                                        let wrapped = wrap_retrieval_preview(content, 72, 200);
                                        println!(
                                            "{}",
                                            crate::term::style_stdout(&wrapped, |s| s
                                                .dimmed()
                                                .to_string())
                                        );
                                    }
                                }
                            }
                            Ok(CommandOut::Done)
                        } else if let (Some(answer), Some(sources)) = (
                            out.get("answer").and_then(serde_json::Value::as_str),
                            out.get("sources").and_then(serde_json::Value::as_array),
                        ) {
                            if let Some(model) =
                                out.get("model").and_then(serde_json::Value::as_str)
                            {
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
                                    println!("{} {}", crate::term::dimmed_word(c, "❯"), line);
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
                                    let shown = crate::google::cli_source_link(path, pack_cloud);
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
                let output =
                    if ctx.output_format == OutputFormat::Json || !crate::term::color_stdout() {
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
