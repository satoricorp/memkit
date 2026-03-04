use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use arrow_array::cast::AsArray;
use arrow_array::types::Float32Type;
use arrow_array::{
    Int64Array, RecordBatch, RecordBatchIterator, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field, Schema};
use futures_util::TryStreamExt;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lance_index::scalar::FullTextSearchQuery;
use sha2::{Digest, Sha256};

use crate::types::{QueryHit, SourceDoc};

const TABLE_NAME: &str = "chunks";

fn db_dir(pack_dir: &Path) -> PathBuf {
    pack_dir.join("lancedb")
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

pub fn rebuild_tables(pack_dir: &Path, docs: &[SourceDoc], embedding_dim: usize) -> Result<()> {
    if docs.is_empty() {
        return Ok(());
    }
    let db_path = db_dir(pack_dir);
    if db_path.exists() {
        fs::remove_dir_all(&db_path).context("failed to reset lancedb directory")?;
    }
    fs::create_dir_all(&db_path).context("failed to create lancedb directory")?;

    let task = async move {
        let uri = db_path.to_string_lossy().to_string();
        let db = lancedb::connect(&uri)
            .execute()
            .await
            .context("failed to connect lancedb")?;

        let schema = make_schema(embedding_dim);
        let chunk_id = StringArray::from_iter_values(docs.iter().map(|d| d.chunk_id.clone()));
        let source_path = StringArray::from_iter_values(docs.iter().map(|d| d.source_path.clone()));
        let chunk_index = UInt32Array::from_iter_values(docs.iter().map(|d| d.chunk_index as u32));
        let start_offset =
            Int64Array::from_iter_values(docs.iter().map(|d| d.start_offset as i64));
        let end_offset = Int64Array::from_iter_values(docs.iter().map(|d| d.end_offset as i64));
        let content = StringArray::from_iter_values(docs.iter().map(|d| d.content.clone()));
        let content_hash = StringArray::from_iter_values(docs.iter().map(|d| d.content_hash.clone()));
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

fn parse_hits_from_batches(batches: &[RecordBatch], score: f32) -> Result<Vec<QueryHit>> {
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
        for i in 0..batch.num_rows() {
            hits.push(QueryHit {
                score,
                file_path: path_col.value(i).to_string(),
                chunk_id: chunk_col.value(i).to_string(),
                chunk_index: idx_col.value(i) as usize,
                content: content_col.value(i).to_string(),
            });
        }
    }
    Ok(hits)
}

pub fn hybrid_query(pack_dir: &Path, query: &str, query_embedding: &[f32], top_k: usize) -> Result<Vec<QueryHit>> {
    let db_path = db_dir(pack_dir);
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let q = query.to_string();
    let vecq = query_embedding.to_vec();

    let task = async move {
        let uri = db_path.to_string_lossy().to_string();
        let db = lancedb::connect(&uri)
            .execute()
            .await
            .context("failed to connect lancedb")?;
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

    let vec_list = parse_hits_from_batches(&vec_hits, 0.0)?;
    let fts_list = parse_hits_from_batches(&fts_hits, 0.0)?;

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

pub fn ensure_chunk_ids(docs: &mut [SourceDoc]) {
    for d in docs.iter_mut() {
        if d.chunk_id.is_empty() {
            d.chunk_id = to_chunk_id(&d.source_path, &d.content_hash, d.chunk_index);
        }
    }
}
