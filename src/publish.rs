//! Publish a local memory pack to S3. API performs the upload; supports user bucket (full URI) or memkit bucket (tenant keys).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use walkdir::WalkDir;

use crate::pack::load_manifest;

/// Where to publish: user's full S3 URI, or memkit bucket with tenant prefix.
pub enum PublishDestination {
    /// User provides full destination (e.g. s3://bucket/prefix/). No extra keys added.
    UserBucket { bucket: String, prefix: String },
    /// Memkit bucket: we add users/{user_id}/packs/{pack_id}/ or orgs/{org_id}/packs/{pack_id}/
    MemkitBucket,
}

/// Publish the pack at `pack_dir` to S3. Returns the resulting S3 URI (base).
pub async fn publish_pack_to_s3(
    pack_dir: &Path,
    destination: PublishDestination,
) -> Result<String> {
    let manifest = load_manifest(pack_dir).context("load manifest for pack_id")?;
    let (bucket, prefix) = match destination {
        PublishDestination::UserBucket { bucket, prefix } => (bucket, prefix),
        PublishDestination::MemkitBucket => {
            let bucket = std::env::var("MEMKIT_BUCKET")
                .context("MEMKIT_BUCKET required for memkit bucket publish")?;
            let base_prefix = std::env::var("MEMKIT_BUCKET_PREFIX").unwrap_or_default();
            let tenant_id = std::env::var("MEMKIT_USER_ID")
                .or_else(|_| std::env::var("MEMKIT_ORG_ID"))
                .context("MEMKIT_USER_ID or MEMKIT_ORG_ID required for memkit bucket")?;
            let tenant_type = if std::env::var("MEMKIT_ORG_ID").is_ok() {
                "orgs"
            } else {
                "users"
            };
            let prefix = if base_prefix.is_empty() {
                format!("{}/{}/packs/{}/", tenant_type, tenant_id, manifest.pack_id)
            } else {
                format!(
                    "{}/{}/{}/packs/{}/",
                    base_prefix.trim_end_matches('/'),
                    tenant_type,
                    tenant_id,
                    manifest.pack_id
                )
            };
            (bucket, prefix)
        }
    };

    let config = aws_config::load_from_env().await;
    let client = Client::new(&config);

    let pack_dir = pack_dir.to_path_buf();
    let files: Vec<(String, PathBuf)> = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        for entry in WalkDir::new(&pack_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            if let Ok(rel) = path.strip_prefix(&pack_dir) {
                out.push((rel.to_string_lossy().replace('\\', "/"), path.to_path_buf()));
            }
        }
        out
    })
    .await
    .context("walk pack dir")?;

    for (rel_path, abs_path) in files {
        let data = tokio::task::spawn_blocking({
            let p = abs_path.clone();
            move || std::fs::read(&p)
        })
        .await
        .context("spawn read")?
        .context("read file")?;
        let key = if prefix.is_empty() {
            rel_path.clone()
        } else {
            format!("{}/{}", prefix.trim_end_matches('/'), rel_path)
        };
        let body = ByteStream::from(data);
        client
            .put_object()
            .bucket(&bucket)
            .key(&key)
            .body(body)
            .send()
            .await
            .with_context(|| format!("upload {}", key))?;
    }

    let uri = if prefix.is_empty() {
        format!("s3://{}/", bucket)
    } else {
        format!("s3://{}/{}", bucket, prefix.trim_end_matches('/'))
    };
    Ok(uri)
}
