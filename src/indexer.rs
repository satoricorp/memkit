use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::embed::provider_from_name;
use crate::lancedb_store::{ensure_chunk_ids, rebuild_tables};
use crate::pack::{
    load_file_state, load_index, load_manifest, save_file_state, save_index, save_manifest,
};
use crate::types::{FileState, IndexStore, SourceConfig, SourceDoc};

fn is_text_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());

    match ext.as_deref() {
        Some("rs" | "ts" | "tsx" | "js" | "jsx" | "md" | "txt" | "json" | "toml" | "yaml" | "yml")
        | None => true,
        _ => false,
    }
}

fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn content_hash(content: &str) -> String {
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    format!("{:x}", h.finalize())
}

fn chunk_text(content: &str, target_chars: usize, overlap_chars: usize) -> Vec<(usize, usize, String)> {
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

fn to_source_configs(sources: &[PathBuf]) -> Vec<SourceConfig> {
    sources
        .iter()
        .map(|p| SourceConfig {
            root_path: p.to_string_lossy().to_string(),
            include: vec!["**/*".to_string()],
            exclude: vec!["**/.git/**".to_string(), "**/target/**".to_string()],
        })
        .collect()
}

pub fn run_index(pack_dir: &Path, sources: &[PathBuf]) -> Result<(usize, usize, usize)> {
    let mut manifest = load_manifest(pack_dir)?;
    manifest.sources = to_source_configs(sources);
    let mut index = load_index(pack_dir)?;
    let mut file_states = load_file_state(pack_dir)?;

    let existing_by_chunk: HashMap<String, SourceDoc> = index
        .docs
        .drain(..)
        .map(|d| (d.chunk_id.clone(), d))
        .collect();
    let mut state_by_path: HashMap<String, FileState> = file_states
        .drain(..)
        .map(|s| (s.file_path.clone(), s))
        .collect();

    let mut scanned = 0usize;
    let mut updated_files = 0usize;
    let mut total_chunks = 0usize;
    let mut next_docs = Vec::new();
    let mut next_states = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut provider = provider_from_name(
        &manifest.embedding.provider,
        &manifest.embedding.model,
        manifest.embedding.dimension,
    )
    .or_else(|e| {
        if manifest.embedding.provider == "fastembed" {
            eprintln!(
                "warning: fastembed init failed ({}), falling back to hash embeddings",
                e
            );
            provider_from_name("hash", &manifest.embedding.model, manifest.embedding.dimension)
        } else {
            Err(e)
        }
    })?;

    for src in sources {
        for entry in WalkDir::new(src)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            if !is_text_file(path) {
                continue;
            }
            let Some(content) = read_text(path) else {
                continue;
            };
            scanned += 1;

            let file_path = path.to_string_lossy().to_string();
            seen_paths.insert(file_path.clone());
            let hash = content_hash(&content);
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
                for doc in existing_by_chunk.values().filter(|d| d.source_path == file_path) {
                    next_docs.push(doc.clone());
                    total_chunks += 1;
                }
                if let Some(s) = state_by_path.remove(&file_path) {
                    next_states.push(s);
                }
                continue;
            }

            updated_files += 1;
            let chunks = chunk_text(
                &content,
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
                next_docs.push(SourceDoc {
                    chunk_id,
                    source_path: file_path.clone(),
                    chunk_index: idx,
                    start_offset: start,
                    end_offset: end,
                    content: chunk,
                    content_hash: hash.clone(),
                    embedding,
                    indexed_at: Utc::now(),
                });
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

    index = IndexStore { docs: next_docs };
    ensure_chunk_ids(&mut index.docs);
    save_index(pack_dir, &index).context("failed to persist index")?;
    rebuild_tables(pack_dir, &index.docs, manifest.embedding.dimension)
        .context("failed to rebuild lancedb tables/indexes")?;
    save_file_state(pack_dir, &next_states).context("failed to persist file state")?;
    manifest.updated_at = Utc::now();
    save_manifest(pack_dir, manifest).context("failed to update manifest timestamp")?;

    Ok((scanned, updated_files, total_chunks))
}
