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
    #[serde(default)]
    pub conversation: ConversationConfig,
    #[serde(default)]
    pub graph: GraphConfig,
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
pub struct ConversationConfig {
    pub strategy: String,
    pub extraction_provider: String,
    pub hydrate_evidence: bool,
}

impl Default for ConversationConfig {
    fn default() -> Self {
        let extraction_provider = if std::env::var("OPENAI_API_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .is_some()
        {
            "openai"
        } else {
            "llama"
        };
        Self {
            strategy: "dual_timestamp_memory".to_string(),
            extraction_provider: extraction_provider.to_string(),
            hydrate_evidence: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphConfig {
    #[serde(default)]
    pub enabled: bool,
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
    #[serde(default)]
    pub memory: MemoryMetadata,
}

/// Graph relation (entity -> relation -> target). Used by ontology and optionally by graph store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRelation {
    pub source: String,
    pub relation: String,
    pub target: String,
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
    #[serde(default)]
    pub memory: MemoryMetadata,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_pack_path: Option<String>,
    #[serde(default)]
    pub grouped_results: Vec<QueryGroup>,
    #[serde(default)]
    pub timings_ms: QueryTimings,
    /// Raw hits from vector store (e.g. Helix) before rerank/truncation, for debugging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieval_results: Option<Vec<QueryHit>>,
    #[serde(default)]
    pub notes: Vec<QueryNote>,
    #[serde(default)]
    pub hydrated_evidence: Vec<QueryEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_time: Option<QueryTimeAnalysis>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryGroup {
    pub group_key: String,
    pub score: f32,
    pub hits: Vec<QueryHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMetadata {
    #[serde(default = "default_doc_kind")]
    pub doc_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_start: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_end: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_time_start: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_time_end: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_time_start: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_time_end: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_time_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal_confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_chunk_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_kind: Option<String>,
}

impl Default for MemoryMetadata {
    fn default() -> Self {
        Self {
            doc_kind: default_doc_kind(),
            record_type: None,
            session_id: None,
            session_index: None,
            turn_start: None,
            turn_end: None,
            role: None,
            session_time_start: None,
            session_time_end: None,
            context_time_start: None,
            context_time_end: None,
            context_time_text: None,
            temporal_kind: None,
            temporal_confidence: None,
            evidence_chunk_id: None,
            evidence_content: None,
            extraction_provider: None,
            extraction_model: None,
            relation_kind: None,
            entity_kind: None,
            value_kind: None,
        }
    }
}

fn default_doc_kind() -> String {
    "source_chunk".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryNote {
    pub chunk_id: String,
    pub note: String,
    #[serde(default)]
    pub hydrated_evidence: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal_match: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryEvidence {
    pub chunk_id: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryTimeAnalysis {
    pub focus: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_time_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_time_start: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_time_end: Option<DateTime<Utc>>,
    #[serde(default)]
    pub wants_session_time: bool,
    #[serde(default)]
    pub wants_context_time: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_relation_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_value_kind: Option<String>,
}
