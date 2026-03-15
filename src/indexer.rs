use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::embed::provider_from_name;
#[cfg(feature = "lance-falkor")]
use crate::falkor_store::{
    ChunkGraphPayload, delete_chunks_for_paths, graph_name_from_env, socket_from_env, upsert_chunks,
};
#[cfg(feature = "lance-falkor")]
use crate::lancedb_store::{ensure_chunk_ids, load_all_docs, rebuild_tables};
#[cfg(feature = "store-helix-only")]
use crate::helix_store::{
    helix_load_all_docs, helix_pack_path_for_local, helix_rebuild_chunks, helix_write_graph_stats,
};
use crate::ontology::OntologyEngine;
use crate::pack::{
    load_file_state, load_manifest, save_file_state, save_manifest,
};
use crate::types::{FileState, SourceConfig, SourceDoc};

fn is_indexable_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());

    match ext.as_deref() {
        Some(
            "rs" | "ts" | "tsx" | "js" | "jsx" | "md" | "txt" | "json" | "toml" | "yaml" | "yml"
            | "doc" | "docx" | "xls" | "xlsx" | "xlsb" | "pdf",
        )
        | None => true,
        _ => false,
    }
}

pub(crate) fn content_hash(content: &str) -> String {
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    format!("{:x}", h.finalize())
}

pub(crate) fn chunk_text(
    content: &str,
    target_chars: usize,
    overlap_chars: usize,
) -> Vec<(usize, usize, String)> {
    if content.is_empty() {
        return Vec::new();
    }
    if content.len() <= target_chars {
        return vec![(0, content.len(), content.to_string())];
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = content.as_bytes();
    while start < bytes.len() {
        let end = (start + target_chars).min(bytes.len());
        let chunk = content[start..end].to_string();
        out.push((start, end, chunk));
        if end == bytes.len() {
            break;
        }
        start = end.saturating_sub(overlap_chars);
    }
    out
}

/// True if the directory looks like a codebase (has common code-oriented top-level dirs).
fn is_likely_codebase(root: &Path) -> bool {
    const CODE_DIRS: &[&str] = &["src", "lib", "packages", "app", "pkg"];
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if meta.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if CODE_DIRS.contains(&name) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn to_source_configs(pack_dir: &Path, sources: &[PathBuf]) -> Vec<SourceConfig> {
    sources
        .iter()
        .map(|p| {
            let root_path = p
                .strip_prefix(pack_dir)
                .ok()
                .and_then(|r| r.to_str().map(String::from))
                .unwrap_or_else(|| p.to_string_lossy().to_string());
            SourceConfig {
                root_path,
                include: vec!["**/*".to_string()],
                exclude: vec!["**/.git/**".to_string(), "**/target/**".to_string()],
            }
        })
        .collect()
}

#[allow(unused_variables)]
pub fn run_index(
    pack_dir: &Path,
    sources: &[PathBuf],
    graph_name_override: Option<&str>,
) -> Result<(usize, usize, usize)> {
    let mut manifest = load_manifest(pack_dir)?;
    manifest.sources = to_source_configs(pack_dir, sources);
    let existing_docs: Vec<_> = {
        #[cfg(feature = "store-helix-only")]
        {
            helix_load_all_docs(&helix_pack_path_for_local(pack_dir), manifest.embedding.dimension)?
        }
        #[cfg(feature = "lance-falkor")]
        {
            load_all_docs(pack_dir, manifest.embedding.dimension)?
        }
    };
    let existing_by_chunk: HashMap<String, SourceDoc> = existing_docs
        .into_iter()
        .map(|d| (d.chunk_id.clone(), d))
        .collect();
    let mut file_states = load_file_state(pack_dir)?;
    let mut state_by_path: HashMap<String, FileState> = file_states
        .drain(..)
        .map(|s| (s.file_path.clone(), s))
        .collect();
    #[allow(unused_variables)]
    let previous_paths: HashSet<String> = state_by_path.keys().cloned().collect();

    let mut scanned = 0usize;
    let mut updated_files = 0usize;
    let mut total_chunks = 0usize;
    let mut next_docs = Vec::new();
    let mut next_states = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut updated_paths = HashSet::new();
    let mut changed_docs = Vec::new();
    let mut provider = provider_from_name(
        &manifest.embedding.provider,
        &manifest.embedding.model,
        manifest.embedding.dimension,
    )
    .or_else(|e| {
        if manifest.embedding.provider == "fastembed" {
            crate::term::warn(format!(
                "warning: fastembed init failed ({}), falling back to hash embeddings",
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

    for src in sources {
        let is_codebase = is_likely_codebase(src);
        for entry in WalkDir::new(src)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            if !is_indexable_file(path) {
                continue;
            }
            let Some(content) = crate::extract::extract_text(path) else {
                continue;
            };
            scanned += 1;

            let file_path = path.to_string_lossy().to_string();
            seen_paths.insert(file_path.clone());

            let relative_path = path
                .strip_prefix(src)
                .unwrap_or(path.as_ref())
                .to_string_lossy()
                .replace('\\', "/");
            let content_to_chunk = if is_codebase && !relative_path.is_empty() {
                format!("[{}]\n\n{}", relative_path, content)
            } else {
                content.clone()
            };
            let hash = content_hash(&content_to_chunk);

            let meta = fs::metadata(path).ok();
            let mtime_unix_ms = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);

            let unchanged = state_by_path
                .get(&file_path)
                .map(|s| s.content_hash == hash)
                .unwrap_or(false);

            if unchanged {
                for doc in existing_by_chunk
                    .values()
                    .filter(|d| d.source_path == file_path)
                {
                    next_docs.push(doc.clone());
                    total_chunks += 1;
                }
                if let Some(s) = state_by_path.remove(&file_path) {
                    next_states.push(s);
                }
                continue;
            }

            updated_files += 1;
            updated_paths.insert(file_path.clone());
            let chunks = chunk_text(
                &content_to_chunk,
                manifest.chunking.target_chars,
                manifest.chunking.overlap_chars,
            );
            let chunk_texts: Vec<String> = chunks.iter().map(|(_, _, c)| c.clone()).collect();
            let embeddings = provider.embed(&chunk_texts)?;

            for (idx, ((start, end, chunk), embedding)) in
                chunks.into_iter().zip(embeddings.into_iter()).enumerate()
            {
                let chunk_hash = content_hash(&format!("{file_path}:{idx}:{hash}"));
                let chunk_id = chunk_hash[..16].to_string();
                let doc = SourceDoc {
                    chunk_id,
                    source_path: file_path.clone(),
                    chunk_index: idx,
                    start_offset: start,
                    end_offset: end,
                    content: chunk,
                    content_hash: hash.clone(),
                    embedding,
                    indexed_at: Utc::now(),
                };
                next_docs.push(doc.clone());
                changed_docs.push(doc);
                total_chunks += 1;
            }

            next_states.push(FileState {
                file_path,
                content_hash: hash,
                mtime_unix_ms,
                size,
                last_chunk_count: chunk_texts.len(),
                last_indexed_at: Utc::now(),
            });
        }
    }

    // Drop states for deleted files by only carrying seen paths.
    next_states.retain(|s| seen_paths.contains(&s.file_path));

    // Preserve chunks that did not come from the file scan (e.g. memkit://add/... from /add).
    for doc in existing_by_chunk.values() {
        if !seen_paths.contains(&doc.source_path) {
            next_docs.push(doc.clone());
        }
    }

    #[cfg(feature = "lance-falkor")]
    {
        ensure_chunk_ids(&mut next_docs);
        ensure_chunk_ids(&mut changed_docs);
        rebuild_tables(pack_dir, &next_docs, manifest.embedding.dimension)
            .context("failed to rebuild lancedb tables/indexes")?;
    }
    #[cfg(feature = "store-helix-only")]
    {
        helix_rebuild_chunks(
            &helix_pack_path_for_local(pack_dir),
            &next_docs,
            manifest.embedding.dimension,
        )
        .context("failed to rebuild helix store")?;
    }
    save_file_state(pack_dir, &next_states).context("failed to persist file state")?;

    let mut ontology = OntologyEngine::new(pack_dir)?;
    #[cfg(feature = "lance-falkor")]
    if let Some(socket_path) = socket_from_env() {
        let graph_name = graph_name_override
            .map(String::from)
            .unwrap_or_else(graph_name_from_env);
        let deleted_paths: Vec<String> = previous_paths
            .difference(&seen_paths)
            .cloned()
            .collect::<Vec<_>>();
        let mut paths_to_refresh = updated_paths.into_iter().collect::<Vec<_>>();
        paths_to_refresh.extend(deleted_paths);

        if !paths_to_refresh.is_empty() {
            if let Err(e) = delete_chunks_for_paths(&socket_path, &graph_name, &paths_to_refresh) {
                crate::term::warn(format!("warning: failed deleting stale graph chunks: {e}"));
            }
        }

        if !changed_docs.is_empty() {
            let graph_chunks = changed_docs
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
                .collect::<Vec<_>>();
            if let Err(e) = upsert_chunks(&socket_path, &graph_name, &graph_chunks) {
                crate::term::warn(format!("warning: failed writing chunks to falkor: {e}"));
            }
        }
    }
    #[cfg(feature = "store-helix-only")]
    {
        let mut all_entities = HashSet::new();
        let mut total_relationships = 0usize;
        for doc in &next_docs {
            let extraction = ontology.extract(&doc.content_hash, &doc.content, 12);
            for e in &extraction.entities {
                all_entities.insert(e.clone());
            }
            total_relationships += extraction.relations.len();
        }
        if let Err(e) = helix_write_graph_stats(pack_dir, all_entities.len(), total_relationships) {
            crate::term::warn(format!("warning: failed writing graph stats: {}", e));
        }
    }

    let mut source_contents: HashMap<String, Vec<String>> = HashMap::new();
    let mut source_hashes: HashMap<String, Vec<String>> = HashMap::new();
    for doc in &next_docs {
        source_contents
            .entry(doc.source_path.clone())
            .or_default()
            .push(doc.content.clone());
        source_hashes
            .entry(doc.source_path.clone())
            .or_default()
            .push(doc.content_hash.clone());
    }
    for (source_path, contents) in source_contents {
        let hashes = source_hashes.get(&source_path).cloned().unwrap_or_default();
        if let Err(e) = ontology.write_artifact(&source_path, &contents, &hashes) {
            crate::term::warn(format!(
                "warning: failed writing ontology artifact for {}: {}",
                source_path, e
            ));
        }
    }
    if let Err(e) = ontology.save() {
        crate::term::warn(format!("warning: failed writing ontology cache: {e}"));
    }

    manifest.updated_at = Utc::now();
    save_manifest(pack_dir, manifest).context("failed to update manifest timestamp")?;

    Ok((scanned, updated_files, total_chunks))
}
