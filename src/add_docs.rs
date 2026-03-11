use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::embed::provider_from_name;
use crate::indexer::{chunk_text, content_hash};
use crate::lancedb_store::{append_docs, ensure_chunk_ids, rebuild_tables};
use crate::pack::{load_index, load_manifest, save_index};
use crate::types::{SourceDoc};

fn to_chunk_id(source_path: &str, content_hash: &str, chunk_index: usize) -> String {
    let mut h = Sha256::new();
    h.update(source_path.as_bytes());
    h.update(content_hash.as_bytes());
    h.update(chunk_index.to_le_bytes());
    format!("{:x}", h.finalize())[..16].to_string()
}

pub fn run_add(
    pack_dir: &Path,
    content: &str,
    source_path: &str,
) -> Result<usize> {
    let manifest = load_manifest(pack_dir)?;
    let mut index = load_index(pack_dir)?;

    let mut provider = provider_from_name(
        &manifest.embedding.provider,
        &manifest.embedding.model,
        manifest.embedding.dimension,
    )
    .or_else(|e| {
        if manifest.embedding.provider == "fastembed" {
            crate::term::warn(format!(
                "warning: fastembed add init failed ({}), falling back to hash embeddings",
                e
            ));
            provider_from_name(
                "hash",
                &manifest.embedding.model,
                manifest.embedding.dimension,
            )
        } else {
            Err(e)
        }
    })?;

    let hash = content_hash(content);
    let chunks = chunk_text(
        content,
        manifest.chunking.target_chars,
        manifest.chunking.overlap_chars,
    );
    let chunk_texts: Vec<String> = chunks.iter().map(|(_, _, c)| c.clone()).collect();
    let embeddings = provider.embed(&chunk_texts)?;

    let mut new_docs = Vec::new();
    for (idx, ((start, end, chunk), embedding)) in
        chunks.into_iter().zip(embeddings.into_iter()).enumerate()
    {
        let chunk_hash = content_hash(&format!("{source_path}:{idx}:{hash}"));
        let chunk_id = to_chunk_id(source_path, &chunk_hash, idx);
        let doc = SourceDoc {
            chunk_id,
            source_path: source_path.to_string(),
            chunk_index: idx,
            start_offset: start,
            end_offset: end,
            content: chunk,
            content_hash: hash.clone(),
            embedding,
            indexed_at: Utc::now(),
        };
        new_docs.push(doc);
    }

    if new_docs.is_empty() {
        return Ok(0);
    }

    let new_count = new_docs.len();
    let db_path = pack_dir.join("lancedb");
    let dim = manifest.embedding.dimension;

    index.docs.append(&mut new_docs);
    ensure_chunk_ids(&mut index.docs);

    save_index(pack_dir, &index).context("failed to persist index")?;

    if db_path.exists() {
        let start = index.docs.len() - new_count;
        append_docs(pack_dir, &index.docs[start..], dim)
            .context("failed to append to lancedb")?;
    } else {
        rebuild_tables(pack_dir, &index.docs, dim)
            .context("failed to rebuild lancedb tables")?;
    }

    Ok(new_docs.len())
}
