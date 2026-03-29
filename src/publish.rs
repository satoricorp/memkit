//! Build and unpack EFS-backed cloud publish artifacts.

use std::fs;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use tar::{Archive, Builder, Header};

use crate::helix_store::helix_pack_path_for_local;
use crate::pack::load_manifest;
use crate::types::Manifest;

#[derive(Debug, Clone)]
pub struct PreparedPublishArchive {
    pub archive_path: PathBuf,
    pub sha256: String,
    pub manifest: Manifest,
}

#[derive(Debug, Clone)]
pub struct UnpackedPublishArtifact {
    pub manifest_path: PathBuf,
    pub helix_path: PathBuf,
    pub manifest: Manifest,
}

pub fn build_cloud_publish_archive(
    pack_dir: &Path,
    scratch_root: &Path,
) -> Result<PreparedPublishArchive> {
    build_cloud_publish_archive_with_pack_id(pack_dir, scratch_root, None)
}

pub fn build_cloud_publish_archive_with_pack_id(
    pack_dir: &Path,
    scratch_root: &Path,
    override_pack_id: Option<&str>,
) -> Result<PreparedPublishArchive> {
    let mut manifest = load_manifest(pack_dir).context("load manifest before publish")?;
    if let Some(pack_id) = override_pack_id {
        manifest.pack_id = pack_id.to_string();
    }
    let helix_path = helix_pack_path_for_local(pack_dir);
    if !helix_path.exists() {
        bail!(
            "helix index missing for {}; index the pack before publishing",
            pack_dir.display()
        );
    }

    fs::create_dir_all(scratch_root)
        .with_context(|| format!("failed to create {}", scratch_root.display()))?;
    let archive_path = scratch_root.join(format!(
        "memkit-publish-{}-{}.tar.gz",
        manifest.pack_id,
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));

    let archive_file = fs::File::create(&archive_path)
        .with_context(|| format!("failed to create {}", archive_path.display()))?;
    let encoder = GzEncoder::new(archive_file, Compression::default());
    let mut tar = Builder::new(encoder);

    let manifest_bytes = serde_json::to_vec_pretty(&manifest).context("serialize manifest")?;
    let mut manifest_header = Header::new_gnu();
    manifest_header.set_size(manifest_bytes.len() as u64);
    manifest_header.set_mode(0o644);
    manifest_header.set_cksum();
    tar.append_data(&mut manifest_header, "manifest.json", manifest_bytes.as_slice())
        .context("failed to append manifest.json")?;

    let state_path = pack_dir.join("state").join("file_state.json");
    if state_path.exists() {
        tar.append_path_with_name(&state_path, "state/file_state.json")
            .with_context(|| format!("failed to append {}", state_path.display()))?;
    }

    tar.append_dir_all("helix", &helix_path)
        .with_context(|| format!("failed to append {}", helix_path.display()))?;

    let encoder = tar
        .into_inner()
        .context("failed to finalize publish tarball")?;
    encoder.finish().context("failed to finish publish archive")?;

    let (sha256, _) = sha256_for_path(&archive_path)?;
    Ok(PreparedPublishArchive {
        archive_path,
        sha256,
        manifest,
    })
}

pub fn unpack_cloud_publish_archive(
    archive_path: &Path,
    revision_root: &Path,
) -> Result<UnpackedPublishArtifact> {
    if revision_root.exists() {
        bail!("revision directory already exists: {}", revision_root.display());
    }
    fs::create_dir_all(revision_root)
        .with_context(|| format!("failed to create {}", revision_root.display()))?;

    let archive_file = fs::File::open(archive_path)
        .with_context(|| format!("failed to open {}", archive_path.display()))?;
    let decoder = GzDecoder::new(archive_file);
    let mut archive = Archive::new(decoder);
    archive
        .unpack(revision_root)
        .with_context(|| format!("failed to unpack {}", archive_path.display()))?;

    let manifest_path = revision_root.join("manifest.json");
    if !manifest_path.exists() {
        bail!(
            "publish artifact missing manifest.json in {}",
            revision_root.display()
        );
    }
    let helix_path = revision_root.join("helix");
    if !helix_path.exists() {
        bail!(
            "publish artifact missing helix directory in {}",
            revision_root.display()
        );
    }
    let manifest_bytes = fs::read(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let manifest = serde_json::from_slice::<Manifest>(&manifest_bytes)
        .context("invalid manifest in publish artifact")?;
    Ok(UnpackedPublishArtifact {
        manifest_path,
        helix_path,
        manifest,
    })
}

pub fn sha256_for_path(path: &Path) -> Result<(String, u64)> {
    let file = fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to stat {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buffer)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok((format!("{:x}", hasher.finalize()), metadata.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::init_pack;
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
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ))
    }

    #[test]
    fn build_and_unpack_publish_archive_roundtrips_manifest_and_helix() {
        let _guard = env_lock().lock().unwrap();
        let root = unique_temp_dir("memkit-publish-test");
        let prior_helix_root = std::env::var("MEMKIT_HELIX_ROOT").ok();
        unsafe {
            std::env::set_var("MEMKIT_HELIX_ROOT", root.join("helix-root"));
        }
        let pack_dir = root.join(".memkit");
        fs::create_dir_all(&pack_dir).expect("create pack dir");
        init_pack(&pack_dir, false, "hash", "hash", 8).expect("init pack");
        let helix_path = helix_pack_path_for_local(&pack_dir);
        fs::create_dir_all(&helix_path).expect("create helix dir");
        fs::write(helix_path.join("stub.txt"), b"ok").expect("write helix stub");

        let scratch = root.join("scratch");
        let prepared = build_cloud_publish_archive(&pack_dir, &scratch).expect("build archive");
        assert!(prepared.archive_path.exists());
        assert!(!prepared.sha256.is_empty());
        let size = fs::metadata(&prepared.archive_path)
            .expect("archive metadata")
            .len();
        assert!(size > 0);

        let revision_root = root.join("revision");
        let unpacked =
            unpack_cloud_publish_archive(&prepared.archive_path, &revision_root).expect("unpack");
        assert_eq!(unpacked.manifest.pack_id, prepared.manifest.pack_id);
        assert!(unpacked.manifest_path.exists());
        assert!(unpacked.helix_path.join("stub.txt").exists());

        let _ = fs::remove_dir_all(root);
        match prior_helix_root {
            Some(value) => unsafe {
                std::env::set_var("MEMKIT_HELIX_ROOT", value);
            },
            None => unsafe {
                std::env::remove_var("MEMKIT_HELIX_ROOT");
            },
        }
    }

    #[test]
    fn build_publish_archive_can_override_cloud_pack_id_without_mutating_local_manifest() {
        let _guard = env_lock().lock().unwrap();
        let root = unique_temp_dir("memkit-publish-override-test");
        let prior_helix_root = std::env::var("MEMKIT_HELIX_ROOT").ok();
        unsafe {
            std::env::set_var("MEMKIT_HELIX_ROOT", root.join("helix-root"));
        }
        let pack_dir = root.join(".memkit");
        fs::create_dir_all(&pack_dir).expect("create pack dir");
        init_pack(&pack_dir, false, "hash", "hash", 8).expect("init pack");
        let original_manifest = load_manifest(&pack_dir).expect("load manifest");
        let helix_path = helix_pack_path_for_local(&pack_dir);
        fs::create_dir_all(&helix_path).expect("create helix dir");
        fs::write(helix_path.join("stub.txt"), b"ok").expect("write helix stub");

        let override_pack_id = uuid::Uuid::new_v4().to_string();
        let scratch = root.join("scratch");
        let prepared = build_cloud_publish_archive_with_pack_id(
            &pack_dir,
            &scratch,
            Some(&override_pack_id),
        )
        .expect("build archive");
        assert_eq!(prepared.manifest.pack_id, override_pack_id);
        assert_eq!(
            load_manifest(&pack_dir).expect("reload manifest").pack_id,
            original_manifest.pack_id
        );

        let revision_root = root.join("revision");
        let unpacked =
            unpack_cloud_publish_archive(&prepared.archive_path, &revision_root).expect("unpack");
        assert_eq!(unpacked.manifest.pack_id, override_pack_id);

        let _ = fs::remove_dir_all(root);
        match prior_helix_root {
            Some(value) => unsafe {
                std::env::set_var("MEMKIT_HELIX_ROOT", value);
            },
            None => unsafe {
                std::env::remove_var("MEMKIT_HELIX_ROOT");
            },
        }
    }
}
