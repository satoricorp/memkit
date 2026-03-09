use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format_version: String,
    pub pack_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub embedding: EmbeddingConfig,
    pub chunking: ChunkingConfig,
    pub sources: Vec<SourceConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub provider: String,
    pub model: String,
    pub dimension: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkingConfig {
    pub strategy: String,
    pub target_chars: usize,
    pub overlap_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    pub root_path: String,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    pub file_path: String,
    pub content_hash: String,
    pub mtime_unix_ms: i64,
    pub size: u64,
    pub last_chunk_count: usize,
    pub last_indexed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceDoc {
    pub chunk_id: String,
    pub source_path: String,
    pub chunk_index: usize,
    pub start_offset: usize,
    pub end_offset: usize,
    pub content: String,
    pub content_hash: String,
    pub embedding: Vec<f32>,
    pub indexed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IndexStore {
    pub docs: Vec<SourceDoc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryHit {
    pub score: f32,
    pub file_path: String,
    pub chunk_id: String,
    pub chunk_index: usize,
    pub content: String,
    #[serde(default)]
    pub start_offset: Option<usize>,
    #[serde(default)]
    pub end_offset: Option<usize>,
    #[serde(default = "default_query_source")]
    pub source: String,
    #[serde(default)]
    pub group_key: Option<String>,
}

fn default_query_source() -> String {
    "vector".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryTimings {
    pub embed: u128,
    pub retrieval: u128,
    pub rerank: u128,
    pub total: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResponse {
    pub results: Vec<QueryHit>,
    pub mode: String,
    #[serde(default)]
    pub grouped_results: Vec<QueryGroup>,
    #[serde(default)]
    pub timings_ms: QueryTimings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryGroup {
    pub group_key: String,
    pub score: f32,
    pub hits: Vec<QueryHit>,
}
