//! Memkit config: ~/.config/memkit/memkit.json
//! Holds model selection and other user preferences.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const CONFIG_DIR: &str = "memkit";
const CONFIG_FILE: &str = "memkit.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemkitConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Path to ~/.config/memkit/memkit.json (or XDG_CONFIG_HOME/memkit/memkit.json).
pub fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join(CONFIG_DIR).join(CONFIG_FILE)
}

/// Create config directory and default memkit.json if missing. Call once at CLI launch.
pub fn ensure_config_exists() -> Result<()> {
    let p = config_path();
    if p.exists() {
        return Ok(());
    }
    let default_cfg = MemkitConfig::default();
    save_config(&default_cfg)
}

pub fn load_config() -> Result<MemkitConfig> {
    let p = config_path();
    if !p.exists() {
        return Ok(MemkitConfig::default());
    }
    let bytes = fs::read(&p).context("failed to read memkit config")?;
    let cfg: MemkitConfig =
        serde_json::from_slice(&bytes).context("invalid memkit.json")?;
    Ok(cfg)
}

pub fn save_config(cfg: &MemkitConfig) -> Result<()> {
    let p = config_path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).context("failed to create config dir")?;
    }
    fs::write(
        &p,
        serde_json::to_vec_pretty(cfg).context("serialize config")?,
    )
    .context("failed to write memkit.json")?;
    Ok(())
}

pub fn set_model(model_id: &str) -> Result<()> {
    if !is_supported_model(model_id) {
        anyhow::bail!(
            "unknown model '{}'. run `mk models` to see supported models.",
            model_id
        );
    }
    let mut cfg = load_config()?;
    cfg.model = Some(model_id.to_string());
    save_config(&cfg)?;
    Ok(())
}

/// Supported model ids (namespaced). Publish this list; used for validation and for `mk models` output.
pub fn supported_models() -> Vec<(&'static str, &'static str)> {
    vec![
        // embed: local GGUF (download if missing)
        ("embed:qwen2.5-2b-instruct", "Qwen 2.5 2B Instruct (local GGUF)"),
        ("embed:tinyllama-1.1b-chat", "TinyLlama 1.1B Chat (local GGUF)"),
        // openai
        ("openai:gpt-4o-mini", "OpenAI GPT-4o mini"),
        ("openai:gpt-4o", "OpenAI GPT-4o"),
        ("openai:gpt-4", "OpenAI GPT-4"),
        // anthropic
        ("anthropic:claude-3-5-haiku", "Anthropic Claude 3.5 Haiku"),
        ("anthropic:claude-3-5-sonnet", "Anthropic Claude 3.5 Sonnet"),
        ("anthropic:claude-3-opus", "Anthropic Claude 3 Opus"),
        // ollama (localhost)
        ("ollama:llama3.2", "Ollama llama3.2"),
        ("ollama:mistral", "Ollama mistral"),
        ("ollama:qwen2.5", "Ollama qwen2.5"),
    ]
}

pub fn is_supported_model(id: &str) -> bool {
    supported_models().iter().any(|(mid, _)| *mid == id)
}

