use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const REGISTRY_DIR: &str = ".memkit";
const REGISTRY_FILE: &str = "registry.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryPack {
    pub path: String,
    #[serde(default)]
    pub name: Option<String>,
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

fn random_pack_name() -> String {
    petname::petname(1, "-").expect("petname generation")
}

fn unique_pack_name(reg: &Registry) -> String {
    let existing: std::collections::HashSet<_> = reg
        .packs
        .iter()
        .filter_map(|p| p.name.as_deref())
        .collect();
    loop {
        let base = random_pack_name();
        if !existing.contains(base.as_str()) {
            return base;
        }
        let suffix = Uuid::new_v4().to_string().chars().take(4).collect::<String>();
        let name = format!("{}-{}", base, suffix);
        if !existing.contains(name.as_str()) {
            return name;
        }
    }
}

pub fn ensure_registered(path: &str, name: Option<String>, is_default: bool) -> Result<()> {
    let mut reg = load_registry()?;
    let normalized = PathBuf::from(path)
        .canonicalize()
        .context("path does not exist")?
        .to_string_lossy()
        .to_string();

    let name = match name {
        Some(n) => {
            if let Some(existing) = reg.packs.iter().find(|p| p.name.as_deref() == Some(n.as_str())) {
                if existing.path != normalized {
                    return Err(anyhow!("pack name \"{}\" is already used by another pack at {}", n, existing.path));
                }
            }
            Some(n)
        }
        None => Some(unique_pack_name(&reg)),
    };

    let existing_idx = reg.packs.iter().position(|p| p.path == normalized);
    if let Some(idx) = existing_idx {
        if is_default {
            reg.default_path = Some(normalized.clone());
            for p in &mut reg.packs {
                p.default = p.path == normalized;
            }
        }
        if let Some(ref n) = name {
            reg.packs[idx].name = Some(n.clone());
        }
    } else {
        reg.packs.push(RegistryPack {
            path: normalized.clone(),
            name,
            default: is_default,
            local: true,
            cloud: false,
        });
        if is_default {
            reg.default_path = Some(normalized);
        }
    }

    save_registry(&reg)?;
    Ok(())
}

/// Set the default pack by name or path. Resolves the argument then updates registry default_path and default flags.
pub fn set_default(name_or_path: &str) -> Result<()> {
    let path = resolve_pack_by_name_or_path(name_or_path)?;
    let normalized = path.to_string_lossy().to_string();
    let mut reg = load_registry()?;
    reg.default_path = Some(normalized.clone());
    for p in &mut reg.packs {
        p.default = p.path == normalized;
    }
    save_registry(&reg)?;
    Ok(())
}

/// Remove a pack from the registry by canonical path. Does not require the path to have pack artifacts.
/// Returns true if a pack was removed, false if not in registry.
pub fn remove_pack_by_path(path: &Path) -> Result<bool> {
    let normalized = path
        .canonicalize()
        .context("path not found")?
        .to_string_lossy()
        .to_string();
    remove_pack_by_path_inner(&normalized)
}

fn remove_pack_by_path_inner(normalized: &str) -> Result<bool> {
    let mut reg = load_registry()?;
    let idx = match reg.packs.iter().position(|p| p.path == normalized) {
        Some(i) => i,
        None => return Ok(false),
    };
    let was_default = reg.packs[idx].default;
    reg.packs.remove(idx);
    if was_default {
        reg.default_path = reg.packs.first().map(|p| p.path.clone());
        for p in &mut reg.packs {
            p.default = reg.default_path.as_deref() == Some(p.path.as_str());
        }
    }
    save_registry(&reg)?;
    Ok(true)
}

/// When default_path is unset but we have packs, set default to the single pack, or the pack at ~/.memkit, or the first pack.
pub fn ensure_default_if_unset() -> Result<()> {
    let mut reg = load_registry().unwrap_or_default();
    if reg.default_path.is_some() || reg.packs.is_empty() {
        return Ok(());
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow!("home directory not available"))?;
    let home_str = home.canonicalize().ok().map(|p| p.to_string_lossy().to_string());
    let default_path = if reg.packs.len() == 1 {
        Some(reg.packs[0].path.clone())
    } else if let Some(ref h) = home_str {
        reg.packs
            .iter()
            .find(|p| p.path == *h || p.path == format!("{}/.memkit", h))
            .map(|p| p.path.clone())
    } else {
        None
    };
    let path_to_set = default_path.or_else(|| reg.packs.first().map(|p| p.path.clone()));
    if let Some(ref path) = path_to_set {
        reg.default_path = Some(path.clone());
        for p in &mut reg.packs {
            p.default = p.path == *path;
            if p.path == *path {
                p.name = Some("default".to_string());
            }
        }
        save_registry(&reg)?;
    }
    Ok(())
}

/// Resolve a pack by name (registry) or by path. Returns the directory that contains the pack (parent of .memkit or pack root).
pub fn resolve_pack_by_name_or_path(arg: &str) -> Result<PathBuf> {
    if arg == "default" {
        ensure_default_if_unset()?;
        let reg = load_registry().unwrap_or_default();
        let path = reg
            .default_path
            .as_ref()
            .ok_or_else(|| anyhow!("no default pack set"))?;
        let path = PathBuf::from(path)
            .canonicalize()
            .context("default pack path no longer exists")?;
        if path.join(".memkit/manifest.json").exists() || path.join("manifest.json").exists() {
            return Ok(path);
        }
        anyhow::bail!("default pack path {} has no manifest", path.display());
    }
    let reg = load_registry().unwrap_or_default();
    if let Some(p) = reg.packs.iter().find(|p| p.name.as_deref() == Some(arg)) {
        let path = PathBuf::from(&p.path)
            .canonicalize()
            .context("registry pack path no longer exists")?;
        if path.join(".memkit/manifest.json").exists() || path.join("manifest.json").exists() {
            return Ok(path);
        }
        anyhow::bail!("pack \"{}\" path {} has no manifest", arg, path.display());
    }
    let path = PathBuf::from(arg)
        .canonicalize()
        .with_context(|| format!("pack path not found: {}", arg))?;
    if path.join(".memkit/manifest.json").exists() {
        return Ok(path);
    }
    if path.join("manifest.json").exists() {
        return Ok(path);
    }
    Err(anyhow::anyhow!("no memory pack at {}", path.display()))
}
