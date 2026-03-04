use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use uuid::Uuid;

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
        fs::write(index_path(pack_dir), serde_json::to_string_pretty(&IndexStore::default())?)
            .context("failed to write index.json")?;
    }
    if !state_path(pack_dir).exists() || force {
        fs::write(state_path(pack_dir), b"[]").context("failed to write state/file_state.json")?;
    }

    Ok(())
}

pub fn load_manifest(pack_dir: &Path) -> Result<Manifest> {
    let bytes =
        fs::read(manifest_path(pack_dir)).context("manifest.json missing; run `satori init`")?;
    let manifest = serde_json::from_slice::<Manifest>(&bytes).context("invalid manifest.json")?;
    Ok(manifest)
}

pub fn save_manifest(pack_dir: &Path, mut manifest: Manifest) -> Result<()> {
    manifest.updated_at = Utc::now();
    fs::write(manifest_path(pack_dir), serde_json::to_vec_pretty(&manifest)?)
        .context("failed writing manifest.json")?;
    Ok(())
}

pub fn load_index(pack_dir: &Path) -> Result<IndexStore> {
    let p = index_path(pack_dir);
    if !p.exists() {
        return Ok(IndexStore::default());
    }
    let bytes = fs::read(&p).context("failed to read index.json")?;
    Ok(serde_json::from_slice::<IndexStore>(&bytes).context("invalid index.json")?)
}

pub fn save_index(pack_dir: &Path, index: &IndexStore) -> Result<()> {
    fs::write(index_path(pack_dir), serde_json::to_vec_pretty(index)?)
        .context("failed writing index.json")?;
    Ok(())
}

pub fn load_file_state(pack_dir: &Path) -> Result<Vec<FileState>> {
    let p = state_path(pack_dir);
    if !p.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(p).context("failed to read file state")?;
    Ok(serde_json::from_slice::<Vec<FileState>>(&bytes).context("invalid file state json")?)
}

pub fn save_file_state(pack_dir: &Path, states: &[FileState]) -> Result<()> {
    fs::write(state_path(pack_dir), serde_json::to_vec_pretty(states)?)
        .context("failed writing state/file_state.json")?;
    Ok(())
}
