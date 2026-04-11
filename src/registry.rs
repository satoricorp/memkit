use std::fs;
use std::path::{Path, PathBuf};

use crate::cloud::is_memkit_uri;
use crate::pack::{has_manifest_at, init_pack};
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
}

impl RegistryPack {
    pub fn local_path(&self) -> Option<&str> {
        if !self.path.is_empty() && !is_memkit_uri(&self.path) {
            Some(self.path.as_str())
        } else {
            None
        }
    }

    pub fn registry_key(&self) -> &str {
        self.local_path().unwrap_or(self.path.as_str())
    }
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
    Ok(sanitize_registry(reg))
}

pub fn save_registry(reg: &Registry) -> Result<()> {
    let p = registry_path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).context("failed to create registry dir")?;
    }
    let sanitized = sanitize_registry(reg.clone());
    fs::write(&p, serde_json::to_vec_pretty(&sanitized)?).context("failed to write registry")?;
    Ok(())
}

fn sanitize_registry(mut reg: Registry) -> Registry {
    reg.packs.retain(|pack| pack.local_path().is_some());
    if reg
        .default_path
        .as_deref()
        .map(|path| reg.packs.iter().any(|pack| pack.local_path() == Some(path)))
        != Some(true)
    {
        reg.default_path = None;
    }
    reg
}

fn local_packs<'a>(reg: &'a Registry) -> impl Iterator<Item = &'a RegistryPack> + 'a {
    reg.packs.iter().filter(|p| p.local_path().is_some())
}

pub fn pack_dir_for_path(dir: &Path) -> PathBuf {
    dir.join(REGISTRY_DIR)
}

fn random_pack_name() -> String {
    petname::petname(1, "-").expect("petname generation")
}

fn unique_pack_name(reg: &Registry) -> String {
    let existing: std::collections::HashSet<_> =
        reg.packs.iter().filter_map(|p| p.name.as_deref()).collect();
    loop {
        let base = random_pack_name();
        if !existing.contains(base.as_str()) {
            return base;
        }
        let suffix = Uuid::new_v4()
            .to_string()
            .chars()
            .take(4)
            .collect::<String>();
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
            if let Some(existing) = reg
                .packs
                .iter()
                .find(|p| p.name.as_deref() == Some(n.as_str()))
            {
                if existing.local_path() != Some(normalized.as_str()) {
                    return Err(anyhow!(
                        "pack name \"{}\" is already used by another pack at {}",
                        n,
                        existing.registry_key()
                    ));
                }
            }
            Some(n)
        }
        None => Some(unique_pack_name(&reg)),
    };

    let existing_idx = reg
        .packs
        .iter()
        .position(|p| p.local_path() == Some(normalized.as_str()));
    if let Some(idx) = existing_idx {
        if is_default {
            reg.default_path = Some(normalized.clone());
            for p in &mut reg.packs {
                p.default = p.local_path() == Some(normalized.as_str());
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
        p.default = p.local_path() == Some(normalized.as_str());
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
    let idx = match reg
        .packs
        .iter()
        .position(|p| p.local_path() == Some(normalized))
    {
        Some(i) => i,
        None => return Ok(false),
    };
    let was_default = reg.packs[idx].default;
    reg.packs.remove(idx);
    if was_default {
        let next_default = local_packs(&reg)
            .next()
            .and_then(|p| p.local_path().map(str::to_string));
        reg.default_path = next_default;
        for p in &mut reg.packs {
            p.default = reg.default_path.as_deref() == p.local_path();
        }
    }
    save_registry(&reg)?;
    Ok(true)
}

/// When default_path is unset but we have packs, set default to the single pack, or the pack at ~/.memkit, or the first pack.
pub fn ensure_default_if_unset() -> Result<()> {
    let mut reg = load_registry().unwrap_or_default();
    let local_pack_count = local_packs(&reg).count();
    if reg.default_path.is_some() || local_pack_count == 0 {
        return Ok(());
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow!("home directory not available"))?;
    let home_str = home
        .canonicalize()
        .ok()
        .map(|p| p.to_string_lossy().to_string());
    let default_path = if local_pack_count == 1 {
        local_packs(&reg)
            .next()
            .and_then(|p| p.local_path().map(str::to_string))
    } else if let Some(ref h) = home_str {
        local_packs(&reg)
            .find(|p| {
                let Some(path) = p.local_path() else {
                    return false;
                };
                path == *h || path == format!("{}/.memkit", h)
            })
            .and_then(|p| p.local_path().map(str::to_string))
    } else {
        None
    };
    let path_to_set = default_path.or_else(|| {
        local_packs(&reg)
            .next()
            .and_then(|p| p.local_path().map(str::to_string))
    });
    if let Some(ref path) = path_to_set {
        reg.default_path = Some(path.clone());
        for p in &mut reg.packs {
            p.default = p.local_path() == Some(path.as_str());
            if p.local_path() == Some(path.as_str()) {
                p.name = Some("default".to_string());
            }
        }
        save_registry(&reg)?;
    }
    Ok(())
}

/// Target for `mk remove`: with `dir` use name/path resolution; with no `dir` use the registry default pack (same as `mk status` default), not "cwd if it looks like a pack".
pub fn resolve_remove_pack_target(dir: Option<&str>) -> Result<PathBuf> {
    if let Some(arg) = dir {
        return resolve_pack_by_name_or_path(arg);
    }
    ensure_default_if_unset()?;
    let reg = load_registry()?;
    let path = reg
        .default_path
        .as_deref()
        .or_else(|| local_packs(&reg).next().and_then(|p| p.local_path()))
        .ok_or_else(|| anyhow!("no packs registered"))?;
    PathBuf::from(path)
        .canonicalize()
        .with_context(|| format!("pack path no longer exists: {}", path))
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
        if has_manifest_at(&path) {
            return Ok(path);
        }
        anyhow::bail!("default pack path {} has no manifest", path.display());
    }
    let reg = load_registry().unwrap_or_default();
    if let Some(p) = reg.packs.iter().find(|p| p.name.as_deref() == Some(arg)) {
        if let Some(local_path) = p.local_path() {
            let path = PathBuf::from(local_path)
                .canonicalize()
                .context("registry pack path no longer exists")?;
            if has_manifest_at(&path) {
                return Ok(path);
            }
            anyhow::bail!("pack \"{}\" path {} has no manifest", arg, path.display());
        }
    }
    if is_memkit_uri(arg) {
        anyhow::bail!(
            "cloud pack uri {} is not part of the local registry. Use the cloud query path instead.",
            arg
        );
    }
    let path = PathBuf::from(arg)
        .canonicalize()
        .with_context(|| format!("pack path not found: {}", arg))?;
    if has_manifest_at(&path) {
        return Ok(path);
    }
    Err(anyhow::anyhow!("no memory pack at {}", path.display()))
}

/// When the registry has no entries, resolve cwd / `~` / create `~/.memkit` and register (same rules as CLI `ensure_pack_root(None)`).
fn ensure_default_pack_for_empty_registry() -> Result<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let home_has_pack = dirs::home_dir()
        .as_ref()
        .map(|h| has_manifest_at(h))
        .unwrap_or(false);
    if !has_manifest_at(&cwd) && !home_has_pack {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("home directory not available"))?;
        let pack_dir = pack_dir_for_path(&home);
        init_pack(&pack_dir, false, "fastembed", "BAAI/bge-small-en-v1.5", 384)
            .context("failed to init default pack")?;
        let normalized = home
            .canonicalize()
            .context("home directory path invalid")?
            .to_string_lossy()
            .to_string();
        ensure_registered(&normalized, Some("default".to_string()), true)?;
        return home.canonicalize().context("home path invalid");
    }
    if has_manifest_at(&cwd) {
        let canon = cwd.canonicalize().context("cwd path invalid")?;
        let normalized = canon.to_string_lossy().to_string();
        ensure_registered(&normalized, None, true)?;
        return Ok(canon);
    }
    if let Some(home) = dirs::home_dir() {
        if has_manifest_at(&home) {
            let canon = home.canonicalize().context("home path invalid")?;
            let normalized = canon.to_string_lossy().to_string();
            ensure_registered(&normalized, None, true)?;
            return Ok(canon);
        }
    }
    anyhow::bail!("no memory pack found. use --pack <name-or-path> or run `mk add <path>` first")
}

/// Pack paths used when starting `mk start` without `--pack` (all registered packs).
pub fn default_serve_pack_paths() -> Result<Vec<PathBuf>> {
    let _ = ensure_default_if_unset();
    let reg = load_registry().unwrap_or_default();
    if local_packs(&reg).next().is_none() {
        let root = ensure_default_pack_for_empty_registry()?;
        return Ok(vec![root]);
    }
    Ok(local_packs(&reg)
        .filter_map(|p| p.local_path().map(PathBuf::from))
        .collect())
}

pub fn default_registry_pack() -> Result<RegistryPack> {
    let _ = ensure_default_if_unset();
    let reg = load_registry().unwrap_or_default();
    if let Some(ref default_path) = reg.default_path {
        if let Some(pack) = reg
            .packs
            .iter()
            .find(|p| p.local_path() == Some(default_path.as_str()))
        {
            return Ok(pack.clone());
        }
    }
    if let Some(pack) = local_packs(&reg).next() {
        return Ok(pack.clone());
    }
    reg.packs
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("no packs registered"))
}

pub fn resolve_registry_pack(arg: Option<&str>) -> Result<RegistryPack> {
    if let Some(raw) = arg {
        if raw == "default" {
            return default_registry_pack();
        }
        let reg = load_registry().unwrap_or_default();
        if let Some(pack) = reg.packs.iter().find(|p| {
            p.name.as_deref() == Some(raw) || p.local_path() == Some(raw) || p.path == raw
        }) {
            return Ok(pack.clone());
        }
        if is_memkit_uri(raw) {
            anyhow::bail!(
                "cloud pack uri {} is not part of the local registry. Use the cloud query path instead.",
                raw
            );
        }
        let path = PathBuf::from(raw)
            .canonicalize()
            .with_context(|| format!("pack path not found: {}", raw))?;
        let normalized = path.to_string_lossy().to_string();
        if let Some(pack) = reg
            .packs
            .iter()
            .find(|p| p.local_path() == Some(normalized.as_str()))
        {
            return Ok(pack.clone());
        }
        if has_manifest_at(&path) {
            return Ok(RegistryPack {
                path: normalized,
                name: None,
                default: false,
            });
        }
        anyhow::bail!("no memory pack at {}", path.display());
    }
    default_registry_pack()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_registry_drops_legacy_cloud_entries() {
        let reg = Registry {
            packs: vec![
                RegistryPack {
                    path: "/tmp/local-pack".to_string(),
                    name: Some("local".to_string()),
                    default: true,
                },
                RegistryPack {
                    path: "memkit://users/user-1/packs/pack-1".to_string(),
                    name: Some("cloud".to_string()),
                    default: false,
                },
            ],
            default_path: Some("memkit://users/user-1/packs/pack-1".to_string()),
        };

        let sanitized = sanitize_registry(reg);
        assert_eq!(sanitized.packs.len(), 1);
        assert_eq!(sanitized.packs[0].local_path(), Some("/tmp/local-pack"));
        assert_eq!(sanitized.default_path, None);
    }
}
