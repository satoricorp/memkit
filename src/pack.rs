use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use walkdir::WalkDir;
use chrono::Utc;
use uuid::Uuid;

use crate::pack_location::PackLocation;
use crate::types::{
    ChunkingConfig, EmbeddingConfig, FileState, Manifest, SourceConfig,
};

pub fn manifest_path(pack_dir: &Path) -> PathBuf {
    pack_dir.join("manifest.json")
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

pub fn save_manifest(pack_dir: &Path, manifest: Manifest) -> Result<()> {
    save_manifest_to_loc(&PackLocation::local(pack_dir), manifest)
}

pub fn save_manifest_to_loc(loc: &PackLocation, mut manifest: Manifest) -> Result<()> {
    manifest.updated_at = Utc::now();
    let data = serde_json::to_vec_pretty(&manifest)?;
    loc.write_file("manifest.json", &data).context("failed writing manifest.json")?;
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

/// Canonical sources directory inside the pack. Add/copy puts files here; index runs from here only.
pub const SOURCES_DIR: &str = "sources";
/// Subdir for single-file adds (one file -> sources/_files/<name>).
const SOURCES_FILES_DIR: &str = "_files";

/// True if the path is under a directory that may contain iCloud/FileProvider cloud-only files (macOS).
fn is_likely_icloud_path(path: &Path) -> bool {
    path.to_string_lossy().contains("Library/Mobile Documents")
        || path.to_string_lossy().contains("FileProvider")
        || path.to_string_lossy().contains("iCloud")
}

/// On macOS, trigger download of cloud-only files: brctl for iCloud Drive, fileproviderctl for FileProvider.
fn try_trigger_cloud_download(path: &Path) {
    let s = path.to_string_lossy();
    if s.contains("Library/Mobile Documents") {
        let _ = Command::new("brctl").arg("download").arg(path).output();
    } else if s.contains("FileProvider") {
        let _ = Command::new("fileproviderctl").arg("materialize").arg(path).output();
    }
}

/// Copy directory tree from src to dest (dest/rel for each file). Skips .memkit. Uses copy or read-then-write.
fn copy_tree_into(src_dir: &Path, dest_dir: &Path) -> Result<()> {
    for entry in WalkDir::new(src_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let rel = path.strip_prefix(src_dir).context("strip prefix")?;
        if rel.components().any(|c| c.as_os_str() == ".memkit") {
            continue;
        }
        let dest = dest_dir.join(rel);
        if let Some(p) = dest.parent() {
            fs::create_dir_all(p).context("failed to create dest parent")?;
        }
        if fs::copy(path, &dest).is_err() {
            let _ = fs::read(path).and_then(|c| fs::write(&dest, c));
        }
    }
    Ok(())
}

/// Result of copying a directory for indexing. For iCloud, index from temp and clean up after.
pub struct CopyDirOutcome {
    /// Source root to add to manifest (pack-relative e.g. "sources/name", or absolute temp path for iCloud).
    pub source_root: String,
    /// If set, remove this path from manifest and delete it after indexing (iCloud temp dir).
    pub cleanup_after_index: Option<PathBuf>,
}

/// Copies a directory tree for indexing. Creates sources/ if needed.
/// For iCloud/FileProvider: downloads to /tmp, returns temp as source_root and cleanup_after_index; index runs from temp, then caller must remove that source and delete temp (no copy into pack).
pub fn copy_dir_into_sources(source_dir: &Path, pack_dir: &Path, name: &str) -> Result<CopyDirOutcome> {
    if !source_dir.is_dir() {
        bail!("not a directory: {}", source_dir.display());
    }

    if is_likely_icloud_path(source_dir) {
        let temp = std::env::temp_dir().join(format!("memkit-icloud-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp).context("failed to create temp dir")?;
        try_trigger_cloud_download(source_dir);
        copy_tree_into(source_dir, &temp)?;
        let path_str = temp.to_string_lossy().to_string();
        return Ok(CopyDirOutcome {
            source_root: path_str,
            cleanup_after_index: Some(temp),
        });
    }

    let sources_base = pack_dir.join(SOURCES_DIR);
    fs::create_dir_all(&sources_base).context("failed to create sources dir")?;
    let dest_root = sources_base.join(name);
    if dest_root.exists() {
        fs::remove_dir_all(&dest_root).context("failed to remove existing source dir")?;
    }
    fs::create_dir_all(&dest_root).context("failed to create source subdir")?;
    copy_tree_into(source_dir, &dest_root)?;
    Ok(CopyDirOutcome {
        source_root: format!("sources/{}", name),
        cleanup_after_index: None,
    })
}

/// Adds a source root to the manifest if not already present. root_path can be pack-relative or absolute. Saves manifest.
pub fn add_source_root(pack_dir: &Path, root_path: &str) -> Result<()> {
    let mut manifest = load_manifest(pack_dir)?;
    if manifest.sources.iter().any(|s| s.root_path == root_path) {
        return Ok(());
    }
    manifest.sources.push(SourceConfig {
        root_path: root_path.to_string(),
        include: vec!["**/*".to_string()],
        exclude: vec!["**/.git/**".to_string(), "**/target/**".to_string()],
    });
    save_manifest(pack_dir, manifest)?;
    Ok(())
}

/// Removes a source root from the manifest. Saves manifest.
pub fn remove_source_root(pack_dir: &Path, root_path: &str) -> Result<()> {
    let mut manifest = load_manifest(pack_dir)?;
    manifest.sources.retain(|s| s.root_path != root_path);
    save_manifest(pack_dir, manifest)?;
    Ok(())
}

/// Resolves manifest source roots to absolute paths for indexing. Pack-relative roots are joined with pack_dir.
pub fn resolve_source_roots(pack_dir: &Path, manifest: &Manifest) -> Vec<PathBuf> {
    manifest
        .sources
        .iter()
        .map(|s| {
            let p = PathBuf::from(&s.root_path);
            if p.is_absolute() {
                p
            } else {
                pack_dir.join(&s.root_path)
            }
        })
        .filter(|p| p.exists())
        .collect()
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
