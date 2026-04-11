use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const MEMKIT_URI_SCHEME: &str = "memkit://";
pub const DEFAULT_CLOUD_ROOT: &str = "/mnt/memkit-cloud";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudTenantKind {
    Users,
    Orgs,
}

impl CloudTenantKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CloudTenantKind::Users => "users",
            CloudTenantKind::Orgs => "orgs",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "users" => Ok(Self::Users),
            "orgs" => Ok(Self::Orgs),
            other => bail!("unsupported cloud tenant kind: {}", other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudPackUri {
    pub tenant_kind: CloudTenantKind,
    pub tenant_id: String,
    pub pack_id: String,
}

impl CloudPackUri {
    pub fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim().trim_end_matches('/');
        let path = trimmed
            .strip_prefix(MEMKIT_URI_SCHEME)
            .ok_or_else(|| anyhow!("cloud pack uri must start with {}", MEMKIT_URI_SCHEME))?;
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() != 4 || parts[2] != "packs" {
            bail!("cloud pack uri must look like memkit://users/<tenant_id>/packs/<pack_id>");
        }
        let tenant_kind = CloudTenantKind::parse(parts[0])?;
        let tenant_id = parts[1].trim();
        let pack_id = parts[3].trim();
        if tenant_id.is_empty() {
            bail!("cloud pack uri tenant_id cannot be empty");
        }
        if pack_id.is_empty() {
            bail!("cloud pack uri pack_id cannot be empty");
        }
        Ok(Self {
            tenant_kind,
            tenant_id: tenant_id.to_string(),
            pack_id: pack_id.to_string(),
        })
    }

    pub fn pack_root(&self, cloud_root: &Path) -> PathBuf {
        cloud_root
            .join("packs")
            .join(self.tenant_kind.as_str())
            .join(&self.tenant_id)
            .join(&self.pack_id)
    }

    pub fn pack_json_path(&self, cloud_root: &Path) -> PathBuf {
        self.pack_root(cloud_root).join("pack.json")
    }

    pub fn current_path(&self, cloud_root: &Path) -> PathBuf {
        self.pack_root(cloud_root).join("current.json")
    }

    pub fn revisions_root(&self, cloud_root: &Path) -> PathBuf {
        self.pack_root(cloud_root).join("revisions")
    }

    pub fn revision_root(&self, cloud_root: &Path, revision_id: &str) -> PathBuf {
        self.revisions_root(cloud_root).join(revision_id)
    }

    pub fn display_name(&self) -> String {
        self.pack_id.clone()
    }
}

impl std::fmt::Display for CloudPackUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}/{}/packs/{}",
            MEMKIT_URI_SCHEME,
            self.tenant_kind.as_str(),
            self.tenant_id,
            self.pack_id
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudPackMetadata {
    pub pack_uri: String,
    pub pack_id: String,
    pub tenant_type: CloudTenantKind,
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_pack_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudCurrentPointer {
    pub revision: String,
    pub sha256: String,
    pub published_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudPackSummary {
    pub pack_uri: String,
    pub pack_id: String,
    pub tenant_type: CloudTenantKind,
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_pack_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<DateTime<Utc>>,
}

pub fn is_memkit_uri(value: &str) -> bool {
    value.trim().starts_with(MEMKIT_URI_SCHEME)
}

pub fn cloud_root() -> PathBuf {
    std::env::var("MEMKIT_CLOUD_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_CLOUD_ROOT))
}

pub fn parse_cloud_pack_uri(value: &str) -> Result<CloudPackUri> {
    CloudPackUri::parse(value)
}

pub fn read_json_file<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("invalid json at {}", path.display()))
}

pub fn write_json_atomically<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file =
        fs::File::create(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to flush {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("failed to replace current pointer {}", path.display()))?;
    Ok(())
}

pub fn ensure_cloud_pack_dirs(uri: &CloudPackUri, cloud_root: &Path) -> Result<()> {
    fs::create_dir_all(uri.revisions_root(cloud_root)).with_context(|| {
        format!(
            "failed to create {}",
            uri.revisions_root(cloud_root).display()
        )
    })
}

pub fn read_pack_metadata(uri: &CloudPackUri, cloud_root: &Path) -> Result<CloudPackMetadata> {
    read_json_file(&uri.pack_json_path(cloud_root))
}

pub fn read_current_pointer(uri: &CloudPackUri, cloud_root: &Path) -> Result<CloudCurrentPointer> {
    read_json_file(&uri.current_path(cloud_root))
}

pub fn summarize_cloud_pack(uri: &CloudPackUri, cloud_root: &Path) -> Result<CloudPackSummary> {
    let metadata = read_pack_metadata(uri, cloud_root)?;
    let current = read_current_pointer(uri, cloud_root).ok();
    Ok(CloudPackSummary {
        pack_uri: metadata.pack_uri,
        pack_id: metadata.pack_id,
        tenant_type: metadata.tenant_type,
        tenant_id: metadata.tenant_id,
        display_name: metadata.display_name,
        source_pack_id: metadata.source_pack_id,
        current_revision: metadata
            .current_revision
            .or_else(|| current.as_ref().map(|c| c.revision.clone())),
        published_at: current.map(|c| c.published_at),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_memkit_uri() {
        let uri = CloudPackUri::parse("memkit://users/user-123/packs/pack-456").expect("parse");
        assert_eq!(uri.tenant_kind, CloudTenantKind::Users);
        assert_eq!(uri.tenant_id, "user-123");
        assert_eq!(uri.pack_id, "pack-456");
        assert_eq!(uri.to_string(), "memkit://users/user-123/packs/pack-456");
    }

    #[test]
    fn derives_efs_paths() {
        let root = PathBuf::from("/mnt/test-cloud");
        let uri = CloudPackUri::parse("memkit://orgs/acme/packs/memory").expect("parse");
        assert_eq!(
            uri.pack_root(&root),
            PathBuf::from("/mnt/test-cloud/packs/orgs/acme/memory")
        );
        assert_eq!(
            uri.current_path(&root),
            PathBuf::from("/mnt/test-cloud/packs/orgs/acme/memory/current.json")
        );
        assert_eq!(
            uri.revision_root(&root, "rev-1"),
            PathBuf::from("/mnt/test-cloud/packs/orgs/acme/memory/revisions/rev-1")
        );
    }
}
