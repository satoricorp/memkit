use anyhow::{Result, anyhow};

use crate::ontology::{LlmConfig, OntologyExtraction, OntologyProvider};

pub struct CandleOntologyProvider {
    _config: LlmConfig,
}

impl CandleOntologyProvider {
    pub fn new(config: LlmConfig) -> Self {
        Self { _config: config }
    }
}

impl OntologyProvider for CandleOntologyProvider {
    fn provider_name(&self) -> &'static str {
        "candle"
    }

    fn model_name(&self) -> String {
        "candle-future".to_string()
    }

    fn extract(&mut self, _content: &str, _max_entities: usize) -> Result<OntologyExtraction> {
        Err(anyhow!(
            "candle ontology provider is not implemented yet; use MEMKIT_LLM_PROVIDER=llama|rules"
        ))
    }
}
