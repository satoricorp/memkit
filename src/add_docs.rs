use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::embed::provider_from_name;
use crate::indexer::{chunk_text, content_hash};
use crate::helix_store::{
    helix_append_chunks, helix_graph_chunk_count, helix_graph_source_paths,
    helix_load_entity_id_map, helix_pack_path_for_local, helix_rebuild_chunks,
    helix_write_entities_edges, helix_write_graph_stats,
};
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

    let path = helix_pack_path_for_local(pack_dir);
        if path.exists() {
            helix_append_chunks(&path, &new_docs, dim).context("failed to append to helix")?;
        } else {
            helix_rebuild_chunks(&path, &new_docs, dim).context("failed to rebuild helix store")?;
            crate::helix_store::helix_clear_entity_map(pack_dir);
        }
        let mut ontology = OntologyEngine::new(pack_dir)?;
        let mut all_entities = HashSet::new();
        let mut all_relations = Vec::new();
        for doc in &new_docs {
            let extraction = ontology.extract(&doc.content_hash, &doc.content, 12);
            for e in &extraction.entities {
                all_entities.insert(e.clone());
            }
            all_relations.extend(extraction.relations.clone());
        }
        let (_existing_entities, existing_rels) = crate::helix_store::helix_graph_counts(pack_dir);
        let mut entity_map = helix_load_entity_id_map(pack_dir);
        helix_write_entities_edges(&path, pack_dir, &mut entity_map, &all_entities, &all_relations)
            .context("failed to write entities/edges to helix")?;
        let prev_chunks = helix_graph_chunk_count(pack_dir).unwrap_or(0);
        let total_chunks = prev_chunks + new_docs.len();
        let mut merged_paths = helix_graph_source_paths(pack_dir).unwrap_or_default();
        for d in &new_docs {
            merged_paths.push(d.source_path.clone());
        }
        merged_paths.sort_unstable();
        merged_paths.dedup();
        helix_write_graph_stats(
            pack_dir,
            entity_map.len(),
            existing_rels + all_relations.len(),
            total_chunks,
            &merged_paths,
            &[],
        )?;
        let _ = ontology.save();

    Ok(new_docs.len())
}
