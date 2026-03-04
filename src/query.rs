use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;

use crate::embed::provider_from_name;
use crate::lancedb_store::hybrid_query;
use crate::pack::load_manifest;
use crate::pack::load_index;
use crate::types::{QueryHit, QueryResponse};

fn tokenize(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn lexical_score(query: &HashSet<String>, content: &str) -> f32 {
    if query.is_empty() {
        return 0.0;
    }
    let doc = tokenize(content);
    let overlap = query.intersection(&doc).count() as f32;
    overlap / query.len() as f32
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

pub fn run_query(pack_dir: &Path, q: &str, mode: &str, top_k: usize) -> Result<QueryResponse> {
    let manifest = load_manifest(pack_dir)?;
    let index = load_index(pack_dir)?;
    let q_tokens = tokenize(q);
    let mut provider = provider_from_name(
        &manifest.embedding.provider,
        &manifest.embedding.model,
        manifest.embedding.dimension,
    )
    .or_else(|e| {
        if manifest.embedding.provider == "fastembed" {
            eprintln!(
                "warning: fastembed query init failed ({}), falling back to hash embeddings",
                e
            );
            provider_from_name("hash", &manifest.embedding.model, manifest.embedding.dimension)
        } else {
            Err(e)
        }
    })?;
    let q_embedding = provider.embed_query(q)?;

    if mode == "hybrid" {
        let results = hybrid_query(pack_dir, q, &q_embedding, top_k)?;
        if !results.is_empty() {
            return Ok(QueryResponse {
                results,
                mode: mode.to_string(),
            });
        }
    }

    let mut hits: Vec<QueryHit> = index
        .docs
        .into_iter()
        .map(|d| {
            let lex = lexical_score(&q_tokens, &d.content);
            let vec = cosine(&q_embedding, &d.embedding);
            let s = match mode {
                "vector" => vec,
                "hybrid" => (0.6 * vec) + (0.4 * lex),
                _ => (0.6 * vec) + (0.4 * lex),
            };
            QueryHit {
                score: s,
                file_path: d.source_path.clone(),
                chunk_id: d.chunk_id,
                chunk_index: d.chunk_index,
                content: d.content,
            }
        })
        .filter(|h| h.score > 0.0)
        .collect();

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    hits.truncate(top_k);

    Ok(QueryResponse {
        results: hits,
        mode: mode.to_string(),
    })
}
