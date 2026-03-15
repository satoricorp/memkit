use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::embed::provider_from_name;
#[cfg(feature = "lance-falkor")]
use crate::falkor_store::{
    graph_name_for_pack, socket_from_env, upsert_chunks, ChunkGraphPayload,
};
use crate::indexer::{chunk_text, content_hash};
#[cfg(feature = "lance-falkor")]
use crate::lancedb_store::{append_docs, ensure_chunk_ids, rebuild_tables};
#[cfg(feature = "store-helix-only")]
use crate::helix_store::{helix_append_chunks, helix_pack_path_for_local, helix_rebuild_chunks};
#[cfg(feature = "lance-falkor")]
use crate::ontology::OntologyEngine;
use crate::pack::load_manifest;
use crate::types::SourceDoc;

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

    let dim = manifest.embedding.dimension;

    #[cfg(feature = "lance-falkor")]
    {
        ensure_chunk_ids(&mut new_docs);
        let db_path = pack_dir.join("lancedb");
        if db_path.exists() {
            append_docs(pack_dir, &new_docs, dim).context("failed to append to lancedb")?;
        } else {
            rebuild_tables(pack_dir, &new_docs, dim).context("failed to rebuild lancedb tables")?;
        }
        if let Some(socket_path) = socket_from_env() {
            if let Ok(graph_name) = graph_name_for_pack(pack_dir) {
                let mut ontology = OntologyEngine::new(pack_dir)?;
                let graph_chunks: Vec<ChunkGraphPayload> = new_docs
                    .iter()
                    .map(|doc| {
                        let extraction = ontology.extract(&doc.content_hash, &doc.content, 12);
                        ChunkGraphPayload {
                            chunk_id: doc.chunk_id.clone(),
                            file_path: doc.source_path.clone(),
                            chunk_index: doc.chunk_index,
                            content_hash: doc.content_hash.clone(),
                            content: doc.content.clone(),
                            entities: extraction.entities,
                            relations: extraction.relations,
                        }
                    })
                    .collect();
                if let Err(e) = upsert_chunks(&socket_path, &graph_name, &graph_chunks) {
                    crate::term::warn(format!("warning: failed writing add chunks to falkor: {e}"));
                }
            }
        }
    }

    #[cfg(feature = "store-helix-only")]
    {
        let path = helix_pack_path_for_local(pack_dir);
        if path.exists() {
            helix_append_chunks(&path, &new_docs, dim).context("failed to append to helix")?;
        } else {
            helix_rebuild_chunks(&path, &new_docs, dim).context("failed to rebuild helix store")?;
        }
    }

    Ok(new_docs.len())
}
