use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const REGISTRY_DIR: &str = ".memkit";
const REGISTRY_FILE: &str = "registry.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryPack {
    pub path: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default = "default_local")]
    pub local: bool,
    #[serde(default)]
    pub cloud: bool,
}

fn default_local() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Registry {
    pub packs: Vec<RegistryPack>,
    pub default_path: Option<String>,
}

pub fn registry_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(REGISTRY_DIR)
        .join(REGISTRY_FILE)
}

pub fn load_registry() -> Result<Registry> {
    let p = registry_path();
    if !p.exists() {
        return Ok(Registry::default());
    }
    let bytes = fs::read(&p).context("failed to read registry")?;
    let reg: Registry = serde_json::from_slice(&bytes).context("invalid registry.json")?;
    Ok(reg)
}

pub fn save_registry(reg: &Registry) -> Result<()> {
    let p = registry_path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).context("failed to create registry dir")?;
    }
    fs::write(&p, serde_json::to_vec_pretty(reg)?).context("failed to write registry")?;
    Ok(())
}

pub fn pack_dir_for_path(dir: &Path) -> PathBuf {
    dir.join(REGISTRY_DIR)
}

pub fn ensure_registered(path: &str, is_default: bool) -> Result<()> {
    let mut reg = load_registry()?;
    let normalized = PathBuf::from(path)
        .canonicalize()
        .context("path does not exist")?
        .to_string_lossy()
        .to_string();

    if !reg.packs.iter().any(|p| p.path == normalized) {
        reg.packs.push(RegistryPack {
            path: normalized.clone(),
            default: is_default,
            local: true,
            cloud: false,
        });
        if is_default {
            reg.default_path = Some(normalized);
        }
    } else if is_default {
        reg.default_path = Some(normalized.clone());
        for p in &mut reg.packs {
            p.default = p.path == normalized;
        }
    }

    save_registry(&reg)?;
    Ok(())
}
