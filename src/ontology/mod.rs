// ONTOLOGY MODULE - INTERNAL USE ONLY
//
// Entity/relation extraction for the indexer (Helix graph edges). No CLI or HTTP
// surface for ontology—internal pipeline only.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::types::GraphRelation;
#[cfg(feature = "llama-embedded")]
use crate::ontology_llama::LlamaOntologyProvider;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OntologyEntity {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OntologyRelation {
    pub source: String,
    pub relation: String,
    pub target: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OntologyExtraction {
    pub entities: Vec<String>,
    pub relations: Vec<GraphRelation>,
    pub confidence: f32,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct OntologyCache {
    by_content_hash: HashMap<String, OntologyExtraction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OntologyArtifact {
    pub source_path: String,
    pub provider: String,
    pub model: String,
    pub generated_at: DateTime<Utc>,
    pub chunk_count: usize,
    pub entities: Vec<OntologyEntity>,
    pub relations: Vec<OntologyRelation>,
}

pub trait OntologyProvider {
    fn provider_name(&self) -> &'static str;
    fn model_name(&self) -> String;
    fn extract(&mut self, content: &str, max_entities: usize) -> Result<OntologyExtraction>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OntologyProviderKind {
    Llama,
    Rules,
}

impl OntologyProviderKind {
    fn from_str_value(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "rules" => Self::Rules,
            "llama" => Self::Llama,
            _ => Self::Rules,
        }
    }

    pub fn from_env() -> Self {
        Self::from_str_value(
            &std::env::var("MEMKIT_LLM_PROVIDER")
                .or_else(|_| std::env::var("MEMKIT_ONTOLOGY_PROVIDER"))
                .unwrap_or_else(|_| "rules".to_string()),
        )
    }
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub provider: OntologyProviderKind,
    #[cfg_attr(not(feature = "llama-embedded"), allow(dead_code))]
    pub model: String,
    #[cfg_attr(not(feature = "llama-embedded"), allow(dead_code))]
    pub max_tokens: usize,
    #[cfg_attr(not(feature = "llama-embedded"), allow(dead_code))]
    pub timeout_ms: u64,
    /// Context length (KV cache size). Default 32768 (~1–2 GB extra KV RAM for a 2B model). Set MEMKIT_LLM_N_CTX to override.
    #[cfg(feature = "llama-embedded")]
    pub n_ctx: u32,
}

/// Resolve relative model path to absolute using current_dir() so the path works
/// regardless of process cwd (e.g. server started from another directory).
fn resolve_model_path(model: &str) -> String {
    let p = Path::new(model);
    if p.is_absolute() {
        return model.to_string();
    }
    std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join(p))
        .and_then(|abs| abs.into_os_string().into_string().ok())
        .unwrap_or_else(|| model.to_string())
}

impl LlmConfig {
    pub fn from_env() -> Self {
        let provider = OntologyProviderKind::from_env();
        let model = std::env::var("MEMKIT_LLM_MODEL")
            .or_else(|_| std::env::var("MEMKIT_ONTOLOGY_MODEL"))
            .unwrap_or_else(|_| {
                ".local-runtime/models/qwen2.5-2b-instruct-Q8_0.gguf".to_string()
            });
        let model = resolve_model_path(&model);
        let max_tokens = std::env::var("MEMKIT_LLM_MAX_TOKENS")
            .or_else(|_| std::env::var("MEMKIT_ONTOLOGY_MAX_TOKENS"))
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(512);
        let timeout_ms = std::env::var("MEMKIT_LLM_TIMEOUT_MS")
            .or_else(|_| std::env::var("MEMKIT_ONTOLOGY_TIMEOUT_MS"))
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(20_000);
        #[cfg(feature = "llama-embedded")]
        let n_ctx = std::env::var("MEMKIT_LLM_N_CTX")
            .or_else(|_| std::env::var("MEMKIT_ONTOLOGY_N_CTX"))
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(32768);
        Self {
            provider,
            model,
            max_tokens,
            timeout_ms,
            #[cfg(feature = "llama-embedded")]
            n_ctx,
        }
    }
}

pub struct RuleOntologyProvider;

impl OntologyProvider for RuleOntologyProvider {
    fn provider_name(&self) -> &'static str {
        "rules"
    }

    fn model_name(&self) -> String {
        "heuristic".to_string()
    }

    fn extract(&mut self, content: &str, max_entities: usize) -> Result<OntologyExtraction> {
        let entities = extract_entities(content, max_entities.max(1));
        let relations = infer_relations(&entities);
        Ok(OntologyExtraction {
            entities,
            relations,
            confidence: 0.45,
            provider: "rules".to_string(),
        })
    }
}

pub struct OntologyEngine {
    pack_dir: PathBuf,
    cache_path: PathBuf,
    cache: OntologyCache,
    provider: Box<dyn OntologyProvider + Send>,
}

impl OntologyEngine {
    pub fn new(pack_dir: &Path) -> Result<Self> {
        let config = LlmConfig::from_env();
        let cache_path = pack_dir.join("state").join("ontology_cache.json");
        let cache = if cache_path.exists() {
            let bytes = fs::read(&cache_path).context("failed to read ontology cache")?;
            serde_json::from_slice::<OntologyCache>(&bytes)
                .context("failed to parse ontology cache")?
        } else {
            OntologyCache::default()
        };
        let provider: Box<dyn OntologyProvider + Send> = match config.provider {
            OntologyProviderKind::Rules => Box::new(RuleOntologyProvider),
            OntologyProviderKind::Llama => {
                #[cfg(feature = "llama-embedded")]
                {
                    match LlamaOntologyProvider::new(config.clone()) {
                        Ok(p) => Box::new(p),
                        Err(err) => {
                            crate::term::warn(format!(
                                "warning: failed to initialize llama ontology provider ({}), using rules fallback",
                                err
                            ));
                            Box::new(RuleOntologyProvider)
                        }
                    }
                }
                #[cfg(not(feature = "llama-embedded"))]
                {
                    crate::term::warn(
                        "warning: MEMKIT_LLM_PROVIDER=llama requires building with --features llama-embedded; using rules",
                    );
                    Box::new(RuleOntologyProvider)
                }
            }
        };
        Ok(Self {
            pack_dir: pack_dir.to_path_buf(),
            cache_path,
            cache,
            provider,
        })
    }

    pub fn provider_name(&self) -> &'static str {
        self.provider.provider_name()
    }

    pub fn model_name(&self) -> String {
        self.provider.model_name()
    }

    pub fn extract(
        &mut self,
        content_hash: &str,
        content: &str,
        max_entities: usize,
    ) -> OntologyExtraction {
        if let Some(existing) = self.cache.by_content_hash.get(content_hash) {
            return existing.clone();
        }

        let extraction = self
            .provider
            .extract(content, max_entities)
            .unwrap_or_else(|err| {
                crate::term::warn(format!(
                    "warning: ontology extraction failed ({}), using rules fallback",
                    err
                ));
                let mut fallback = RuleOntologyProvider;
                fallback.extract(content, max_entities).unwrap_or_default()
            });
        self.cache
            .by_content_hash
            .insert(content_hash.to_string(), extraction.clone());
        extraction
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.cache_path.parent() {
            fs::create_dir_all(parent).context("failed to create ontology cache dir")?;
        }
        let bytes = serde_json::to_vec_pretty(&self.cache)?;
        fs::write(&self.cache_path, bytes).context("failed to write ontology cache")?;
        Ok(())
    }

    pub fn artifact_path_for_source(&self, source_path: &str) -> PathBuf {
        let mut h = Sha256::new();
        h.update(source_path.as_bytes());
        let source_hash = format!("{:x}", h.finalize());
        self.pack_dir
            .join("ontology")
            .join(format!("{}.ontology.json", &source_hash[..16]))
    }

    pub fn write_artifact(
        &mut self,
        source_path: &str,
        chunk_contents: &[String],
        chunk_hashes: &[String],
    ) -> Result<PathBuf> {
        let mut entity_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut relation_counts: BTreeMap<(String, String, String), usize> = BTreeMap::new();

        for (idx, content) in chunk_contents.iter().enumerate() {
            let extraction = if let Some(hash) = chunk_hashes.get(idx) {
                if let Some(cached) = self.cache.by_content_hash.get(hash) {
                    cached.clone()
                } else {
                    self.extract(hash, content, 12)
                }
            } else {
                OntologyExtraction {
                    entities: extract_entities(content, 12),
                    relations: infer_relations(&extract_entities(content, 12)),
                    confidence: 0.4,
                    provider: "rules".to_string(),
                }
            };
            for e in extraction.entities {
                *entity_counts.entry(e).or_insert(0) += 1;
            }
            for rel in extraction.relations {
                *relation_counts
                    .entry((rel.source, rel.relation, rel.target))
                    .or_insert(0) += 1;
            }
        }

        let entities = entity_counts
            .into_iter()
            .map(|(name, count)| OntologyEntity { name, count })
            .collect::<Vec<_>>();
        let relations = relation_counts
            .into_iter()
            .map(|((source, relation, target), count)| OntologyRelation {
                source,
                relation,
                target,
                count,
            })
            .collect::<Vec<_>>();

        let artifact = OntologyArtifact {
            source_path: source_path.to_string(),
            provider: self.provider_name().to_string(),
            model: self.model_name(),
            generated_at: Utc::now(),
            chunk_count: chunk_contents.len(),
            entities,
            relations,
        };
        let artifact_path = self.artifact_path_for_source(source_path);
        if let Some(parent) = artifact_path.parent() {
            fs::create_dir_all(parent).context("failed to create ontology artifact dir")?;
        }
        fs::write(&artifact_path, serde_json::to_vec_pretty(&artifact)?)
            .context("failed to write ontology artifact")?;
        Ok(artifact_path)
    }

}

fn extract_entities(content: &str, max_entities: usize) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the", "and", "for", "that", "with", "this", "from", "into", "while", "where", "when",
        "then", "have", "has", "had", "will", "would", "could", "should", "using", "use", "used",
        "into", "about", "your", "you", "they", "their", "there", "were", "been", "are", "was",
        "but", "not", "its", "our", "out", "all", "any", "can", "may", "per", "via", "api", "json",
    ];

    let mut counts: HashMap<String, usize> = HashMap::new();
    for token in content
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(str::trim)
        .filter(|t| t.len() >= 3)
    {
        let lower = token.to_ascii_lowercase();
        if STOPWORDS.contains(&lower.as_str()) || lower.chars().all(|c| c.is_numeric()) {
            continue;
        }
        *counts.entry(lower).or_insert(0) += 1;
    }

    let mut scored = counts.into_iter().collect::<Vec<_>>();
    scored.sort_by(|a, b| {
        b.1.cmp(&a.1).then_with(|| {
            let al = a.0.len();
            let bl = b.0.len();
            bl.cmp(&al)
        })
    });

    scored
        .into_iter()
        .take(max_entities)
        .map(|(name, _)| name)
        .collect()
}

fn infer_relations(entities: &[String]) -> Vec<GraphRelation> {
    let mut relations = Vec::new();
    for pair in entities.windows(2).take(12) {
        relations.push(GraphRelation {
            source: pair[0].clone(),
            relation: "related_to".to_string(),
            target: pair[1].clone(),
        });
    }
    relations
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{OntologyEngine, OntologyProviderKind};

    #[test]
    fn parses_provider_kind() {
        assert_eq!(
            OntologyProviderKind::from_str_value("llama"),
            OntologyProviderKind::Llama
        );
        assert_eq!(
            OntologyProviderKind::from_str_value("rules"),
            OntologyProviderKind::Rules
        );
        assert_eq!(
            OntologyProviderKind::from_str_value("unknown"),
            OntologyProviderKind::Rules
        );
    }

    #[test]
    fn writes_artifact_file() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        let pack_dir = PathBuf::from(format!("/tmp/satori-ontology-test-{}", nonce));
        fs::create_dir_all(pack_dir.join("state")).expect("state dir should exist");

        let mut engine = OntologyEngine::new(&pack_dir).expect("engine should initialize");
        let out = engine
            .write_artifact(
                "/tmp/demo-source",
                &["Rust memory pack chunk".to_string()],
                &["hash1".to_string()],
            )
            .expect("artifact should be written");
        assert!(out.exists());

        let _ = fs::remove_dir_all(pack_dir);
    }
}
