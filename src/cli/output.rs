use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::types::CommandOut;
use crate::cli_client::{self, ServerConfig};
use crate::config;

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

pub fn print_help() {
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

pub fn print_version() {
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

pub fn print_login_text(out: &serde_json::Value) {
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

pub fn print_logout_text(out: &serde_json::Value) {
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

pub fn print_whoami_text(out: &crate::auth::WhoAmIResponse) {
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

pub async fn handle_status_command(
    cfg: &ServerConfig,
    output_json: bool,
    dir: Option<&str>,
) -> Result<CommandOut> {
    if dir.is_none() {
        let data = cli_client::list(cfg, output_json, cli_client::ListOutputKind::Status).await?;
        if output_json {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
        return Ok(CommandOut::Done);
    }

    let data = cli_client::status(cfg, dir).await?;
    if output_json {
        println!("{}", serde_json::to_string_pretty(&data)?);
    } else {
        cli_client::print_status(&data);
    }
    Ok(CommandOut::Done)
}

pub async fn handle_list_command(cfg: &ServerConfig, output_json: bool) -> Result<CommandOut> {
    let pack_data = cli_client::list(cfg, output_json, cli_client::ListOutputKind::Full).await?;
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

pub async fn handle_doctor_command(cfg: &ServerConfig, output_json: bool) -> Result<CommandOut> {
    let data = cli_client::doctor(cfg).await?;
    if output_json {
        println!("{}", serde_json::to_string_pretty(&data)?);
    } else {
        print_doctor_summary(&data);
    }
    Ok(CommandOut::Done)
}
