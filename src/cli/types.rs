//! CLI parse tree: commands and `mk use` field semantics.
use std::path::PathBuf;

use crate::cli::schema::SchemaFormat;

pub struct ServeConfig {
    pub packs: Vec<PathBuf>,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub enum UseField {
    /// Key omitted from JSON or not applicable for this invocation.
    Absent,
    /// `mk use pack` / `"pack": null` — show current default pack.
    Show,
    /// Set pack or model.
    Set(String),
}

#[derive(Debug, Clone)]
pub struct UseSpec {
    pub pack: UseField,
    pub model: UseField,
}

pub enum CliCommand {
    Add {
        local_path: Option<String>,
        pack: Option<String>,
        api_request: Option<serde_json::Value>,
    },
    Remove {
        dir: Option<String>,
        yes: bool,
    },
    Status {
        dir: Option<String>,
    },
    /// List registered packs (and per-pack status) plus current/supported models.
    List,
    Query {
        query: String,
        top_k: usize,
        use_reranker: bool,
        raw: bool,
        pack: Option<String>,
    },
    Schema {
        command: Option<String>,
        format: SchemaFormat,
    },
    Publish {
        pack: Option<String>,
        destination: Option<String>,
    },
    Login,
    Logout,
    WhoAmI,
    Use(UseSpec),
    Doctor,
    Serve {
        pack: Option<String>,
        host: Option<String>,
        port: Option<u16>,
        foreground: bool,
    },
    Stop {
        port: Option<u16>,
    },
    /// Print version and exit.
    Version,
    Help,
}
