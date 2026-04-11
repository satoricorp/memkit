//! `mk schema` — memkit schema as YAML (and optional JSON Schema for agent inputs).

use anyhow::{Result, anyhow};
use owo_colors::OwoColorize;
use serde_json::json;

pub const SCHEMA_COMMANDS: &[&str] = &[
    "add", "remove", "status", "query", "publish", "login", "logout", "whoami", "use", "list",
    "doctor", "schema", "start", "stop", "help", "version",
];

/// Global flags (see `parse_global_flags` in `main.rs`); may appear anywhere in argv.
fn global_block() -> serde_json::Value {
    json!({
        "flags": ["--output json|text", "--dry-run"],
        "env": ["OUTPUT_FORMAT=json"],
        "examples": [
            "mk doctor --output json",
            "mk query \"how does indexing work\" --output json --dry-run"
        ]
    })
}

/// Shell argv lines first, then `mk -j` lines (one YAML list per command).
fn example_list(argv: &[&str], agent_json: &[&str]) -> serde_json::Value {
    let mut items = Vec::with_capacity(argv.len() + agent_json.len());
    for &line in argv {
        items.push(json!(line));
    }
    for &line in agent_json {
        items.push(json!(line));
    }
    serde_json::Value::Array(items)
}

/// Per-command examples: argv-style then `mk -j` (single source for top-level + per-command memkit).
fn examples_for_command(cmd: &str) -> serde_json::Value {
    match cmd {
        "add" => example_list(
            &[
                "mk add ./path/to/source --pack ./memory-pack",
                "mk add https://docs.google.com/document/d/abc123 --pack ./memory-pack",
            ],
            &[
                "mk -j '{\"command\":\"add\",\"path\":\"./specs\",\"pack\":\"./memory-pack\"}'",
                "mk -j '{\"command\":\"add\",\"documents\":[{\"type\":\"url\",\"value\":\"https://example.com\"}]}'",
            ],
        ),
        "remove" => example_list(
            &["mk remove ./memory-pack --yes", "mk remove --yes"],
            &["mk -j '{\"command\":\"remove\",\"dir\":\"./memory-pack\",\"confirm\":true}'"],
        ),
        "status" => example_list(
            &["mk status ./memory-pack", "mk status"],
            &[
                "mk -j '{\"command\":\"status\",\"dir\":\"./memory-pack\"}'",
                "mk -j '{\"command\":\"status\"}'",
            ],
        ),
        "query" => example_list(
            &[
                "mk query \"how does auth work\" --top-k 12 --no-rerank --pack mypack --raw --output json",
                "mk query \"what changed?\" --pack pack-id-123 --cloud",
                "mk query \"what changed?\" --pack memkit://users/123/packs/pack-abc",
            ],
            &[
                "mk -j '{\"command\":\"query\",\"query\":\"how does auth work\",\"top_k\":8,\"use_reranker\":true,\"raw\":false,\"pack\":\"./memory-pack\"}'",
                "mk -j '{\"command\":\"query\",\"query\":\"what changed?\",\"pack\":\"pack-id-123\",\"cloud\":true}'",
                "mk -j '{\"command\":\"query\",\"query\":\"what changed?\",\"pack\":\"memkit://users/123/packs/pack-abc\"}'",
            ],
        ),
        "publish" => example_list(
            &[
                "mk publish --pack ./memory-pack",
                "mk publish --pack ./memory-pack --cloud-pack-id 550e8400-e29b-41d4-a716-446655440000",
                "mk publish --pack ./memory-pack --pack-uri memkit://users/123/packs/pack-abc --overwrite",
            ],
            &[
                "mk -j '{\"command\":\"publish\",\"pack\":\"./memory-pack\"}'",
                "mk -j '{\"command\":\"publish\",\"pack\":\"./memory-pack\",\"cloud_pack_id\":\"550e8400-e29b-41d4-a716-446655440000\"}'",
                "mk -j '{\"command\":\"publish\",\"pack\":\"./memory-pack\",\"pack_uri\":\"memkit://users/123/packs/pack-abc\",\"overwrite\":true}'",
            ],
        ),
        "login" => example_list(
            &["mk login", "mk login --output json"],
            &["mk -j '{\"command\":\"login\"}'"],
        ),
        "logout" => example_list(
            &["mk logout", "mk logout --output json"],
            &["mk -j '{\"command\":\"logout\"}'"],
        ),
        "whoami" => example_list(
            &["mk whoami", "mk whoami --output json"],
            &["mk -j '{\"command\":\"whoami\"}'"],
        ),
        "use" => example_list(
            &[
                "mk use pack ./memory-pack",
                "mk use model openai:gpt-4",
                "mk use cloud https://example.com",
            ],
            &[
                "mk -j '{\"command\":\"use\",\"pack\":null,\"model\":null,\"cloud_url\":null}'",
                "mk -j '{\"command\":\"use\",\"model\":\"openai:gpt-4\"}'",
                "mk -j '{\"command\":\"use\",\"cloud_url\":\"https://example.com\"}'",
            ],
        ),
        "list" => example_list(
            &["mk list --output json"],
            &["mk -j '{\"command\":\"list\"}'"],
        ),
        "doctor" => example_list(
            &["mk doctor --output json"],
            &["mk -j '{\"command\":\"doctor\"}'"],
        ),
        "schema" => example_list(
            &["mk schema --format json-schema query", "mk schema query"],
            &["mk -j '{\"command\":\"schema\",\"format\":\"json-schema\",\"schema\":\"query\"}'"],
        ),
        "start" => example_list(
            &["mk start --pack ./memory-pack --host 127.0.0.1 --port 4242 --foreground"],
            &[
                "mk -j '{\"command\":\"start\",\"pack\":\"./memory-pack\",\"host\":\"127.0.0.1\",\"port\":4242,\"foreground\":true}'",
            ],
        ),
        "stop" => example_list(
            &["mk stop --port 4242"],
            &["mk -j '{\"command\":\"stop\",\"port\":4242}'"],
        ),
        "help" => example_list(
            &["mk help", "mk --help"],
            &["mk -j '{\"command\":\"help\"}'"],
        ),
        "version" => example_list(
            &["mk version", "mk -V"],
            &["mk -j '{\"command\":\"version\"}'"],
        ),
        _ => example_list(&[], &[]),
    }
}

/// Top-level `mk schema` document: `usage`, `global`, then one key per command with example lines.
fn memkit_schema_index() -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert(
        "usage".to_string(),
        json!("mk schema [--format json|json-schema] [command]"),
    );
    m.insert("global".to_string(), global_block());
    for c in SCHEMA_COMMANDS {
        m.insert(c.to_string(), examples_for_command(c));
    }
    serde_json::Value::Object(m)
}

fn attach_examples(cmd: &str, mut schema: serde_json::Value) -> serde_json::Value {
    if let Some(o) = schema.as_object_mut() {
        o.insert("examples".to_string(), examples_for_command(cmd));
    }
    schema
}

fn json_schema_attach_examples(
    mut schema: serde_json::Value,
    example_instances: Vec<serde_json::Value>,
) -> serde_json::Value {
    if let Some(o) = schema.as_object_mut() {
        o.insert("examples".to_string(), json!(example_instances));
    }
    schema
}

/// Output shape for `mk schema`: memkit wrapper vs JSON Schema for `--json` inputs (both printed as YAML).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaFormat {
    /// Memkit schema object (`command` + `input` / `output` descriptions), YAML on stdout.
    Memkit,
    /// [JSON Schema](https://json-schema.org/) for the `--json` input object per command.
    JsonSchema,
}

pub fn schema_for_command(cmd: &str) -> Option<serde_json::Value> {
    let base = match cmd {
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
                    "dir": {"type": "string", "description": "Pack name or path (optional). Omit to remove the registry default pack (same as the (default) line in `mk status`). To remove the pack at the current directory, pass `.` or an explicit path."},
                    "confirm": {"type": "boolean", "default": false, "description": "Same as argv --yes / -y; skip TTY prompt"}
                }
            }
        }),
        "status" => serde_json::json!({
            "command": "status",
            "input": {
                "type": "object",
                "properties": {
                    "dir": {"type": "string", "description": "Pack directory; omit to list all registered packs"}
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
                    "pack": {"type": "string", "description": "Pack name, local path, pack_id, or memkit:// cloud URI (optional)"},
                    "cloud": {"type": "boolean", "default": false, "description": "When true, prefer the cloud copy for name/pack_id selectors. memkit:// URIs are always cloud."}
                },
                "required": ["query"]
            }
        }),
        "publish" => serde_json::json!({
            "command": "publish",
            "input": {
                "type": "object",
                "properties": {
                    "pack": {"type": "string", "description": "Pack name or path"},
                    "path": {"type": "string", "description": "Alias for pack"},
                    "pack_uri": {"type": "string", "description": "Optional cloud URI like memkit://users/<tenant>/packs/<pack-id>"},
                    "uri": {"type": "string", "description": "Alias for pack_uri"},
                    "cloud_pack_id": {"type": "string", "description": "Optional UUID to publish the local pack as a new cloud pack identity without changing the local manifest"},
                    "new_pack_id": {"type": "string", "description": "Alias for cloud_pack_id"},
                    "overwrite": {"type": "boolean", "default": false}
                }
            }
        }),
        "login" => serde_json::json!({
            "command": "login",
            "input": {},
            "output": {
                "authenticated": "boolean",
                "profile": "object | null",
                "jwtExpiresAt": "string | null"
            }
        }),
        "logout" => serde_json::json!({
            "command": "logout",
            "input": {},
            "output": {
                "authenticated": "boolean",
                "logged_out": "boolean"
            }
        }),
        "whoami" => serde_json::json!({
            "command": "whoami",
            "input": {},
            "output": {
                "authenticated": "boolean",
                "profile": "object | null",
                "jwtExpiresAt": "string | null",
                "refresh_error": "string | null"
            }
        }),
        "use" => serde_json::json!({
            "command": "use",
            "input": {
                "type": "object",
                "description": "Omit pack, model, and cloud_url to show all defaults. Use null for any field to show only that field; use a string to set. Shell: mk use pack <name>, mk use model <id>, or mk use cloud <url|default>.",
                "properties": {
                    "pack": {"description": "null = show default pack; string = set default pack by name or path"},
                    "model": {"description": "null = show default model; string = set (e.g. openai:gpt-5.4)"},
                    "cloud_url": {"description": "null = show effective cloud deployment URL; string = set it; use \"default\" in shell mode to reset to the memkit deployment"}
                }
            }
        }),
        "list" => serde_json::json!({
            "command": "list",
            "input": {},
            "output": {
                "packs": "object (registry / status payload from pack listing)",
                "models": {
                    "current": "string | null",
                    "supported": [{"id": "string", "description": "string"}]
                }
            }
        }),
        "doctor" => serde_json::json!({
            "command": "doctor",
            "input": {},
            "output": {
                "config_path": "string",
                "config_exists": "boolean",
                "server_url": "string",
                "server_reachable": "boolean",
                "health": "object (when reachable)"
            }
        }),
        "schema" => serde_json::json!({
            "command": "schema",
            "input": {
                "type": "object",
                "properties": {
                    "format": {"type": "string", "description": "json|memkit or json-schema"},
                    "schema": {"type": "string", "description": "Subcommand name to introspect (e.g. query)"}
                }
            }
        }),
        "start" => serde_json::json!({
            "command": "start",
            "input": {
                "type": "object",
                "properties": {
                    "pack": {"type": "string", "description": "Pack path or name (optional; defaults to registered packs)"},
                    "host": {"type": "string", "description": "Bind address (default 127.0.0.1)"},
                    "port": {"type": "integer", "description": "Listen port (default 4242)"},
                    "foreground": {"type": "boolean", "description": "Run in foreground (default false; background without --foreground)"}
                }
            }
        }),
        "stop" => serde_json::json!({
            "command": "stop",
            "input": {
                "type": "object",
                "properties": {
                    "port": {"type": "integer", "description": "Server port (default 4242; also API_PORT env)"}
                }
            }
        }),
        "help" => serde_json::json!({
            "command": "help",
            "input": {}
        }),
        "version" => serde_json::json!({
            "command": "version",
            "input": {}
        }),
        _ => return None,
    };
    Some(attach_examples(cmd, base))
}

fn print_value_as_yaml(value: &serde_json::Value) -> Result<()> {
    let yaml =
        serde_yaml::to_string(value).map_err(|e| anyhow!("serialize schema as YAML: {}", e))?;
    let trimmed = yaml.trim_end();
    println!(
        "{}",
        crate::term::style_stdout(trimmed, |s| s.cyan().to_string())
    );
    Ok(())
}

/// JSON Schema (draft 2020-12) for the `--json` body of each command (excluding outer `"command"`).
fn input_json_schema_for_command(cmd: &str) -> Option<serde_json::Value> {
    let schema_uri = "https://json-schema.org/draft/2020-12/schema";
    Some(match cmd {
        "add" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk add (JSON input)",
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Local path to add" },
                    "pack": { "type": "string" },
                    "documents": { "type": "array" },
                    "conversation": { "type": "array" }
                }
            }),
            vec![
                json!({
                    "path": "./specs",
                    "pack": "./memory-pack"
                }),
                json!({
                    "documents": [{ "type": "url", "value": "https://example.com" }]
                }),
            ],
        ),
        "remove" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk remove (JSON input)",
                "type": "object",
                "properties": {
                    "dir": { "type": "string" },
                    "confirm": { "type": "boolean", "default": false }
                }
            }),
            vec![json!({ "dir": "./memory-pack", "confirm": true })],
        ),
        "status" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk status (JSON input)",
                "type": "object",
                "properties": {
                    "dir": { "type": "string" }
                }
            }),
            vec![json!({ "dir": "./memory-pack" }), json!({})],
        ),
        "query" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk query (JSON input)",
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "top_k": { "type": "integer", "minimum": 1, "default": 8 },
                    "use_reranker": { "type": "boolean", "default": true },
                    "raw": { "type": "boolean", "default": false },
                    "pack": { "type": "string" },
                    "cloud": { "type": "boolean", "default": false }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            vec![
                json!({
                    "query": "how does auth work",
                    "top_k": 12,
                    "use_reranker": false,
                    "raw": true,
                    "pack": "./memory-pack"
                }),
                json!({
                    "query": "what changed?",
                    "pack": "pack-id-123",
                    "cloud": true
                }),
            ],
        ),
        "publish" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk publish (JSON input)",
                "type": "object",
                "properties": {
                    "pack": { "type": "string" },
                    "path": { "type": "string" },
                    "pack_uri": { "type": "string" },
                    "uri": { "type": "string" },
                    "cloud_pack_id": { "type": "string" },
                    "new_pack_id": { "type": "string" },
                    "overwrite": { "type": "boolean" }
                }
            }),
            vec![
                json!({
                    "pack": "./memory-pack",
                    "pack_uri": "memkit://users/123/packs/pack-abc",
                    "overwrite": true
                }),
                json!({
                    "pack": "./memory-pack",
                    "cloud_pack_id": "550e8400-e29b-41d4-a716-446655440000"
                }),
            ],
        ),
        "login" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk login (JSON input)",
                "type": "object",
                "properties": {}
            }),
            vec![json!({})],
        ),
        "logout" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk logout (JSON input)",
                "type": "object",
                "properties": {}
            }),
            vec![json!({})],
        ),
        "whoami" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk whoami (JSON input)",
                "type": "object",
                "properties": {}
            }),
            vec![json!({})],
        ),
        "use" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk use (JSON input)",
                "type": "object",
                "properties": {
                    "pack": { "description": "null = show default pack; string = set" },
                    "model": { "description": "null = show default model; string = set (e.g. openai:gpt-5.4)" },
                    "cloud_url": { "description": "null = show effective cloud URL; string = set" }
                }
            }),
            vec![
                json!({ "pack": serde_json::Value::Null, "model": serde_json::Value::Null, "cloud_url": serde_json::Value::Null }),
                json!({ "model": "openai:gpt-4" }),
                json!({ "cloud_url": "https://example.com" }),
            ],
        ),
        "list" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk list (JSON input)",
                "type": "object",
                "properties": {}
            }),
            vec![json!({})],
        ),
        "doctor" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk doctor (JSON input)",
                "type": "object",
                "properties": {}
            }),
            vec![json!({})],
        ),
        "schema" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk schema (JSON input)",
                "type": "object",
                "properties": {
                    "format": { "type": "string", "description": "json|memkit or json-schema" },
                    "schema": { "type": "string", "description": "Subcommand to introspect" }
                }
            }),
            vec![
                json!({ "format": "json-schema", "schema": "query" }),
                json!({}),
            ],
        ),
        "start" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk start (JSON input)",
                "type": "object",
                "properties": {
                    "pack": { "type": "string" },
                    "host": { "type": "string" },
                    "port": { "type": "integer", "minimum": 1, "maximum": 65535 },
                    "foreground": { "type": "boolean" }
                }
            }),
            vec![json!({
                "pack": "./memory-pack",
                "host": "127.0.0.1",
                "port": 4242,
                "foreground": true
            })],
        ),
        "stop" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk stop (JSON input)",
                "type": "object",
                "properties": {
                    "port": { "type": "integer", "minimum": 1, "maximum": 65535 }
                }
            }),
            vec![json!({ "port": 4242 })],
        ),
        "help" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk help (JSON input)",
                "type": "object",
                "properties": {}
            }),
            vec![json!({})],
        ),
        "version" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk version (JSON input)",
                "type": "object",
                "properties": {}
            }),
            vec![json!({})],
        ),
        _ => return None,
    })
}

pub fn print_schema(cmd: Option<&str>, format: SchemaFormat) -> Result<()> {
    match (cmd, format) {
        (None, SchemaFormat::Memkit) => {
            print_value_as_yaml(&memkit_schema_index())?;
        }
        (None, SchemaFormat::JsonSchema) => {
            anyhow::bail!(
                "specify a command for JSON Schema output, e.g. mk schema --format json-schema query. available: {}",
                SCHEMA_COMMANDS.join(", ")
            );
        }
        (Some(c), SchemaFormat::Memkit) => {
            if let Some(schema) = schema_for_command(c) {
                print_value_as_yaml(&schema)?;
            } else {
                anyhow::bail!(
                    "unknown schema: {}. available: {}",
                    c,
                    SCHEMA_COMMANDS.join(", ")
                );
            }
        }
        (Some(c), SchemaFormat::JsonSchema) => {
            if let Some(schema) = input_json_schema_for_command(c) {
                print_value_as_yaml(&schema)?;
            } else {
                anyhow::bail!(
                    "unknown schema: {}. available: {}",
                    c,
                    SCHEMA_COMMANDS.join(", ")
                );
            }
        }
    }
    Ok(())
}

pub fn schema_format_from_str(s: &str) -> Result<SchemaFormat> {
    match s.trim() {
        "json" | "memkit" => Ok(SchemaFormat::Memkit),
        "json-schema" => Ok(SchemaFormat::JsonSchema),
        other => Err(anyhow!(
            "unknown schema format: {} (use json or json-schema)",
            other
        )),
    }
}
