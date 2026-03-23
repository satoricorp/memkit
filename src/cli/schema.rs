//! `mk schema` — memkit schema as YAML (and optional JSON Schema for agent inputs).

use anyhow::{Result, anyhow};
use owo_colors::OwoColorize;
use serde_json::json;

pub const SCHEMA_COMMANDS: &[&str] = &[
    "add",
    "remove",
    "status",
    "query",
    "publish",
    "use",
    "list",
    "doctor",
    "schema",
    "serve",
    "stop",
    "help",
    "version",
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
            &["mk query \"how does auth work\" --top-k 12 --no-rerank --pack mypack --raw --output json"],
            &["mk -j '{\"command\":\"query\",\"query\":\"how does auth work\",\"top_k\":8,\"use_reranker\":true,\"raw\":false,\"pack\":\"./memory-pack\"}'"],
        ),
        "publish" => example_list(
            &["mk publish --pack ./memory-pack --destination s3://bucket/prefix"],
            &["mk -j '{\"command\":\"publish\",\"pack\":\"./memory-pack\",\"destination\":\"s3://bucket/prefix\"}'"],
        ),
        "use" => example_list(
            &["mk use pack ./memory-pack", "mk use model openai:gpt-4"],
            &[
                "mk -j '{\"command\":\"use\",\"pack\":null,\"model\":null}'",
                "mk -j '{\"command\":\"use\",\"model\":\"openai:gpt-4\"}'",
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
        "serve" => example_list(
            &["mk serve --pack ./memory-pack --host 127.0.0.1 --port 4242 --foreground"],
            &["mk -j '{\"command\":\"serve\",\"pack\":\"./memory-pack\",\"host\":\"127.0.0.1\",\"port\":4242,\"foreground\":true}'"],
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

fn all_examples_map() -> serde_json::Value {
    let mut m = serde_json::Map::new();
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

fn json_schema_attach_examples(mut schema: serde_json::Value, example_instances: Vec<serde_json::Value>) -> serde_json::Value {
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
                    "dir": {"type": "string", "description": "Directory to remove pack from (optional)"},
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
                    "pack": {"type": "string", "description": "Pack name or path (optional)"}
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
                    "destination": {"type": "string", "description": "e.g. s3://bucket/prefix"}
                }
            }
        }),
        "use" => serde_json::json!({
            "command": "use",
            "input": {
                "type": "object",
                "description": "Omit both pack and model to show defaults for both. Use null for pack or model to show only that field; use a string to set. Shell: only mk use pack <name> and mk use model <id> (set).",
                "properties": {
                    "pack": {"description": "null = show default pack; string = set default pack by name or path"},
                    "model": {"description": "null = show default model; string = set (e.g. openai:gpt-5.4)"}
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
        "serve" => serde_json::json!({
            "command": "serve",
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
    let yaml = serde_yaml::to_string(value).map_err(|e| anyhow!("serialize schema as YAML: {}", e))?;
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
                    "pack": { "type": "string" }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            vec![json!({
                "query": "how does auth work",
                "top_k": 12,
                "use_reranker": false,
                "raw": true,
                "pack": "./memory-pack"
            })],
        ),
        "publish" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk publish (JSON input)",
                "type": "object",
                "properties": {
                    "pack": { "type": "string" },
                    "path": { "type": "string" },
                    "destination": { "type": "string" }
                }
            }),
            vec![json!({
                "pack": "./memory-pack",
                "destination": "s3://bucket/prefix"
            })],
        ),
        "use" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk use (JSON input)",
                "type": "object",
                "properties": {
                    "pack": { "description": "null = show default pack; string = set" },
                    "model": { "description": "null = show default model; string = set (e.g. openai:gpt-5.4)" }
                }
            }),
            vec![
                json!({ "pack": serde_json::Value::Null, "model": serde_json::Value::Null }),
                json!({ "model": "openai:gpt-4" }),
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
            vec![json!({ "format": "json-schema", "schema": "query" }), json!({})],
        ),
        "serve" => json_schema_attach_examples(
            json!({
                "$schema": schema_uri,
                "title": "mk serve (JSON input)",
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
            print_value_as_yaml(&serde_json::json!({
                "commands": SCHEMA_COMMANDS,
                "usage": "mk schema [--format json|json-schema] [command]",
                "global": global_block(),
                "examples": all_examples_map(),
            }))?;
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
                anyhow::bail!("unknown schema: {}. available: {}", c, SCHEMA_COMMANDS.join(", "));
            }
        }
        (Some(c), SchemaFormat::JsonSchema) => {
            if let Some(schema) = input_json_schema_for_command(c) {
                print_value_as_yaml(&schema)?;
            } else {
                anyhow::bail!("unknown schema: {}. available: {}", c, SCHEMA_COMMANDS.join(", "));
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
