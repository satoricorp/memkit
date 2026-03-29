use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;

use crate::embed::provider_from_name;
use crate::helix_store::{helix_hybrid_query, helix_load_all_docs};
use crate::pack::load_manifest_from_loc;
use crate::pack_location::PackLocation;
use crate::rerank::{DEFAULT_RERANKER_MODEL, try_create_reranker};
use crate::types::{QueryGroup, QueryHit, QueryResponse, QueryTimings};

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

const RERANK_LIMIT: usize = 50;

pub fn run_query(
    loc: &PackLocation,
    q: &str,
    top_k: usize,
    use_reranker: bool,
    path_filter: Option<&str>,
) -> Result<QueryResponse> {
    let total_start = Instant::now();
    let manifest = load_manifest_from_loc(loc)?;
    let dim = manifest.embedding.dimension;
    let helix_path = loc.helix_path();
    let index_docs: Vec<_> = helix_load_all_docs(&helix_path, dim)?;

    let embed_start = Instant::now();
    let mut provider = provider_from_name(
        &manifest.embedding.provider,
        &manifest.embedding.model,
        dim,
    )
    .or_else(|e| {
        if manifest.embedding.provider == "fastembed" {
            crate::term::warn(format!(
                "warning: fastembed query init failed ({}), falling back to hash embeddings",
                e
            ));
            provider_from_name("hash", &manifest.embedding.model, dim)
        } else {
            Err(e)
        }
    })?;
    let q_embedding = provider.embed_query(q)?;
    let embed_ms = embed_start.elapsed().as_millis();

    let retrieval_start = Instant::now();
    let top_for_backend = top_k.saturating_mul(2);
    let mut hits: Vec<QueryHit> =
        helix_hybrid_query(&helix_path, q, &q_embedding, top_for_backend, path_filter)?;
    let retrieval_results = hits.clone();
    let retrieval_ms = retrieval_start.elapsed().as_millis();

    let rerank_start = Instant::now();

    if hits.is_empty() {
        let docs = index_docs.iter().filter(|d| {
            path_filter.map_or(true, |pf| {
                let p = d.source_path.replace('\\', "/");
                let pf_norm = pf.replace('\\', "/");
                p.contains(&pf_norm)
            })
        });
        hits = docs
            .map(|d| {
                let vec = cosine(&q_embedding, &d.embedding);
                QueryHit {
                    score: vec,
                    file_path: d.source_path.clone(),
                    chunk_id: d.chunk_id.clone(),
                    chunk_index: d.chunk_index,
                    content: d.content.clone(),
                    start_offset: Some(d.start_offset),
                    end_offset: Some(d.end_offset),
                    source: "vector".to_string(),
                    group_key: Some(d.source_path.clone()),
                }
            })
            .filter(|h| h.score > 0.0)
            .collect();
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });

    let mode = if use_reranker && !hits.is_empty() {
        if let Ok(Some(mut reranker)) = try_create_reranker(DEFAULT_RERANKER_MODEL) {
            let limit = hits.len().min(RERANK_LIMIT);
            let to_rerank: Vec<QueryHit> = hits.drain(..limit).collect();
            let contents: Vec<&str> = to_rerank.iter().map(|h| h.content.as_str()).collect();
            let rest: Vec<QueryHit> = hits.drain(..).collect();
            match reranker.rerank(q, &contents, false, None) {
                Ok(results) => {
                    let reordered: Vec<QueryHit> = results
                        .into_iter()
                        .map(|r| {
                            let mut h = to_rerank[r.index].clone();
                            h.score = r.score;
                            h
                        })
                        .collect();
                    hits = reordered;
                    hits.extend(rest);
                    "rerank"
                }
                Err(_) => {
                    hits = to_rerank;
                    hits.extend(rest);
                    hits.sort_by(|a, b| {
                        b.score
                            .partial_cmp(&a.score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| a.file_path.cmp(&b.file_path))
                    });
                    "fusion"
                }
            }
        } else {
            "fusion"
        }
    } else {
        "fusion"
    };

    hits.truncate(top_k);
    let rerank_ms = rerank_start.elapsed().as_millis();

    let mut group_map: HashMap<String, QueryGroup> = HashMap::new();
    for hit in &hits {
        let group_key = hit
            .group_key
            .clone()
            .unwrap_or_else(|| hit.file_path.clone());
        group_map
            .entry(group_key.clone())
            .and_modify(|g| {
                g.score = g.score.max(hit.score);
                if g.hits.len() < 3 {
                    g.hits.push(hit.clone());
                }
            })
            .or_insert(QueryGroup {
                group_key,
                score: hit.score,
                hits: vec![hit.clone()],
            });
    }
    let mut grouped_results = group_map.into_values().collect::<Vec<_>>();
    grouped_results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(QueryResponse {
        results: hits,
        mode: mode.to_string(),
        grouped_results,
        timings_ms: QueryTimings {
            embed: embed_ms,
            retrieval: retrieval_ms,
            rerank: rerank_ms,
            total: total_start.elapsed().as_millis(),
        },
        retrieval_results: Some(retrieval_results),
    })
}

pub fn run_query_multi(
    packs: &[PathBuf],
    q: &str,
    top_k: usize,
    use_reranker: bool,
    path_filter: Option<&str>,
) -> Result<QueryResponse> {
    if packs.is_empty() {
        return Err(anyhow::anyhow!("at least one pack required"));
    }
    if packs.len() == 1 {
        return run_query(
            &PackLocation::local(&packs[0]),
            q,
            top_k,
            use_reranker,
            path_filter,
        );
    }

    let total_start = Instant::now();
    let top_for_backend = top_k.saturating_mul(2);
    let path_filter_owned = path_filter.map(String::from);
    let results: Vec<QueryResponse> = std::thread::scope(|scope| {
        let handles: Vec<_> = packs
            .iter()
            .map(|pack| {
                let loc = PackLocation::local(pack.clone());
                let q = q.to_string();
                let pf = path_filter_owned.clone();
                scope.spawn(move || {
                    run_query(&loc, &q, top_for_backend, use_reranker, pf.as_deref())
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or(Err(anyhow::anyhow!("thread join failed")))
            })
            .collect::<Result<Vec<_>>>()
    })?;

    let mut all_hits = Vec::new();
    let mut all_grouped = Vec::new();
    let mut max_embed = 0u128;
    let mut max_retrieval = 0u128;
    let mut max_rerank = 0u128;
    for r in &results {
        all_hits.extend(r.results.clone());
        all_grouped.extend(r.grouped_results.clone());
        max_embed = max_embed.max(r.timings_ms.embed);
        max_retrieval = max_retrieval.max(r.timings_ms.retrieval);
        max_rerank = max_rerank.max(r.timings_ms.rerank);
    }

    all_hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    all_hits.truncate(top_k);

    all_grouped.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let grouped_results: Vec<QueryGroup> = all_grouped.into_iter().take(top_k).collect();

    Ok(QueryResponse {
        results: all_hits,
        mode: if use_reranker { "rerank" } else { "fusion" }.to_string(),
        grouped_results,
        timings_ms: QueryTimings {
            embed: max_embed,
            retrieval: max_retrieval,
            rerank: max_rerank,
            total: total_start.elapsed().as_millis(),
        },
        retrieval_results: None,
    })
}
