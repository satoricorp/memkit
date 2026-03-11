use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use uuid::Uuid;

use crate::pack_location::PackLocation;
use crate::types::{
    ChunkingConfig, EmbeddingConfig, FileState, IndexStore, Manifest, SourceConfig,
};

pub fn manifest_path(pack_dir: &Path) -> PathBuf {
    pack_dir.join("manifest.json")
}

pub fn index_path(pack_dir: &Path) -> PathBuf {
    pack_dir.join("index.json")
}

pub fn state_path(pack_dir: &Path) -> PathBuf {
    pack_dir.join("state").join("file_state.json")
}

pub fn init_pack(
    pack_dir: &Path,
    force: bool,
    provider: &str,
    model: &str,
    dim: usize,
) -> Result<()> {
    if pack_dir.exists() && !force {
        if manifest_path(pack_dir).exists() {
            bail!(
                "pack already exists at {} (use --force to overwrite scaffold)",
                pack_dir.display()
            );
        }
    }

    fs::create_dir_all(pack_dir).context("failed to create pack directory")?;
    fs::create_dir_all(pack_dir.join("state")).context("failed to create state directory")?;
    fs::create_dir_all(pack_dir.join("logs")).context("failed to create logs directory")?;

    let now = Utc::now();
    let manifest = Manifest {
        format_version: "1.0.0".to_string(),
        pack_id: Uuid::new_v4().to_string(),
        created_at: now,
        updated_at: now,
        embedding: EmbeddingConfig {
            provider: provider.to_string(),
            model: model.to_string(),
            dimension: dim,
        },
        chunking: ChunkingConfig {
            strategy: "char_window".to_string(),
            target_chars: 1200,
            overlap_chars: 200,
        },
        sources: Vec::<SourceConfig>::new(),
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    fs::write(manifest_path(pack_dir), manifest_json).context("failed to write manifest.json")?;

    if !index_path(pack_dir).exists() || force {
        fs::write(
            index_path(pack_dir),
            serde_json::to_string_pretty(&IndexStore::default())?,
        )
        .context("failed to write index.json")?;
    }
    if !state_path(pack_dir).exists() || force {
        fs::write(state_path(pack_dir), b"[]").context("failed to write state/file_state.json")?;
    }

    Ok(())
}

pub fn load_manifest(pack_dir: &Path) -> Result<Manifest> {
    load_manifest_from_loc(&PackLocation::local(pack_dir))
}

pub fn load_manifest_from_loc(loc: &PackLocation) -> Result<Manifest> {
    let bytes = loc.read_file("manifest.json").context("manifest.json missing; run `mk index <dir>`")?;
    let manifest = serde_json::from_slice::<Manifest>(&bytes).context("invalid manifest.json")?;
    Ok(manifest)
}

pub fn save_manifest(pack_dir: &Path, mut manifest: Manifest) -> Result<()> {
    save_manifest_to_loc(&PackLocation::local(pack_dir), manifest)
}

pub fn save_manifest_to_loc(loc: &PackLocation, mut manifest: Manifest) -> Result<()> {
    manifest.updated_at = Utc::now();
    let data = serde_json::to_vec_pretty(&manifest)?;
    loc.write_file("manifest.json", &data).context("failed writing manifest.json")?;
    Ok(())
}

pub fn load_index(pack_dir: &Path) -> Result<IndexStore> {
    load_index_from_loc(&PackLocation::local(pack_dir))
}

pub fn load_index_from_loc(loc: &PackLocation) -> Result<IndexStore> {
    let bytes = match loc.read_file("index.json") {
        Ok(b) => b,
        Err(_) => return Ok(IndexStore::default()),
    };
    Ok(serde_json::from_slice::<IndexStore>(&bytes).context("invalid index.json")?)
}

pub fn save_index(pack_dir: &Path, index: &IndexStore) -> Result<()> {
    save_index_to_loc(&PackLocation::local(pack_dir), index)
}

pub fn save_index_to_loc(loc: &PackLocation, index: &IndexStore) -> Result<()> {
    let data = serde_json::to_vec_pretty(index)?;
    loc.write_file("index.json", &data).context("failed writing index.json")?;
    Ok(())
}

pub fn load_file_state(pack_dir: &Path) -> Result<Vec<FileState>> {
    load_file_state_from_loc(&PackLocation::local(pack_dir))
}

pub fn load_file_state_from_loc(loc: &PackLocation) -> Result<Vec<FileState>> {
    let bytes = match loc.read_file("state/file_state.json") {
        Ok(b) => b,
        Err(_) => return Ok(Vec::new()),
    };
    Ok(serde_json::from_slice::<Vec<FileState>>(&bytes).context("invalid file state json")?)
}

pub fn save_file_state(pack_dir: &Path, states: &[FileState]) -> Result<()> {
    save_file_state_to_loc(&PackLocation::local(pack_dir), states)
}

pub fn save_file_state_to_loc(loc: &PackLocation, states: &[FileState]) -> Result<()> {
    let data = serde_json::to_vec_pretty(states)?;
    loc.write_file("state/file_state.json", &data).context("failed writing state/file_state.json")?;
    Ok(())
}

/// Copies a file into the pack root and returns the destination path.
/// Preserves the file name; overwrites if destination exists.
pub fn copy_file_to_pack(source: &Path, pack_root: &Path) -> Result<PathBuf> {
    if !source.is_file() {
        bail!("not a file: {}", source.display());
    }
    let name = source
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid source path"))?;
    let dest = pack_root.join(name);
    fs::copy(source, &dest).context("failed to copy file")?;
    Ok(dest)
}

/// Memkit artifacts to remove when scrubbing. Covers both .memkit layout and legacy root layout.
const MEMKIT_ARTIFACTS: &[&str] = &[
    ".memkit",
    "manifest.json",
    "index.json",
    "config.json",
    "state",
    "lancedb",
    "logs",
    "ontology",
];

/// Scrubs the memory pack from a directory. Removes .memkit, lancedb, manifest.json,
/// and any other files that support memkit.
pub fn scrub_pack_from_dir(dir: &Path) -> Result<()> {
    let mut removed_any = false;
    for name in MEMKIT_ARTIFACTS {
        let p = dir.join(name);
        if p.exists() {
            if p.is_dir() {
                fs::remove_dir_all(&p).with_context(|| format!("failed to remove {}", p.display()))?;
            } else {
                fs::remove_file(&p).with_context(|| format!("failed to remove {}", p.display()))?;
            }
            removed_any = true;
        }
    }
    if !removed_any {
        bail!(
            "not a memory pack: {} (no .memkit, manifest.json, lancedb, or other memkit artifacts)",
            dir.display()
        );
    }
    Ok(())
}
