//! `mk schema` — memkit JSON blobs and optional JSON Schema output for agent inputs.

use anyhow::{Result, anyhow};
use serde_json::json;

pub const SCHEMA_COMMANDS: &[&str] = &[
    "add",
    "remove",
    "status",
    "query",
    "publish",
    "use",
    "models",
    "doctor",
];

/// Output shape for `mk schema`: legacy memkit wrapper vs JSON Schema for `--json` inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaFormat {
    /// Pretty-printed memkit schema object (`command` + `input` / `output` descriptions).
    Memkit,
    /// [JSON Schema](https://json-schema.org/) for the `--json` input object per command.
    JsonSchema,
}

pub fn schema_for_command(cmd: &str) -> Option<serde_json::Value> {
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
                "description": "Omit both pack and model to show defaults for both. Use null for pack or model to show only that field; use a string to set.",
                "properties": {
                    "pack": {"description": "null = show default pack; string = set default pack by name or path"},
                    "model": {"description": "null = show default model; string = set (e.g. openai:gpt-5.2)"}
                }
            }
        }),
        "models" => serde_json::json!({
            "command": "models",
            "input": {},
            "output": {
                "current": "string | null",
                "supported": [{"id": "string", "description": "string"}]
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
        _ => return None,
    })
}

/// JSON Schema (draft 2020-12) for the `--json` body of each command (excluding outer `"command"`).
fn input_json_schema_for_command(cmd: &str) -> Option<serde_json::Value> {
    let schema_uri = "https://json-schema.org/draft/2020-12/schema";
    Some(match cmd {
        "add" => json!({
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
        "remove" => json!({
            "$schema": schema_uri,
            "title": "mk remove (JSON input)",
            "type": "object",
            "properties": {
                "dir": { "type": "string" },
                "confirm": { "type": "boolean", "default": false }
            }
        }),
        "status" => json!({
            "$schema": schema_uri,
            "title": "mk status (JSON input)",
            "type": "object",
            "properties": {
                "dir": { "type": "string" }
            }
        }),
        "query" => json!({
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
        "publish" => json!({
            "$schema": schema_uri,
            "title": "mk publish (JSON input)",
            "type": "object",
            "properties": {
                "pack": { "type": "string" },
                "path": { "type": "string" },
                "destination": { "type": "string" }
            }
        }),
        "use" => json!({
            "$schema": schema_uri,
            "title": "mk use (JSON input)",
            "type": "object",
            "properties": {
                "pack": { "description": "null = show default pack; string = set" },
                "model": { "description": "null = show default model; string = set (e.g. openai:gpt-5.2)" }
            }
        }),
        "models" => json!({
            "$schema": schema_uri,
            "title": "mk models (JSON input)",
            "type": "object",
            "properties": {}
        }),
        "doctor" => json!({
            "$schema": schema_uri,
            "title": "mk doctor (JSON input)",
            "type": "object",
            "properties": {}
        }),
        _ => return None,
    })
}

pub fn print_schema(cmd: Option<&str>, format: SchemaFormat) -> Result<()> {
    match (cmd, format) {
        (None, SchemaFormat::Memkit) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "commands": SCHEMA_COMMANDS,
                    "usage": "mk schema [--format json|json-schema] [command]"
                }))?
            );
        }
        (None, SchemaFormat::JsonSchema) => {
            anyhow::bail!(
                "specify a command for JSON Schema output, e.g. mk schema --format json-schema query. available: {}",
                SCHEMA_COMMANDS.join(", ")
            );
        }
        (Some(c), SchemaFormat::Memkit) => {
            if let Some(schema) = schema_for_command(c) {
                println!("{}", serde_json::to_string_pretty(&schema)?);
            } else {
                anyhow::bail!("unknown schema: {}. available: {}", c, SCHEMA_COMMANDS.join(", "));
            }
        }
        (Some(c), SchemaFormat::JsonSchema) => {
            if let Some(schema) = input_json_schema_for_command(c) {
                println!("{}", serde_json::to_string_pretty(&schema)?);
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
