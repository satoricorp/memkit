//! HelixDB store: one LMDB per memory pack, organized as <base_dir>/<user_id>/<memory_pack_id>/.
//! Requires `helix` feature.
//! Chunks are stored as vectors; entities as nodes and relationships as edges in the same Helix DB.
//! Entity/relationship counts are also persisted in graph_stats.json for status.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
#[cfg(feature = "helix")]
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use chrono::Utc;

use crate::types::{GraphRelation, QueryHit, SourceDoc};

const CHUNK_LABEL: &str = "chunk";
const ENTITY_LABEL: &str = "Entity";
const ENTITY_IDS_FILE: &str = "entity_ids.json";

/// LMDB only allows one open `Env` per on-disk path per process. Serialize all Helix opens
/// (index, query, entity writes, remove) so concurrent handlers never hit `Env already open`.
#[cfg(feature = "helix")]
static HELIX_STORE_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
#[cfg(feature = "helix")]
static HELIX_STORAGE_CACHE: OnceLock<
    Mutex<HashMap<PathBuf, Arc<helix_db::helix_engine::storage_core::HelixGraphStorage>>>,
> = OnceLock::new();

#[cfg(feature = "helix")]
fn helix_store_guard() -> std::sync::MutexGuard<'static, ()> {
    HELIX_STORE_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Base directory for all Helix pack DBs. From env MEMKIT_HELIX_ROOT or default ~/.memkit/helix.
pub fn helix_base_dir() -> PathBuf {
    std::env::var("MEMKIT_HELIX_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .map(|h| h.join(".memkit").join("helix"))
                .unwrap_or_else(|| PathBuf::from(".memkit/helix"))
        })
}

/// Path for one pack's Helix DB: <base_dir>/<user_id>/<memory_pack_id>/.
pub fn helix_pack_path(base_dir: &Path, user_id: &str, memory_pack_id: &str) -> PathBuf {
    base_dir.join(user_id).join(memory_pack_id)
}

/// Helix path for a local pack directory (user_id = "default", id = sanitized path).
pub fn helix_pack_path_for_local(pack_dir: &Path) -> PathBuf {
    let base = helix_base_dir();
    let path_buf = pack_dir
        .canonicalize()
        .ok()
        .unwrap_or_else(|| pack_dir.to_path_buf());
    let id = path_buf
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "_");
    helix_pack_path(&base, "default", &id)
}

/// Remove the Helix database for a local pack. No-op if the path does not exist.
/// Tries both pack_root and pack_root.join(".memkit") so we match how the indexer
/// stores helix (under the .memkit pack path).
#[cfg(feature = "helix")]
pub fn remove_helix_for_pack(pack_root: &Path) -> Result<()> {
    let _guard = helix_store_guard();
    use std::fs;
    for candidate in [pack_root.to_path_buf(), pack_root.join(".memkit")] {
        let path = helix_pack_path_for_local(&candidate);
        clear_helix_storage_cache(&path);
        if path.exists() && path.is_dir() {
            fs::remove_dir_all(&path).context("failed to remove Helix pack DB")?;
        }
    }
    Ok(())
}

#[cfg(feature = "helix")]
fn open_helix_storage(
    path: &Path,
) -> Result<Arc<helix_db::helix_engine::storage_core::HelixGraphStorage>> {
    use helix_db::helix_engine::storage_core::version_info::VersionInfo;
    use helix_db::helix_engine::traversal_core::config::Config;

    std::fs::create_dir_all(path).context("create helix pack dir")?;
    let cache_key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let cache = HELIX_STORAGE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(storage) = cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&cache_key)
        .cloned()
    {
        return Ok(storage);
    }

    let path_str = cache_key
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-UTF8 path"))?;
    let config = Config::default();
    let version_info = VersionInfo::default();
    let storage = helix_db::helix_engine::storage_core::HelixGraphStorage::new(
        path_str,
        config,
        version_info,
    )
    .map_err(|e| anyhow::anyhow!("helix open: {:?}", e))?;
    let storage = Arc::new(storage);
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(cache_key, storage.clone());
    Ok(storage)
}

#[cfg(feature = "helix")]
fn clear_helix_storage_cache(path: &Path) {
    if let Some(cache) = HELIX_STORAGE_CACHE.get() {
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(path);
        if let Ok(canonical) = path.canonicalize() {
            guard.remove(&canonical);
        }
    }
}

#[cfg(feature = "helix")]
fn doc_to_properties_and_embedding<'a>(
    doc: &SourceDoc,
    arena: &'a bumpalo::Bump,
) -> (
    Option<helix_db::utils::properties::ImmutablePropertiesMap<'a>>,
    Vec<f64>,
) {
    use helix_db::protocol::value::Value;

    let embedding_f64: Vec<f64> = doc.embedding.iter().map(|&x| x as f64).collect();
    let mut keys_and_values: Vec<(&str, Value)> = vec![
        ("chunk_id", Value::String(doc.chunk_id.clone())),
        ("source_path", Value::String(doc.source_path.clone())),
        ("content", Value::String(doc.content.clone())),
        ("content_hash", Value::String(doc.content_hash.clone())),
        ("chunk_index", Value::I32(doc.chunk_index as i32)),
        ("start_offset", Value::I64(doc.start_offset as i64)),
        ("end_offset", Value::I64(doc.end_offset as i64)),
        ("indexed_at", Value::String(doc.indexed_at.to_rfc3339())),
    ];
    keys_and_values.push(("doc_kind", Value::String(doc.memory.doc_kind.clone())));
    if let Some(value) = &doc.memory.record_type {
        keys_and_values.push(("record_type", Value::String(value.clone())));
    }
    if let Some(value) = &doc.memory.session_id {
        keys_and_values.push(("session_id", Value::String(value.clone())));
    }
    if let Some(value) = doc.memory.session_index {
        keys_and_values.push(("session_index", Value::I64(value as i64)));
    }
    if let Some(value) = doc.memory.turn_start {
        keys_and_values.push(("turn_start", Value::I64(value as i64)));
    }
    if let Some(value) = doc.memory.turn_end {
        keys_and_values.push(("turn_end", Value::I64(value as i64)));
    }
    if let Some(value) = &doc.memory.role {
        keys_and_values.push(("role", Value::String(value.clone())));
    }
    if let Some(value) = &doc.memory.session_time_start {
        keys_and_values.push(("session_time_start", Value::String(value.to_rfc3339())));
    }
    if let Some(value) = &doc.memory.session_time_end {
        keys_and_values.push(("session_time_end", Value::String(value.to_rfc3339())));
    }
    if let Some(value) = &doc.memory.context_time_start {
        keys_and_values.push(("context_time_start", Value::String(value.to_rfc3339())));
    }
    if let Some(value) = &doc.memory.context_time_end {
        keys_and_values.push(("context_time_end", Value::String(value.to_rfc3339())));
    }
    if let Some(value) = &doc.memory.context_time_text {
        keys_and_values.push(("context_time_text", Value::String(value.clone())));
    }
    if let Some(value) = &doc.memory.temporal_kind {
        keys_and_values.push(("temporal_kind", Value::String(value.clone())));
    }
    if let Some(value) = doc.memory.temporal_confidence {
        keys_and_values.push((
            "temporal_confidence",
            Value::String(format!("{:.3}", value)),
        ));
    }
    if let Some(value) = &doc.memory.evidence_chunk_id {
        keys_and_values.push(("evidence_chunk_id", Value::String(value.clone())));
    }
    if let Some(value) = &doc.memory.evidence_content {
        keys_and_values.push(("evidence_content", Value::String(value.clone())));
    }
    if let Some(value) = &doc.memory.extraction_provider {
        keys_and_values.push(("extraction_provider", Value::String(value.clone())));
    }
    if let Some(value) = &doc.memory.extraction_model {
        keys_and_values.push(("extraction_model", Value::String(value.clone())));
    }
    if let Some(value) = &doc.memory.relation_kind {
        keys_and_values.push(("relation_kind", Value::String(value.clone())));
    }
    if let Some(value) = &doc.memory.entity_kind {
        keys_and_values.push(("entity_kind", Value::String(value.clone())));
    }
    if let Some(value) = &doc.memory.value_kind {
        keys_and_values.push(("value_kind", Value::String(value.clone())));
    }
    let len = keys_and_values.len();
    let items = keys_and_values.into_iter().map(|(k, v)| {
        let k_arena = arena.alloc_str(k);
        (k_arena as &str, v)
    });
    let props = helix_db::utils::properties::ImmutablePropertiesMap::new(len, items, arena);
    (Some(props), embedding_f64)
}

#[cfg(feature = "helix")]
fn insert_docs_into_storage(
    storage: &helix_db::helix_engine::storage_core::HelixGraphStorage,
    docs: &[SourceDoc],
) -> Result<()> {
    use heed3::RoTxn;
    use helix_db::helix_engine::traversal_core::ops::g::G;
    use helix_db::helix_engine::traversal_core::ops::vectors::insert::InsertVAdapter;
    use helix_db::helix_engine::vector_core::vector::HVector;

    let mut txn = storage
        .graph_env
        .write_txn()
        .map_err(|e| anyhow::anyhow!("helix write_txn: {:?}", e))?;
    let arena = bumpalo::Bump::new();

    for doc in docs {
        let (props, embedding_f64) = doc_to_properties_and_embedding(doc, &arena);
        let _ = G::new_mut(storage, &arena, &mut txn)
            .insert_v::<fn(&HVector, &RoTxn) -> bool>(&embedding_f64, CHUNK_LABEL, props)
            .collect_to_obj()
            .map_err(|e| anyhow::anyhow!("helix insert_v: {:?}", e))?;
    }

    txn.commit()
        .map_err(|e| anyhow::anyhow!("helix commit: {:?}", e))?;
    Ok(())
}

/// Rebuild chunks at the given Helix path. Clears the directory first if it exists.
pub fn helix_rebuild_chunks(path: &Path, docs: &[SourceDoc], _embedding_dim: usize) -> Result<()> {
    #[cfg(feature = "helix")]
    {
        let _guard = helix_store_guard();
        clear_helix_storage_cache(path);
        if path.exists() {
            std::fs::remove_dir_all(path).context("remove existing helix dir")?;
        }
        std::fs::create_dir_all(path).context("create helix pack dir")?;
        let storage = open_helix_storage(path)?;
        insert_docs_into_storage(&storage, docs)?;
        Ok(())
    }
    #[cfg(not(feature = "helix"))]
    {
        let _ = (path, docs, _embedding_dim);
        anyhow::bail!("build with --features helix to use Helix store");
    }
}

/// Append docs to the pack's Helix DB.
pub fn helix_append_chunks(path: &Path, docs: &[SourceDoc], _embedding_dim: usize) -> Result<()> {
    #[cfg(feature = "helix")]
    {
        let _guard = helix_store_guard();
        if docs.is_empty() {
            return Ok(());
        }
        let storage = open_helix_storage(path)?;
        insert_docs_into_storage(&storage, docs)?;
        Ok(())
    }
    #[cfg(not(feature = "helix"))]
    {
        let _ = (path, docs, _embedding_dim);
        anyhow::bail!("build with --features helix to use Helix store");
    }
}

#[cfg(feature = "helix")]
fn value_as_str(v: &helix_db::protocol::value::Value) -> Option<&str> {
    use helix_db::protocol::value::Value;
    match v {
        Value::String(s) => Some(s.as_str()),
        _ => None,
    }
}

#[cfg(feature = "helix")]
fn value_as_i64(v: &helix_db::protocol::value::Value) -> Option<i64> {
    use helix_db::protocol::value::Value;
    match v {
        Value::I8(x) => Some(*x as i64),
        Value::I16(x) => Some(*x as i64),
        Value::I32(x) => Some(*x as i64),
        Value::I64(x) => Some(*x),
        _ => None,
    }
}

#[cfg(feature = "helix")]
fn value_as_datetime(v: &helix_db::protocol::value::Value) -> Option<chrono::DateTime<Utc>> {
    value_as_str(v)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(feature = "helix")]
fn value_as_f32(v: &helix_db::protocol::value::Value) -> Option<f32> {
    value_as_str(v).and_then(|s| s.parse::<f32>().ok())
}

#[cfg(feature = "helix")]
fn hvector_to_source_doc(
    v: &helix_db::helix_engine::vector_core::vector::HVector,
) -> Option<SourceDoc> {
    let p = v.properties.as_ref()?;
    let chunk_id = p.get("chunk_id").and_then(value_as_str).map(String::from)?;
    let source_path = p
        .get("source_path")
        .and_then(value_as_str)
        .map(String::from)?;
    let content = p.get("content").and_then(value_as_str).map(String::from)?;
    let content_hash = p
        .get("content_hash")
        .and_then(value_as_str)
        .map(String::from)?;
    let chunk_index = p.get("chunk_index").and_then(value_as_i64).unwrap_or(0) as usize;
    let start_offset = p.get("start_offset").and_then(value_as_i64).unwrap_or(0) as usize;
    let end_offset = p.get("end_offset").and_then(value_as_i64).unwrap_or(0) as usize;
    let indexed_at = p
        .get("indexed_at")
        .and_then(value_as_datetime)
        .unwrap_or_else(Utc::now);
    let embedding: Vec<f32> = v.data.iter().map(|&x| x as f32).collect();
    Some(SourceDoc {
        chunk_id,
        source_path,
        chunk_index,
        start_offset,
        end_offset,
        content,
        content_hash,
        embedding,
        indexed_at,
        memory: crate::types::MemoryMetadata {
            doc_kind: p
                .get("doc_kind")
                .and_then(value_as_str)
                .map(String::from)
                .unwrap_or_else(|| "source_chunk".to_string()),
            record_type: p
                .get("record_type")
                .and_then(value_as_str)
                .map(String::from),
            session_id: p.get("session_id").and_then(value_as_str).map(String::from),
            session_index: p
                .get("session_index")
                .and_then(value_as_i64)
                .map(|v| v as usize),
            turn_start: p
                .get("turn_start")
                .and_then(value_as_i64)
                .map(|v| v as usize),
            turn_end: p.get("turn_end").and_then(value_as_i64).map(|v| v as usize),
            role: p.get("role").and_then(value_as_str).map(String::from),
            session_time_start: p.get("session_time_start").and_then(value_as_datetime),
            session_time_end: p.get("session_time_end").and_then(value_as_datetime),
            context_time_start: p.get("context_time_start").and_then(value_as_datetime),
            context_time_end: p.get("context_time_end").and_then(value_as_datetime),
            context_time_text: p
                .get("context_time_text")
                .and_then(value_as_str)
                .map(String::from),
            temporal_kind: p
                .get("temporal_kind")
                .and_then(value_as_str)
                .map(String::from),
            temporal_confidence: p.get("temporal_confidence").and_then(value_as_f32),
            evidence_chunk_id: p
                .get("evidence_chunk_id")
                .and_then(value_as_str)
                .map(String::from),
            evidence_content: p
                .get("evidence_content")
                .and_then(value_as_str)
                .map(String::from),
            extraction_provider: p
                .get("extraction_provider")
                .and_then(value_as_str)
                .map(String::from),
            extraction_model: p
                .get("extraction_model")
                .and_then(value_as_str)
                .map(String::from),
            relation_kind: p
                .get("relation_kind")
                .and_then(value_as_str)
                .map(String::from),
            entity_kind: p
                .get("entity_kind")
                .and_then(value_as_str)
                .map(String::from),
            value_kind: p.get("value_kind").and_then(value_as_str).map(String::from),
        },
    })
}

/// Load all chunks from the pack's Helix DB.
pub fn helix_load_all_docs(path: &Path, _dim: usize) -> Result<Vec<SourceDoc>> {
    #[cfg(feature = "helix")]
    {
        let _guard = helix_store_guard();
        use helix_db::helix_engine::traversal_core::ops::g::G;
        use helix_db::helix_engine::traversal_core::ops::source::v_from_type::VFromTypeAdapter;
        use helix_db::helix_engine::traversal_core::traversal_value::TraversalValue;

        let storage = open_helix_storage(path)?;
        let txn = storage
            .graph_env
            .read_txn()
            .map_err(|e| anyhow::anyhow!("helix read_txn: {:?}", e))?;
        let arena = bumpalo::Bump::new();

        let vectors: Vec<_> = G::new(&*storage, &txn, &arena)
            .v_from_type(CHUNK_LABEL, true)
            .filter_map(|r| r.ok())
            .filter_map(|tv| match tv {
                TraversalValue::Vector(v) => hvector_to_source_doc(&v),
                _ => None,
            })
            .collect();
        Ok(vectors)
    }
    #[cfg(not(feature = "helix"))]
    {
        let _ = (path, _dim);
        anyhow::bail!("build with --features helix to use Helix store");
    }
}

#[cfg(feature = "helix")]
fn path_matches_filter(file_path: &str, path_filter: &str) -> bool {
    let p = file_path.replace('\\', "/");
    let f = path_filter.replace('\\', "/");
    p.contains(&f)
}

/// Hybrid vector search (keyword/BM25 not yet implemented). Returns QueryHits.
pub fn helix_hybrid_query(
    path: &Path,
    _query: &str,
    query_embedding: &[f32],
    top_k: usize,
    path_filter: Option<&str>,
) -> Result<Vec<QueryHit>> {
    #[cfg(feature = "helix")]
    {
        let _guard = helix_store_guard();
        use heed3::RoTxn;
        use helix_db::helix_engine::traversal_core::ops::g::G;
        use helix_db::helix_engine::traversal_core::ops::vectors::search::SearchVAdapter;
        use helix_db::helix_engine::traversal_core::traversal_value::TraversalValue;
        use helix_db::helix_engine::vector_core::vector::HVector;

        let storage = open_helix_storage(path)?;
        let txn = storage
            .graph_env
            .read_txn()
            .map_err(|e| anyhow::anyhow!("helix read_txn: {:?}", e))?;
        let arena = bumpalo::Bump::new();

        let query_f64: Vec<f64> = query_embedding.iter().map(|&x| x as f64).collect();
        let limit = top_k.saturating_mul(2).max(10);

        let results: Vec<QueryHit> = G::new(&*storage, &txn, &arena)
            .search_v::<fn(&HVector, &RoTxn) -> bool, _>(&query_f64, limit, CHUNK_LABEL, None)
            .filter_map(|r: Result<_, _>| r.ok())
            .filter_map(|tv| match tv {
                TraversalValue::Vector(v) => {
                    let score = v.distance.map(|d| (1.0 - d / 2.0) as f32).unwrap_or(0.0);
                    let doc = hvector_to_source_doc(&v)?;
                    Some(QueryHit {
                        score,
                        file_path: doc.source_path.clone(),
                        chunk_id: doc.chunk_id.clone(),
                        chunk_index: doc.chunk_index,
                        content: doc.content.clone(),
                        start_offset: Some(doc.start_offset),
                        end_offset: Some(doc.end_offset),
                        source: "helix_vector".to_string(),
                        group_key: Some(doc.source_path),
                        memory: doc.memory.clone(),
                    })
                }
                _ => None,
            })
            .collect();

        let mut out = results;
        if let Some(pf) = path_filter {
            out.retain(|h| path_matches_filter(&h.file_path, pf));
        }
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.file_path.cmp(&b.file_path))
        });
        out.truncate(top_k);
        Ok(out)
    }
    #[cfg(not(feature = "helix"))]
    {
        let _ = (path, _query, query_embedding, top_k, path_filter);
        anyhow::bail!("build with --features helix to use Helix store");
    }
}

const GRAPH_STATS_FILE: &str = "graph_stats.json";

/// Write entity/relationship counts plus chunk count and source paths so `/status` can avoid opening Helix.
pub fn helix_write_graph_stats(
    pack_dir: &Path,
    entity_count: usize,
    relationship_count: usize,
    chunk_count: usize,
    source_paths: &[String],
    index_warnings: &[String],
) -> Result<()> {
    let path = pack_dir.join(GRAPH_STATS_FILE);
    let json = serde_json::json!({
        "entities": entity_count,
        "relationships": relationship_count,
        "chunks": chunk_count,
        "source_paths": source_paths,
        "index_warnings": index_warnings,
    });
    std::fs::write(&path, serde_json::to_string_pretty(&json)?)
        .context("write graph_stats.json")?;
    Ok(())
}

/// When `graph_stats.json` includes `chunks` (written by the indexer), status can use this instead of loading all vectors.
#[cfg(feature = "helix")]
pub fn helix_try_cached_index_status(pack_dir: &Path) -> Option<(usize, Vec<String>, Vec<String>)> {
    let path = pack_dir.join(GRAPH_STATS_FILE);
    let data = std::fs::read_to_string(&path).ok()?;
    let obj: serde_json::Value = serde_json::from_str(&data).ok()?;
    let chunks = obj.get("chunks")?.as_u64()? as usize;
    let paths: Vec<String> = obj
        .get("source_paths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let warnings: Vec<String> = obj
        .get("index_warnings")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Some((chunks, paths, warnings))
}

#[cfg(not(feature = "helix"))]
pub fn helix_try_cached_index_status(
    _pack_dir: &Path,
) -> Option<(usize, Vec<String>, Vec<String>)> {
    None
}

/// Last index warnings from `graph_stats.json` (when cache path is not used).
#[cfg(feature = "helix")]
pub fn helix_read_index_warnings(pack_dir: &Path) -> Vec<String> {
    let path = pack_dir.join(GRAPH_STATS_FILE);
    let Some(data) = std::fs::read_to_string(&path).ok() else {
        return Vec::new();
    };
    let Some(obj) = serde_json::from_str::<serde_json::Value>(&data).ok() else {
        return Vec::new();
    };
    obj.get("index_warnings")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(not(feature = "helix"))]
pub fn helix_read_index_warnings(_pack_dir: &Path) -> Vec<String> {
    Vec::new()
}

#[cfg(feature = "helix")]
pub fn helix_graph_chunk_count(pack_dir: &Path) -> Option<usize> {
    let path = pack_dir.join(GRAPH_STATS_FILE);
    let data = std::fs::read_to_string(&path).ok()?;
    let obj: serde_json::Value = serde_json::from_str(&data).ok()?;
    obj.get("chunks")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
}

#[cfg(not(feature = "helix"))]
pub fn helix_graph_chunk_count(_pack_dir: &Path) -> Option<usize> {
    None
}

#[cfg(feature = "helix")]
pub fn helix_graph_source_paths(pack_dir: &Path) -> Option<Vec<String>> {
    let path = pack_dir.join(GRAPH_STATS_FILE);
    let data = std::fs::read_to_string(&path).ok()?;
    let obj: serde_json::Value = serde_json::from_str(&data).ok()?;
    obj.get("source_paths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
}

#[cfg(not(feature = "helix"))]
pub fn helix_graph_source_paths(_pack_dir: &Path) -> Option<Vec<String>> {
    None
}

/// Read entity and relationship counts from the pack's graph_stats.json. Returns (0, 0) if missing or invalid.
pub fn helix_graph_counts(pack_dir: &Path) -> (usize, usize) {
    let path = pack_dir.join(GRAPH_STATS_FILE);
    let Ok(data) = std::fs::read_to_string(&path) else {
        return (0, 0);
    };
    let Ok(obj) = serde_json::from_str::<serde_json::Value>(&data) else {
        return (0, 0);
    };
    let entities = obj.get("entities").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let relationships = obj
        .get("relationships")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    (entities, relationships)
}

fn entity_ids_path(pack_dir: &Path) -> PathBuf {
    pack_dir.join(ENTITY_IDS_FILE)
}

/// Load entity name -> Helix node id from pack's entity_ids.json. Returns empty map if missing/invalid.
pub fn helix_load_entity_id_map(pack_dir: &Path) -> HashMap<String, u128> {
    let path = entity_ids_path(pack_dir);
    let Ok(data) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    let obj: serde_json::Map<String, serde_json::Value> = match serde_json::from_str(&data) {
        Ok(o) => o,
        Err(_) => return HashMap::new(),
    };
    let mut map = HashMap::new();
    for (name, v) in obj {
        if let Some(s) = v.as_str() {
            if let Ok(id) = u128::from_str_radix(s, 16) {
                map.insert(name, id);
            }
        }
    }
    map
}

/// Save entity name -> node id map to pack's entity_ids.json (ids as hex strings).
pub fn helix_save_entity_id_map(pack_dir: &Path, map: &HashMap<String, u128>) -> Result<()> {
    let path = entity_ids_path(pack_dir);
    let obj: serde_json::Map<String, serde_json::Value> = map
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(format!("{:x}", v))))
        .collect();
    std::fs::write(&path, serde_json::to_string_pretty(&obj)?).context("write entity_ids.json")?;
    Ok(())
}

/// Remove the entity id map file (e.g. after a full rebuild so we start with a fresh map).
pub fn helix_clear_entity_map(pack_dir: &Path) {
    let _ = std::fs::remove_file(entity_ids_path(pack_dir));
}

#[cfg(feature = "helix")]
fn insert_entities_and_edges<'a>(
    storage: &'a helix_db::helix_engine::storage_core::HelixGraphStorage,
    arena: &bumpalo::Bump,
    txn: &mut heed3::RwTxn<'a>,
    entity_map: &mut HashMap<String, u128>,
    entities: &std::collections::HashSet<String>,
    relations: &[GraphRelation],
) -> Result<()> {
    use helix_db::helix_engine::traversal_core::ops::g::G;
    use helix_db::helix_engine::traversal_core::ops::source::add_e::AddEAdapter;
    use helix_db::helix_engine::traversal_core::ops::source::add_n::AddNAdapter;
    use helix_db::helix_engine::traversal_core::traversal_value::TraversalValue;
    use helix_db::protocol::value::Value;
    use helix_db::utils::properties::ImmutablePropertiesMap;

    let all_entities: std::collections::HashSet<String> = entities
        .iter()
        .cloned()
        .chain(
            relations
                .iter()
                .flat_map(|r| [r.source.clone(), r.target.clone()]),
        )
        .collect();

    for name in &all_entities {
        if entity_map.contains_key(name) {
            continue;
        }
        let keys_and_values: Vec<(&str, Value)> = vec![("name", Value::String(name.clone()))];
        let len = keys_and_values.len();
        let items = keys_and_values.into_iter().map(|(k, v)| {
            let k_arena = arena.alloc_str(k);
            (k_arena as &str, v)
        });
        let props = ImmutablePropertiesMap::new(len, items, arena);
        let tv = G::new_mut(storage, arena, txn)
            .add_n(ENTITY_LABEL, Some(props), None)
            .collect_to_obj()
            .map_err(|e| anyhow::anyhow!("helix add_n: {:?}", e))?;
        if let TraversalValue::Node(node) = tv {
            entity_map.insert(name.clone(), node.id);
        }
    }

    let mut seen_rel: HashSet<(String, String, String)> = HashSet::new();
    for rel in relations {
        if !seen_rel.insert((rel.source.clone(), rel.relation.clone(), rel.target.clone())) {
            continue;
        }
        let Some(&from_id) = entity_map.get(&rel.source) else {
            continue;
        };
        let Some(&to_id) = entity_map.get(&rel.target) else {
            continue;
        };
        let label = arena.alloc_str(rel.relation.as_str());
        let edge_res = G::new_mut(storage, arena, txn)
            .add_edge(label, None, from_id, to_id, false, false)
            .collect_to_obj();
        if let Err(e) = edge_res {
            let msg = format!("{e:?}");
            // add_docs appends to an existing graph; edges may already exist from a prior run.
            if msg.contains("KEYEXIST") || msg.contains("MDB_KEYEXIST") {
                continue;
            }
            return Err(anyhow::anyhow!("helix add_edge: {:?}", e));
        }
    }
    Ok(())
}

/// Write entity nodes and relationship edges into the pack's Helix DB. Updates entity_map in place and persists it to pack_dir.
pub fn helix_write_entities_edges(
    path: &Path,
    pack_dir: &Path,
    entity_map: &mut HashMap<String, u128>,
    entities: &std::collections::HashSet<String>,
    relations: &[GraphRelation],
) -> Result<()> {
    #[cfg(feature = "helix")]
    {
        let _guard = helix_store_guard();
        let storage = open_helix_storage(path)?;
        let mut txn = storage
            .graph_env
            .write_txn()
            .map_err(|e| anyhow::anyhow!("helix write_txn: {:?}", e))?;
        let arena = bumpalo::Bump::new();
        insert_entities_and_edges(
            storage.as_ref(),
            &arena,
            &mut txn,
            entity_map,
            entities,
            relations,
        )?;
        txn.commit()
            .map_err(|e| anyhow::anyhow!("helix commit: {:?}", e))?;
        helix_save_entity_id_map(pack_dir, entity_map)?;
        Ok(())
    }
    #[cfg(not(feature = "helix"))]
    {
        let _ = (path, pack_dir, entity_map, entities, relations);
        anyhow::bail!("build with --features helix to use Helix store");
    }
}

#[cfg(all(test, feature = "helix"))]
mod helix_compiler_tests {
    /// Spike: verify HelixQL compiler (helixc) is embeddable — parse a minimal schema in-process.
    #[test]
    fn helixql_parse_minimal_schema() {
        use helix_db::helixc::parser::{HelixParser, write_to_temp_file};

        let content = write_to_temp_file(vec!["N::Chunk { id: String }"]);
        let result = HelixParser::parse_source(&content);
        assert!(result.is_ok(), "parse should succeed: {:?}", result.err());
        let source = result.unwrap();
        assert!(!source.schema.is_empty());
    }
}

/// Integration test: write a few chunks to helix and read them back. Run with:
///   cargo test --features store-helix-only helix_index_and_load -- --nocapture
#[cfg(all(test, feature = "helix"))]
#[test]
fn helix_index_and_load() {
    use crate::types::SourceDoc;
    use chrono::Utc;
    use std::path::PathBuf;

    let dim = 4usize;
    let indexed_at = Utc::now();
    let docs: Vec<SourceDoc> = vec![
        SourceDoc {
            chunk_id: "c1".to_string(),
            source_path: "test://one".to_string(),
            chunk_index: 0,
            start_offset: 0,
            end_offset: 5,
            content: "hello".to_string(),
            content_hash: "h1".to_string(),
            embedding: vec![0.1f32, 0.2, 0.3, 0.4],
            indexed_at,
            memory: crate::types::MemoryMetadata::default(),
        },
        SourceDoc {
            chunk_id: "c2".to_string(),
            source_path: "test://two".to_string(),
            chunk_index: 1,
            start_offset: 0,
            end_offset: 5,
            content: "world".to_string(),
            content_hash: "h2".to_string(),
            embedding: vec![0.5f32, 0.6, 0.7, 0.8],
            indexed_at,
            memory: crate::types::MemoryMetadata::default(),
        },
    ];

    let temp = std::env::temp_dir()
        .join("helix_test")
        .join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).expect("create temp dir");
    let path = PathBuf::from(&temp);

    // Write
    helix_rebuild_chunks(&path, &docs, dim).expect("helix_rebuild_chunks should succeed");

    // Read back
    let loaded = helix_load_all_docs(&path, dim).expect("helix_load_all_docs should succeed");

    assert_eq!(loaded.len(), 2, "should load 2 docs from helix");
    let contents: Vec<&str> = loaded.iter().map(|d| d.content.as_str()).collect();
    assert!(
        contents.contains(&"hello"),
        "loaded docs should contain 'hello'"
    );
    assert!(
        contents.contains(&"world"),
        "loaded docs should contain 'world'"
    );

    let _ = std::fs::remove_dir_all(&temp);
}
