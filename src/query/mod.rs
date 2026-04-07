use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;

use crate::conversation::{
    build_query_note, expand_query_variants, query_time_analysis, shape_score_boost,
    should_hydrate_evidence, temporal_score_boost,
};
use crate::embed::provider_from_name;
use crate::helix_store::helix_hybrid_query;
use crate::pack::load_manifest_from_loc;
use crate::pack_location::PackLocation;
use crate::rerank::{DEFAULT_RERANKER_MODEL, try_create_reranker};
use crate::types::{QueryEvidence, QueryGroup, QueryHit, QueryResponse, QueryTimings};

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
    let query_variants = expand_query_variants(q);
    let variant_embeddings = query_variants
        .iter()
        .map(|variant| provider.embed_query(variant))
        .collect::<Result<Vec<_>, _>>()?;
    let embed_ms = embed_start.elapsed().as_millis();

    let query_time = query_time_analysis(q);
    let retrieval_start = Instant::now();
    let top_for_backend = top_k.saturating_mul(4).max(12);
    let mut merged_hits: HashMap<String, QueryHit> = HashMap::new();
    for (variant_index, (variant, embedding)) in query_variants
        .iter()
        .zip(variant_embeddings.iter())
        .enumerate()
    {
        let variant_hits = helix_hybrid_query(
            &helix_path,
            variant,
            embedding,
            top_for_backend,
            path_filter,
        )?;
        for mut hit in variant_hits {
            hit.score -= variant_index as f32 * 0.01;
            match merged_hits.get_mut(&hit.chunk_id) {
                Some(existing) if hit.score > existing.score => *existing = hit,
                None => {
                    merged_hits.insert(hit.chunk_id.clone(), hit);
                }
                _ => {}
            }
        }
    }
    let mut hits: Vec<QueryHit> = merged_hits.into_values().collect();
    if hits
        .iter()
        .any(|hit| hit.memory.doc_kind == "memory_record")
    {
        hits.retain(|hit| hit.memory.doc_kind == "memory_record");
    }
    for hit in &mut hits {
        hit.score += temporal_score_boost(hit, &query_time);
        hit.score += shape_score_boost(hit, &query_time);
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    let retrieval_results = hits.clone();
    let retrieval_ms = retrieval_start.elapsed().as_millis();

    let rerank_start = Instant::now();

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
    let hydrate_evidence = should_hydrate_evidence(&query_time, &hits);
    let notes = hits
        .iter()
        .map(|hit| build_query_note(hit, hydrate_evidence, &query_time))
        .collect::<Vec<_>>();
    let hydrated_evidence = if hydrate_evidence {
        hits.iter()
            .filter_map(|hit| {
                hit.memory
                    .evidence_content
                    .as_ref()
                    .map(|evidence| QueryEvidence {
                        chunk_id: hit.chunk_id.clone(),
                        evidence: evidence.clone(),
                    })
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

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
        resolved_pack_path: loc.debug_display_path(),
        grouped_results,
        timings_ms: QueryTimings {
            embed: embed_ms,
            retrieval: retrieval_ms,
            rerank: rerank_ms,
            total: total_start.elapsed().as_millis(),
        },
        retrieval_results: Some(retrieval_results),
        notes,
        hydrated_evidence,
        query_time: Some(query_time),
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
    let mut all_notes = Vec::new();
    let mut all_evidence = Vec::new();
    let mut max_embed = 0u128;
    let mut max_retrieval = 0u128;
    let mut max_rerank = 0u128;
    for r in &results {
        all_hits.extend(r.results.clone());
        all_grouped.extend(r.grouped_results.clone());
        all_notes.extend(r.notes.clone());
        all_evidence.extend(r.hydrated_evidence.clone());
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
        resolved_pack_path: None,
        grouped_results,
        timings_ms: QueryTimings {
            embed: max_embed,
            retrieval: max_retrieval,
            rerank: max_rerank,
            total: total_start.elapsed().as_millis(),
        },
        retrieval_results: None,
        notes: all_notes,
        hydrated_evidence: all_evidence,
        query_time: None,
    })
}
