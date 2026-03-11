use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use arrow_array::cast::AsArray;
use arrow_array::types::Float32Type;
use arrow_array::{Int64Array, RecordBatch, RecordBatchIterator, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use chrono::Utc;
use futures_util::TryStreamExt;
use lance_index::scalar::FullTextSearchQuery;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use sha2::{Digest, Sha256};

use crate::types::{QueryHit, SourceDoc};

const TABLE_NAME: &str = "chunks";

fn db_dir(pack_dir: &Path) -> PathBuf {
    pack_dir.join("lancedb")
}

/// Connect to LanceDB at URI (file path or s3://). Optionally pass storage options for S3.
async fn connect_lancedb(
    uri: &str,
    storage_options: Option<&[(String, String)]>,
) -> std::result::Result<lancedb::Connection, anyhow::Error> {
    let mut builder = lancedb::connect(uri);
    if let Some(opts) = storage_options {
        builder = builder.storage_options(opts.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    builder.execute().await.context("failed to connect lancedb")
}

fn to_chunk_id(file_path: &str, content_hash: &str, chunk_index: usize) -> String {
    let mut h = Sha256::new();
    h.update(file_path.as_bytes());
    h.update(content_hash.as_bytes());
    h.update(chunk_index.to_le_bytes());
    format!("{:x}", h.finalize())[..16].to_string()
}

fn make_schema(embedding_dim: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("chunk_id", DataType::Utf8, false),
        Field::new("source_path", DataType::Utf8, false),
        Field::new("chunk_index", DataType::UInt32, false),
        Field::new("start_offset", DataType::Int64, false),
        Field::new("end_offset", DataType::Int64, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("content_hash", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                embedding_dim as i32,
            ),
            false,
        ),
    ]))
}

/// Rebuild chunks table at the given URI (file or s3://). For local path, caller should clear the dir first.
pub fn rebuild_tables_with_uri(
    uri: &str,
    storage_options: Option<&[(String, String)]>,
    docs: &[SourceDoc],
    embedding_dim: usize,
) -> Result<()> {
    if docs.is_empty() {
        return Ok(());
    }
    let uri = uri.to_string();
    let storage_options: Option<Vec<(String, String)>> = storage_options.map(|o| o.to_vec());
    let docs = docs.to_vec();

    let task = async move {
        let db = connect_lancedb(&uri, storage_options.as_deref()).await?;

        let schema = make_schema(embedding_dim);
        let chunk_id = StringArray::from_iter_values(docs.iter().map(|d| d.chunk_id.clone()));
        let source_path = StringArray::from_iter_values(docs.iter().map(|d| d.source_path.clone()));
        let chunk_index = UInt32Array::from_iter_values(docs.iter().map(|d| d.chunk_index as u32));
        let start_offset = Int64Array::from_iter_values(docs.iter().map(|d| d.start_offset as i64));
        let end_offset = Int64Array::from_iter_values(docs.iter().map(|d| d.end_offset as i64));
        let content = StringArray::from_iter_values(docs.iter().map(|d| d.content.clone()));
        let content_hash =
            StringArray::from_iter_values(docs.iter().map(|d| d.content_hash.clone()));
        let embedding = arrow_array::FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            docs.iter()
                .map(|d| Some(d.embedding.iter().copied().map(Some).collect::<Vec<_>>())),
            embedding_dim as i32,
        );

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(chunk_id),
                Arc::new(source_path),
                Arc::new(chunk_index),
                Arc::new(start_offset),
                Arc::new(end_offset),
                Arc::new(content),
                Arc::new(content_hash),
                Arc::new(embedding),
            ],
        )?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone());
        let tbl = db
            .create_table(TABLE_NAME, Box::new(batches))
            .execute()
            .await
            .context("failed to create chunks table")?;

        if docs.len() >= 256 {
            tbl.create_index(&["embedding"], Index::Auto)
                .execute()
                .await
                .context("failed to create vector index")?;
        }
        tbl.create_index(&["content"], Index::FTS(Default::default()))
            .execute()
            .await
            .context("failed to create fts index")?;

        Ok::<(), anyhow::Error>(())
    };
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(task))?;
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(task)?;
    }
    Ok(())
}

pub fn rebuild_tables(pack_dir: &Path, docs: &[SourceDoc], embedding_dim: usize) -> Result<()> {
    if docs.is_empty() {
        return Ok(());
    }
    let db_path = db_dir(pack_dir);
    if db_path.exists() {
        fs::remove_dir_all(&db_path).context("failed to reset lancedb directory")?;
    }
    fs::create_dir_all(&db_path).context("failed to create lancedb directory")?;
    let uri = db_path.to_string_lossy().to_string();
    rebuild_tables_with_uri(&uri, None, docs, embedding_dim)
}

/// Load all chunks from the LanceDB table at the given URI. Returns empty vec if the table does not exist.
pub fn load_all_docs_with_uri(
    uri: &str,
    storage_options: Option<&[(String, String)]>,
    _embedding_dim: usize,
) -> Result<Vec<SourceDoc>> {
    let uri = uri.to_string();
    let storage_options: Option<Vec<(String, String)>> = storage_options.map(|o| o.to_vec());

    let task = async move {
        let db = connect_lancedb(&uri, storage_options.as_deref()).await?;
        let table = match db.open_table(TABLE_NAME).execute().await {
            Ok(t) => t,
            Err(_) => return Ok::<_, anyhow::Error>(Vec::new()),
        };

        let batches: Vec<RecordBatch> = table
            .query()
            .select(Select::Columns(vec![
                "chunk_id".to_string(),
                "source_path".to_string(),
                "chunk_index".to_string(),
                "start_offset".to_string(),
                "end_offset".to_string(),
                "content".to_string(),
                "content_hash".to_string(),
                "embedding".to_string(),
            ]))
            .execute()
            .await?
            .try_collect()
            .await?;

        let mut docs = Vec::new();
        for batch in &batches {
            let chunk_id = batch
                .column_by_name("chunk_id")
                .ok_or_else(|| anyhow!("missing chunk_id"))?
                .as_string::<i32>();
            let source_path = batch
                .column_by_name("source_path")
                .ok_or_else(|| anyhow!("missing source_path"))?
                .as_string::<i32>();
            let chunk_index = batch
                .column_by_name("chunk_index")
                .ok_or_else(|| anyhow!("missing chunk_index"))?
                .as_primitive::<arrow_array::types::UInt32Type>();
            let start_offset = batch
                .column_by_name("start_offset")
                .ok_or_else(|| anyhow!("missing start_offset"))?
                .as_primitive::<arrow_array::types::Int64Type>();
            let end_offset = batch
                .column_by_name("end_offset")
                .ok_or_else(|| anyhow!("missing end_offset"))?
                .as_primitive::<arrow_array::types::Int64Type>();
            let content = batch
                .column_by_name("content")
                .ok_or_else(|| anyhow!("missing content"))?
                .as_string::<i32>();
            let content_hash = batch
                .column_by_name("content_hash")
                .ok_or_else(|| anyhow!("missing content_hash"))?
                .as_string::<i32>();
            let embedding_col = batch
                .column_by_name("embedding")
                .ok_or_else(|| anyhow!("missing embedding"))?
                .as_fixed_size_list();

            for i in 0..batch.num_rows() {
                let emb = embedding_col.value(i);
                let emb_prim = emb.as_primitive::<Float32Type>();
                let embedding: Vec<f32> = emb_prim.values().to_vec();
                docs.push(SourceDoc {
                    chunk_id: chunk_id.value(i).to_string(),
                    source_path: source_path.value(i).to_string(),
                    chunk_index: chunk_index.value(i) as usize,
                    start_offset: start_offset.value(i) as usize,
                    end_offset: end_offset.value(i) as usize,
                    content: content.value(i).to_string(),
                    content_hash: content_hash.value(i).to_string(),
                    embedding,
                    indexed_at: Utc::now(),
                });
            }
        }
        Ok(docs)
    };

    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(task))
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(task)
    }
}

/// Load all chunks from the LanceDB table (local path). Returns empty vec if the table does not exist.
pub fn load_all_docs(pack_dir: &Path, embedding_dim: usize) -> Result<Vec<SourceDoc>> {
    let db_path = db_dir(pack_dir);
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let uri = db_path.to_string_lossy().to_string();
    load_all_docs_with_uri(&uri, None, embedding_dim)
}

fn parse_hits_from_batches(
    batches: &[RecordBatch],
    score: f32,
    source_name: &str,
) -> Result<Vec<QueryHit>> {
    let mut hits = Vec::new();
    for batch in batches {
        let chunk_col = batch
            .column_by_name("chunk_id")
            .ok_or_else(|| anyhow!("missing chunk_id column"))?
            .as_string::<i32>();
        let path_col = batch
            .column_by_name("source_path")
            .ok_or_else(|| anyhow!("missing source_path column"))?
            .as_string::<i32>();
        let content_col = batch
            .column_by_name("content")
            .ok_or_else(|| anyhow!("missing content column"))?
            .as_string::<i32>();
        let idx_col = batch
            .column_by_name("chunk_index")
            .ok_or_else(|| anyhow!("missing chunk_index column"))?
            .as_primitive::<arrow_array::types::UInt32Type>();
        let start_col = batch
            .column_by_name("start_offset")
            .ok_or_else(|| anyhow!("missing start_offset column"))?
            .as_primitive::<arrow_array::types::Int64Type>();
        let end_col = batch
            .column_by_name("end_offset")
            .ok_or_else(|| anyhow!("missing end_offset column"))?
            .as_primitive::<arrow_array::types::Int64Type>();
        for i in 0..batch.num_rows() {
            hits.push(QueryHit {
                score,
                file_path: path_col.value(i).to_string(),
                chunk_id: chunk_col.value(i).to_string(),
                chunk_index: idx_col.value(i) as usize,
                content: content_col.value(i).to_string(),
                start_offset: Some(start_col.value(i) as usize),
                end_offset: Some(end_col.value(i) as usize),
                source: source_name.to_string(),
                group_key: Some(path_col.value(i).to_string()),
            });
        }
    }
    Ok(hits)
}

fn path_matches_filter(file_path: &str, path_filter: &str) -> bool {
    let p = file_path.replace('\\', "/");
    let f = path_filter.replace('\\', "/");
    p.contains(&f)
}

/// Query using LanceDB at the given URI (file path or s3://). Use storage_options for S3.
pub fn hybrid_query_with_uri(
    uri: &str,
    storage_options: Option<&[(String, String)]>,
    query: &str,
    query_embedding: &[f32],
    top_k: usize,
    path_filter: Option<&str>,
) -> Result<Vec<QueryHit>> {
    let q = query.to_string();
    let vecq = query_embedding.to_vec();
    let path_filter_owned = path_filter.map(String::from);
    let uri = uri.to_string();
    let storage_options: Option<Vec<(String, String)>> = storage_options.map(|o| o.to_vec());

    let task = async move {
        let db = connect_lancedb(&uri, storage_options.as_deref()).await?;
        let table = db
            .open_table(TABLE_NAME)
            .execute()
            .await
            .context("failed to open chunks table")?;

        let vec_batches: Vec<RecordBatch> = table
            .query()
            .nearest_to(vecq)?
            .select(Select::Columns(vec![
                "chunk_id".to_string(),
                "source_path".to_string(),
                "chunk_index".to_string(),
                "content".to_string(),
                "start_offset".to_string(),
                "end_offset".to_string(),
            ]))
            .limit(top_k * 4)
            .execute()
            .await?
            .try_collect()
            .await?;

        let fts_batches: Vec<RecordBatch> = table
            .query()
            .full_text_search(FullTextSearchQuery::new(q))
            .select(Select::Columns(vec![
                "chunk_id".to_string(),
                "source_path".to_string(),
                "chunk_index".to_string(),
                "content".to_string(),
                "start_offset".to_string(),
                "end_offset".to_string(),
            ]))
            .limit(top_k * 4)
            .execute()
            .await?
            .try_collect()
            .await?;

        Ok::<_, anyhow::Error>((vec_batches, fts_batches))
    };
    let (vec_hits, fts_hits) = if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(task))?
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(task)?
    };

    let mut vec_list = parse_hits_from_batches(&vec_hits, 0.0, "lancedb_vector")?;
    let mut fts_list = parse_hits_from_batches(&fts_hits, 0.0, "lancedb_fts")?;
    if let Some(ref pf) = path_filter_owned {
        vec_list.retain(|h| path_matches_filter(&h.file_path, pf));
        fts_list.retain(|h| path_matches_filter(&h.file_path, pf));
    }

    // Reciprocal rank fusion over two ranked lists.
    let mut by_id: HashMap<String, QueryHit> = HashMap::new();
    let k = 60.0f32;
    for (rank, hit) in vec_list.into_iter().enumerate() {
        let s = 1.0f32 / (k + rank as f32 + 1.0);
        by_id
            .entry(hit.chunk_id.clone())
            .and_modify(|e| e.score += s)
            .or_insert(QueryHit { score: s, ..hit });
    }
    for (rank, hit) in fts_list.into_iter().enumerate() {
        let s = 1.0f32 / (k + rank as f32 + 1.0);
        by_id
            .entry(hit.chunk_id.clone())
            .and_modify(|e| e.score += s)
            .or_insert(QueryHit { score: s, ..hit });
    }

    let mut out: Vec<QueryHit> = by_id.into_values().collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    out.truncate(top_k);
    Ok(out)
}

pub fn hybrid_query(
    pack_dir: &Path,
    query: &str,
    query_embedding: &[f32],
    top_k: usize,
    path_filter: Option<&str>,
) -> Result<Vec<QueryHit>> {
    let db_path = db_dir(pack_dir);
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let uri = db_path.to_string_lossy().to_string();
    hybrid_query_with_uri(&uri, None, query, query_embedding, top_k, path_filter)
}

/// Append docs to the chunks table at the given URI. Use storage_options for S3.
pub fn append_docs_with_uri(
    uri: &str,
    storage_options: Option<&[(String, String)]>,
    docs: &[SourceDoc],
    embedding_dim: usize,
) -> Result<()> {
    if docs.is_empty() {
        return Ok(());
    }
    let uri = uri.to_string();
    let storage_options: Option<Vec<(String, String)>> = storage_options.map(|o| o.to_vec());
    let docs = docs.to_vec();

    let task = async move {
        let db = connect_lancedb(&uri, storage_options.as_deref()).await?;

        let table = db
            .open_table(TABLE_NAME)
            .execute()
            .await
            .context("failed to open chunks table")?;

        let schema = make_schema(embedding_dim);
        let chunk_id = StringArray::from_iter_values(docs.iter().map(|d| d.chunk_id.clone()));
        let source_path = StringArray::from_iter_values(docs.iter().map(|d| d.source_path.clone()));
        let chunk_index = UInt32Array::from_iter_values(docs.iter().map(|d| d.chunk_index as u32));
        let start_offset = Int64Array::from_iter_values(docs.iter().map(|d| d.start_offset as i64));
        let end_offset = Int64Array::from_iter_values(docs.iter().map(|d| d.end_offset as i64));
        let content = StringArray::from_iter_values(docs.iter().map(|d| d.content.clone()));
        let content_hash =
            StringArray::from_iter_values(docs.iter().map(|d| d.content_hash.clone()));
        let embedding = arrow_array::FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            docs.iter()
                .map(|d| Some(d.embedding.iter().copied().map(Some).collect::<Vec<_>>())),
            embedding_dim as i32,
        );

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(chunk_id),
                Arc::new(source_path),
                Arc::new(chunk_index),
                Arc::new(start_offset),
                Arc::new(end_offset),
                Arc::new(content),
                Arc::new(content_hash),
                Arc::new(embedding),
            ],
        )?;

        table.add(Box::new(RecordBatchIterator::new(
            vec![Ok(batch)].into_iter(),
            schema,
        )))
        .execute()
        .await
        .context("failed to append to chunks table")?;

        Ok::<_, anyhow::Error>(())
    };
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(task))?;
    } else {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(task)?;
    }
    Ok(())
}

pub fn append_docs(
    pack_dir: &Path,
    docs: &[SourceDoc],
    embedding_dim: usize,
) -> Result<()> {
    let uri = db_dir(pack_dir).to_string_lossy().to_string();
    append_docs_with_uri(&uri, None, docs, embedding_dim)
}

pub fn ensure_chunk_ids(docs: &mut [SourceDoc]) {
    for d in docs.iter_mut() {
        if d.chunk_id.is_empty() {
            d.chunk_id = to_chunk_id(&d.source_path, &d.content_hash, d.chunk_index);
        }
    }
}
