use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::conversation::{ConversationSessionInput, build_conversation_docs};
use crate::embed::provider_from_name;
use crate::helix_store::{
    helix_append_chunks, helix_graph_chunk_count, helix_graph_source_paths,
    helix_load_entity_id_map, helix_pack_path_for_local, helix_write_entities_edges,
    helix_write_graph_stats,
};
use crate::indexer::{chunk_text, content_hash};
use crate::ontology::LlmConfig;
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

pub fn run_add(pack_dir: &Path, content: &str, source_path: &str) -> Result<usize> {
    let manifest = load_manifest(pack_dir)?;
    let mut provider = create_embed_provider(
        &manifest.embedding.provider,
        &manifest.embedding.model,
        manifest.embedding.dimension,
        "add",
    )?;

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
            memory: crate::types::MemoryMetadata::default(),
        };
        new_docs.push(doc);
    }

    if new_docs.is_empty() {
        return Ok(0);
    }

    persist_docs(
        pack_dir,
        manifest.embedding.dimension,
        manifest.graph.enabled,
        &new_docs,
    )?;
    Ok(new_docs.len())
}

pub fn run_add_conversations(
    pack_dir: &Path,
    sessions: &[ConversationSessionInput],
) -> Result<usize> {
    let manifest = load_manifest(pack_dir)?;
    let extraction_provider = std::env::var("MEMKIT_CONVERSATION_PROVIDER")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| {
            normalize_runtime_conversation_provider(&manifest.conversation.extraction_provider)
        });
    let extraction_model = conversation_extraction_model(&extraction_provider);
    let indexed = build_conversation_docs(sessions, &extraction_provider, &extraction_model)?;
    let mut provider = create_embed_provider(
        &manifest.embedding.provider,
        &manifest.embedding.model,
        manifest.embedding.dimension,
        "conversation add",
    )?;
    let embed_batch_size = std::env::var("MEMKIT_CONVERSATION_EMBED_BATCH")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(128);

    let mut total_docs = 0usize;
    for doc_set in indexed {
        let mut docs = doc_set.docs;
        if docs.is_empty() {
            continue;
        }
        for chunk in docs.chunks_mut(embed_batch_size) {
            let texts: Vec<String> = chunk.iter().map(|doc| doc.content.clone()).collect();
            let embeddings = provider.embed(&texts)?;
            for (doc, embedding) in chunk.iter_mut().zip(embeddings.into_iter()) {
                doc.embedding = embedding;
            }
        }
        persist_docs(
            pack_dir,
            manifest.embedding.dimension,
            manifest.graph.enabled,
            &docs,
        )?;
        total_docs += docs.len();
    }

    Ok(total_docs)
}

fn normalize_runtime_conversation_provider(configured: &str) -> String {
    match configured.trim().to_ascii_lowercase().as_str() {
        "" | "rules" => {
            if std::env::var("OPENAI_API_KEY")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .is_some()
            {
                "openai".to_string()
            } else {
                "llama".to_string()
            }
        }
        other => other.to_string(),
    }
}

fn create_embed_provider(
    provider: &str,
    model: &str,
    dimension: usize,
    label: &str,
) -> Result<Box<dyn crate::embed::EmbeddingProvider>> {
    provider_from_name(provider, model, dimension).or_else(|e| {
        if provider == "fastembed" {
            crate::term::warn(format!(
                "warning: fastembed {} init failed ({}), falling back to hash embeddings",
                label, e
            ));
            provider_from_name("hash", model, dimension)
        } else {
            Err(e)
        }
    })
}

fn persist_docs(
    pack_dir: &Path,
    dim: usize,
    graph_enabled_by_manifest: bool,
    new_docs: &[SourceDoc],
) -> Result<()> {
    let path = helix_pack_path_for_local(pack_dir);
    helix_append_chunks(&path, new_docs, dim).context("failed to append to helix")?;
    let graph_enabled = crate::config::resolve_graph_enabled(graph_enabled_by_manifest);

    let (existing_entities, existing_rels) = crate::helix_store::helix_graph_counts(pack_dir);
    let prev_chunks = helix_graph_chunk_count(pack_dir).unwrap_or(0);
    let total_chunks = prev_chunks + new_docs.len();
    let mut merged_paths = helix_graph_source_paths(pack_dir).unwrap_or_default();
    for d in new_docs {
        merged_paths.push(d.source_path.clone());
    }
    merged_paths.sort_unstable();
    merged_paths.dedup();

    if new_docs
        .iter()
        .all(|doc| doc.memory.doc_kind == "memory_record")
    {
        helix_write_graph_stats(
            pack_dir,
            existing_entities,
            existing_rels,
            total_chunks,
            &merged_paths,
            &[],
        )?;
        return Ok(());
    }

    if !graph_enabled {
        helix_write_graph_stats(pack_dir, 0, 0, total_chunks, &merged_paths, &[])?;
        return Ok(());
    }

    let mut ontology = OntologyEngine::new(pack_dir)?;
    let mut all_entities = HashSet::new();
    let mut all_relations = Vec::new();
    for doc in new_docs {
        let extraction = ontology.extract(&doc.content_hash, &doc.content, 12);
        for e in &extraction.entities {
            all_entities.insert(e.clone());
        }
        all_relations.extend(extraction.relations.clone());
    }
    let mut entity_map = helix_load_entity_id_map(pack_dir);
    helix_write_entities_edges(
        &path,
        pack_dir,
        &mut entity_map,
        &all_entities,
        &all_relations,
    )
    .context("failed to write entities/edges to helix")?;
    helix_write_graph_stats(
        pack_dir,
        entity_map.len(),
        existing_rels + all_relations.len(),
        total_chunks,
        &merged_paths,
        &[],
    )?;
    let _ = ontology.save();
    Ok(())
}

fn conversation_extraction_model(provider: &str) -> String {
    match provider.to_ascii_lowercase().as_str() {
        "llama" => std::env::var("MEMKIT_CONVERSATION_MODEL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| LlmConfig::from_env().model),
        "openai" => std::env::var("MEMKIT_CONVERSATION_MODEL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(crate::config::resolve_openai_synthesis_model),
        _ => "heuristic".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::run_add_conversations;
    use crate::conversation::{ConversationSessionInput, ConversationTurn};
    use crate::helix_store::helix_load_all_docs;

    fn temp_pack_root() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_millis();
        std::env::temp_dir().join(format!("memkit-conv-add-{}", nonce))
    }

    #[test]
    fn conversation_add_writes_docs_and_graph_stats() {
        let pack_root = temp_pack_root();
        let pack_dir = pack_root.join(".memkit");
        fs::create_dir_all(pack_dir.join("state")).expect("create pack state dir");
        crate::pack::init_pack(&pack_dir, true, "hash", "test", 384).expect("init pack");
        fs::write(pack_dir.join("state/file_state.json"), b"[]").expect("write state");

        let count = run_add_conversations(
            &pack_dir,
            &[ConversationSessionInput {
                session_id: Some("session-1".to_string()),
                session_time: Some("2025-03-31".to_string()),
                conversation: vec![
                    ConversationTurn {
                        role: "user".to_string(),
                        content: "In the 1950s we bought a house.".to_string(),
                    },
                    ConversationTurn {
                        role: "assistant".to_string(),
                        content: "Noted.".to_string(),
                    },
                    ConversationTurn {
                        role: "user".to_string(),
                        content: "I visited it again last Sunday.".to_string(),
                    },
                ],
            }],
        )
        .expect("conversation add should succeed");

        assert!(count > 0, "conversation add should write memory docs");
        assert!(
            pack_dir.join("graph_stats.json").exists(),
            "graph stats should be written"
        );

        let docs = helix_load_all_docs(
            &crate::helix_store::helix_pack_path_for_local(&pack_dir),
            384,
        )
        .expect("load docs should succeed");
        assert!(!docs.is_empty(), "helix should contain inserted docs");
        assert!(
            docs.iter()
                .any(|doc| doc.memory.doc_kind == "memory_record"),
            "at least one memory record should round-trip"
        );
    }
}
