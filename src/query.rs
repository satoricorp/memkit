use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;

use crate::embed::provider_from_name;
use crate::falkor_store::{
    graph_name_for_pack, graph_name_from_env, query_chunks as query_falkor_chunks, socket_from_env,
};
use crate::lancedb_store::hybrid_query_with_uri;
use crate::pack_location::PackLocation;
use crate::pack::{load_index_from_loc, load_manifest_from_loc};
use crate::rerank::{try_create_reranker, DEFAULT_RERANKER_MODEL};
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
    graph_name_override: Option<&str>,
    path_filter: Option<&str>,
) -> Result<QueryResponse> {
    let total_start = Instant::now();
    let manifest = load_manifest_from_loc(loc)?;
    let index = load_index_from_loc(loc)?;

    let embed_start = Instant::now();
    let mut provider = provider_from_name(
        &manifest.embedding.provider,
        &manifest.embedding.model,
        manifest.embedding.dimension,
    )
    .or_else(|e| {
        if manifest.embedding.provider == "fastembed" {
            crate::term::warn(format!(
                "warning: fastembed query init failed ({}), falling back to hash embeddings",
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
    let q_embedding = provider.embed_query(q)?;
    let embed_ms = embed_start.elapsed().as_millis();

    let retrieval_start = Instant::now();
    let lancedb_uri = loc.lancedb_uri();
    let storage_options = loc.storage_options().map(|o| o.to_vec());
    let query_text = q.to_string();
    let query_embedding = q_embedding.clone();
    let falkor_socket = socket_from_env();
    let graph_name = graph_name_override
        .map(String::from)
        .unwrap_or_else(graph_name_from_env);
    let top_for_backend = top_k.saturating_mul(2);
    let path_filter_lance = path_filter.map(String::from);
    let path_filter_falkor = path_filter.map(String::from);
    let (mut lancedb_hits, falkor_hits) = std::thread::scope(|scope| {
        let lance = scope.spawn(|| {
            hybrid_query_with_uri(
                &lancedb_uri,
                storage_options.as_deref(),
                &query_text,
                &query_embedding,
                top_for_backend,
                path_filter_lance.as_deref(),
            )
            .unwrap_or_default()
        });
        let graph = scope.spawn(|| {
            if let Some(socket_path) = falkor_socket {
                query_falkor_chunks(
                    &socket_path,
                    &graph_name,
                    &query_text,
                    top_for_backend,
                    path_filter_falkor.as_deref(),
                )
                .unwrap_or_else(|err| {
                    crate::term::warn(format!(
                        "warning: falkor query failed, continuing with lancedb: {err}"
                    ));
                    Vec::new()
                })
            } else {
                Vec::new()
            }
        });

        let lancedb_hits = lance.join().unwrap_or_default();
        let falkor_hits = graph.join().unwrap_or_default();
        (lancedb_hits, falkor_hits)
    });
    let retrieval_ms = retrieval_start.elapsed().as_millis();

    let rerank_start = Instant::now();
    let index_docs = index.docs;

    if lancedb_hits.is_empty() && falkor_hits.is_empty() {
        let docs = index_docs.iter().filter(|d| {
            path_filter.map_or(true, |pf| {
                let p = d.source_path.replace('\\', "/");
                let pf_norm = pf.replace('\\', "/");
                p.contains(&pf_norm)
            })
        });
        lancedb_hits = docs
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

    let mut merged: HashMap<String, QueryHit> = HashMap::new();
    for (rank, hit) in lancedb_hits.into_iter().enumerate() {
        let rr = 1.0 / (60.0 + rank as f32 + 1.0);
        merged
            .entry(hit.chunk_id.clone())
            .and_modify(|e| {
                e.score += rr;
                e.source = "vector".to_string();
            })
            .or_insert(QueryHit {
                score: rr,
                source: "vector".to_string(),
                ..hit
            });
    }
    for (rank, hit) in falkor_hits.into_iter().enumerate() {
        let rr = (1.0 / (60.0 + rank as f32 + 1.0)) * 0.9;
        merged
            .entry(hit.chunk_id.clone())
            .and_modify(|e| {
                e.score += rr;
                e.source = if e.source == "vector" { "both" } else { "graph" }.to_string();
            })
            .or_insert(QueryHit {
                score: rr,
                source: "graph".to_string(),
                ..hit
            });
    }

    let mut hits: Vec<QueryHit> = merged
        .into_values()
        .map(|d| QueryHit {
            group_key: Some(d.file_path.clone()),
            ..d
        })
        .filter(|h| h.score > 0.0)
        .collect();

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
        return run_query(&PackLocation::local(&packs[0]), q, top_k, use_reranker, None, path_filter);
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
                    let graph_name = loc.as_path().and_then(|p| graph_name_for_pack(p).ok());
                    run_query(
                        &loc,
                        &q,
                        top_for_backend,
                        use_reranker,
                        graph_name.as_deref(),
                        pf.as_deref(),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or(Err(anyhow::anyhow!("thread join failed"))))
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
    let grouped_results: Vec<QueryGroup> = all_grouped
        .into_iter()
        .take(top_k)
        .collect();

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
    })
}
