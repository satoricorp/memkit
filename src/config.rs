//! Memkit config: ~/.config/memkit/memkit.json
//! Holds model selection and auth/session state.

use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

const CONFIG_DIR: &str = "memkit";
const CONFIG_FILE: &str = "memkit.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemkitConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<PersistedAuth>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AuthProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedAuth {
    #[serde(rename = "sessionToken")]
    pub session_token: String,
    pub jwt: String,
    #[serde(rename = "jwtExpiresAt")]
    pub jwt_expires_at: String,
    pub profile: AuthProfile,
}

impl PersistedAuth {
    pub fn jwt_expires_at_utc(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.jwt_expires_at)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }
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

pub fn load_config() -> Result<MemkitConfig> {
    let p = config_path();
    if !p.exists() {
        return Ok(MemkitConfig::default());
    }
    let bytes = fs::read(&p).context("failed to read memkit config")?;
    let cfg: MemkitConfig = serde_json::from_slice(&bytes).context("invalid memkit.json")?;
    Ok(cfg)
}

pub fn save_config(cfg: &MemkitConfig) -> Result<()> {
    let p = config_path();
    if let Some(parent) = p.parent() {
        ensure_secure_config_dir(parent)?;
    }
    write_config_atomically(
        &p,
        &serde_json::to_vec_pretty(cfg).context("serialize config")?,
    )?;
    Ok(())
}

fn ensure_secure_config_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).context("failed to create config dir")?;
    #[cfg(unix)]
    {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .context("failed to set config dir permissions")?;
    }
    Ok(())
}

fn write_config_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(CONFIG_FILE);
    let tmp = path.with_file_name(format!(
        ".{}.tmp-{}-{}",
        file_name,
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    #[cfg(unix)]
    {
        let mut opts = fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true).mode(0o600);
        let mut file = opts
            .open(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to flush {}", tmp.display()))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(&tmp, bytes).with_context(|| format!("failed to write {}", tmp.display()))?;
    }
    fs::rename(&tmp, path).context("failed to replace memkit.json")?;
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .context("failed to set memkit.json permissions")?;
    }
    Ok(())
}

pub fn set_model(model_id: &str) -> Result<()> {
    if !is_supported_model(model_id) {
        anyhow::bail!(
            "unknown model '{}'. run `mk list` to see supported models.",
            model_id
        );
    }
    let mut cfg = load_config()?;
    cfg.model = Some(model_id.to_string());
    save_config(&cfg)?;
    Ok(())
}

pub const DEFAULT_CLOUD_URL: &str = "https://api.memkit.io";

fn normalize_cloud_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let parsed = Url::parse(trimmed).context("cloud URL must be a valid absolute URL")?;
    match parsed.scheme() {
        "http" | "https" => Ok(trimmed.to_string()),
        _ => anyhow::bail!("cloud URL must use http or https"),
    }
}

pub fn set_cloud_url(url: Option<&str>) -> Result<()> {
    let mut cfg = load_config()?;
    cfg.cloud_url = match url {
        Some(value) => Some(normalize_cloud_url(value)?),
        None => None,
    };
    save_config(&cfg)
}

pub fn resolve_cloud_url() -> String {
    if let Ok(url) = std::env::var("MEMKIT_URL") {
        if let Ok(normalized) = normalize_cloud_url(&url) {
            return normalized;
        }
    }
    if let Ok(cfg) = load_config() {
        if let Some(url) = cfg.cloud_url {
            if let Ok(normalized) = normalize_cloud_url(&url) {
                return normalized;
            }
        }
    }
    DEFAULT_CLOUD_URL.to_string()
}

pub fn set_auth(auth: Option<PersistedAuth>) -> Result<()> {
    let mut cfg = load_config()?;
    cfg.auth = auth;
    save_config(&cfg)
}

/// Supported model ids (namespaced). Publish this list; used for validation and for `mk list` output.
pub fn supported_models() -> Vec<(&'static str, &'static str)> {
    vec![
        // embed: local GGUF (download if missing)
        (
            "embed:qwen2.5-2b-instruct",
            "Qwen 2.5 2B Instruct (local GGUF)",
        ),
        (
            "embed:tinyllama-1.1b-chat",
            "TinyLlama 1.1B Chat (local GGUF)",
        ),
        // openai
        ("openai:gpt-5.4", "OpenAI GPT-5.4"),
        ("openai:gpt-5.2", "OpenAI GPT-5.2"),
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

/// Default OpenAI chat model for query synthesis (raw API id for `chat/completions`).
pub const DEFAULT_OPENAI_SYNTHESIS_MODEL: &str = "gpt-5.4";

/// Strip `openai:` namespace from memkit.json model ids for the OpenAI HTTP API.
pub fn openai_api_model_id(namespaced_or_plain: &str) -> String {
    namespaced_or_plain
        .strip_prefix("openai:")
        .unwrap_or(namespaced_or_plain)
        .to_string()
}

/// Model id for query synthesis: `MEMKIT_OPENAI_MODEL`, else `openai:*` from memkit.json, else default.
pub fn resolve_openai_synthesis_model() -> String {
    if let Ok(m) = std::env::var("MEMKIT_OPENAI_MODEL") {
        let t = m.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Ok(cfg) = load_config() {
        if let Some(ref id) = cfg.model {
            if id.starts_with("openai:") {
                return openai_api_model_id(id);
            }
        }
    }
    DEFAULT_OPENAI_SYNTHESIS_MODEL.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ))
    }

    #[test]
    fn save_config_writes_secure_file_and_roundtrips_auth() {
        let _guard = env_lock().lock().unwrap();
        let temp = unique_temp_dir("memkit-config-test");
        std::fs::create_dir_all(&temp).expect("create temp dir");
        let prior = std::env::var("XDG_CONFIG_HOME").ok();
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &temp);
        }

        let cfg = MemkitConfig {
            model: Some("openai:gpt-5.4".to_string()),
            cloud_url: Some("https://example.memkit.test".to_string()),
            auth: Some(PersistedAuth {
                session_token: "session-123".to_string(),
                jwt: "jwt-123".to_string(),
                jwt_expires_at: "2030-01-01T00:00:00Z".to_string(),
                profile: AuthProfile {
                    email: Some("user@example.com".to_string()),
                    ..AuthProfile::default()
                },
            }),
        };

        save_config(&cfg).expect("save config");
        let loaded = load_config().expect("load config");
        assert_eq!(loaded.model, cfg.model);
        assert_eq!(loaded.cloud_url, cfg.cloud_url);
        assert_eq!(loaded.auth, cfg.auth);

        #[cfg(unix)]
        {
            let file_mode = std::fs::metadata(config_path())
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(file_mode, 0o600);
        }

        let _ = std::fs::remove_dir_all(temp);
        match prior {
            Some(value) => unsafe {
                std::env::set_var("XDG_CONFIG_HOME", value);
            },
            None => unsafe {
                std::env::remove_var("XDG_CONFIG_HOME");
            },
        }
    }

    #[test]
    fn resolve_cloud_url_prefers_env_then_config_then_default() {
        let _guard = env_lock().lock().unwrap();
        let temp = unique_temp_dir("memkit-cloud-url-test");
        std::fs::create_dir_all(&temp).expect("create temp dir");
        let prior_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let prior_memkit_url = std::env::var("MEMKIT_URL").ok();
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &temp);
            std::env::remove_var("MEMKIT_URL");
        }

        assert_eq!(resolve_cloud_url(), DEFAULT_CLOUD_URL);

        set_cloud_url(Some("https://custom.example.com/")).expect("set config cloud url");
        assert_eq!(resolve_cloud_url(), "https://custom.example.com");

        unsafe {
            std::env::set_var("MEMKIT_URL", "https://env.example.com/");
        }
        assert_eq!(resolve_cloud_url(), "https://env.example.com");

        match prior_memkit_url {
            Some(value) => unsafe {
                std::env::set_var("MEMKIT_URL", value);
            },
            None => unsafe {
                std::env::remove_var("MEMKIT_URL");
            },
        }
        match prior_xdg {
            Some(value) => unsafe {
                std::env::set_var("XDG_CONFIG_HOME", value);
            },
            None => unsafe {
                std::env::remove_var("XDG_CONFIG_HOME");
            },
        }
        let _ = std::fs::remove_dir_all(temp);
    }
}
