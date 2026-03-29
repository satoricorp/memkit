use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::embed::provider_from_name;
use crate::helix_store::{
    helix_clear_entity_map, helix_load_all_docs, helix_load_entity_id_map,
    helix_pack_path_for_local, helix_rebuild_chunks, helix_write_entities_edges,
    helix_write_graph_stats,
};
use crate::ontology::OntologyEngine;
use crate::pack::{load_file_state, load_manifest, save_file_state, save_manifest};
use crate::types::{FileState, SourceConfig, SourceDoc};

fn is_indexable_file(path: &Path) -> bool {
    if path.file_name().and_then(|n| n.to_str()) == Some(".DS_Store") {
        return false;
    }
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
    while start < content.len() {
        let end_byte = (start + target_chars).min(content.len());
        let end = content.floor_char_boundary(end_byte);
        let chunk = content[start..end].to_string();
        out.push((start, end, chunk));
        if end == content.len() {
            break;
        }
        let next_start = end.saturating_sub(overlap_chars);
        start = content.floor_char_boundary(next_start);
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
                exclude: vec![
                    "**/.git/**".to_string(),
                    "**/target/**".to_string(),
                    "**/.DS_Store".to_string(),
                ],
            }
        })
        .collect()
}

pub fn run_index(
    pack_dir: &Path,
    sources: &[PathBuf],
) -> Result<(usize, usize, usize, Vec<String>)> {
    let mut manifest = load_manifest(pack_dir)?;
    manifest.sources = to_source_configs(pack_dir, sources);
    let existing_docs: Vec<_> = helix_load_all_docs(
        &helix_pack_path_for_local(pack_dir),
        manifest.embedding.dimension,
    )?;
    let existing_by_chunk: HashMap<String, SourceDoc> = existing_docs
        .into_iter()
        .map(|d| (d.chunk_id.clone(), d))
        .collect();
    let mut file_states = load_file_state(pack_dir)?;
    let mut state_by_path: HashMap<String, FileState> = file_states
        .drain(..)
        .map(|s| (s.file_path.clone(), s))
        .collect();

    let mut scanned = 0usize;
    let mut updated_files = 0usize;
    let mut total_chunks = 0usize;
    let mut index_warnings: Vec<String> = Vec::new();
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
            let file_path = path.to_string_lossy().to_string();
            let Some(content) = crate::extract::extract_text(path) else {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_ascii_lowercase());
                let likely_binary_issue = matches!(
                    ext.as_deref(),
                    Some("pdf" | "doc" | "docx" | "xls" | "xlsx" | "xlsb")
                );
                let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                if likely_binary_issue || (is_indexable_file(path) && size > 0) {
                    if index_warnings.len() < 200 {
                        let detail = match ext.as_deref() {
                            Some("pdf") => {
                                "skipped (PDF: unreadable, encrypted, invalid stream, or empty—pdf-extract often fails on bad streams)"
                            }
                            Some("doc" | "docx" | "xls" | "xlsx" | "xlsb") => {
                                "skipped (office file could not be read or is empty)"
                            }
                            _ => "skipped (no text extracted)",
                        };
                        index_warnings.push(format!("{}: {}", file_path, detail));
                    }
                }
                continue;
            };
            scanned += 1;
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

    helix_rebuild_chunks(
        &helix_pack_path_for_local(pack_dir),
        &next_docs,
        manifest.embedding.dimension,
    )
    .context("failed to rebuild helix store")?;
    helix_clear_entity_map(pack_dir);
    save_file_state(pack_dir, &next_states).context("failed to persist file state")?;

    let mut ontology = OntologyEngine::new(pack_dir)?;
    let mut all_entities = HashSet::new();
    let mut all_relations = Vec::new();
    for doc in &next_docs {
        let extraction = ontology.extract(&doc.content_hash, &doc.content, 12);
        for e in &extraction.entities {
            all_entities.insert(e.clone());
        }
        all_relations.extend(extraction.relations.clone());
    }
    let path = helix_pack_path_for_local(pack_dir);
    let mut entity_map = helix_load_entity_id_map(pack_dir);
    if let Err(e) = helix_write_entities_edges(
        &path,
        pack_dir,
        &mut entity_map,
        &all_entities,
        &all_relations,
    ) {
        crate::term::warn(format!(
            "warning: failed writing entities/edges to helix: {}",
            e
        ));
    }
    let unique_source_paths: Vec<String> = {
        let mut v: Vec<String> = next_docs.iter().map(|d| d.source_path.clone()).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let relationship_count_unique = all_relations
        .iter()
        .map(|r| (r.source.clone(), r.relation.clone(), r.target.clone()))
        .collect::<HashSet<_>>()
        .len();
    if let Err(e) = helix_write_graph_stats(
        pack_dir,
        entity_map.len(),
        relationship_count_unique,
        total_chunks,
        &unique_source_paths,
        &index_warnings,
    ) {
        crate::term::warn(format!("warning: failed writing graph stats: {}", e));
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

    Ok((scanned, updated_files, total_chunks, index_warnings))
}
