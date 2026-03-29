//! Pack location: local path or an EFS-backed cloud revision. All pack I/O goes through this abstraction.

use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub enum PackLocation {
    Local(PathBuf),
    Cloud { revision_root: PathBuf },
}

impl PackLocation {
    pub fn local(path: impl Into<PathBuf>) -> Self {
        PackLocation::Local(path.into())
    }

    pub fn cloud(revision_root: impl Into<PathBuf>) -> Self {
        PackLocation::Cloud {
            revision_root: revision_root.into(),
        }
    }

    pub fn helix_path(&self) -> PathBuf {
        match self {
            PackLocation::Local(root) => crate::helix_store::helix_pack_path_for_local(root),
            PackLocation::Cloud { revision_root, .. } => revision_root.join("helix"),
        }
    }

    pub fn read_file(&self, rel_path: &str) -> Result<Vec<u8>> {
        match self {
            PackLocation::Local(root) | PackLocation::Cloud { revision_root: root, .. } => {
                let p = root.join(rel_path);
                std::fs::read(&p).with_context(|| format!("failed to read {}", p.display()))
            }
        }
    }

    pub fn write_file(&self, rel_path: &str, data: &[u8]) -> Result<()> {
        match self {
            PackLocation::Local(root) | PackLocation::Cloud { revision_root: root, .. } => {
                let p = root.join(rel_path);
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent).context("failed to create parent dir")?;
                }
                std::fs::write(&p, data).with_context(|| format!("failed to write {}", p.display()))
            }
        }
    }
}
