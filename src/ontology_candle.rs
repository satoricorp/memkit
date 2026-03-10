use anyhow::{Result, anyhow};

use crate::ontology::{OntologyConfig, OntologyExtraction, OntologyProvider};

pub struct CandleOntologyProvider {
    _config: OntologyConfig,
}

impl CandleOntologyProvider {
    pub fn new(config: OntologyConfig) -> Self {
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
            "candle ontology provider is not implemented yet; use MEMKIT_ONTOLOGY_PROVIDER=llama|rules"
        ))
    }
}
