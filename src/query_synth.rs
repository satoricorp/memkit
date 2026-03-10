use anyhow::{Context, Result};

use crate::ontology::OntologyConfig;
use crate::ontology_llama::generate_completion;
use crate::types::QueryResponse;

const MAX_CONTEXT_CHARS: usize = 3000;
const MAX_CHUNKS: usize = 3;
const MAX_ANSWER_TOKENS: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryProvider {
    Llama,
}

impl QueryProvider {
    pub fn label(&self) -> &'static str {
        match self {
            QueryProvider::Llama => "Internal: Llama",
        }
    }
}

pub fn synthesize_answer(query: &str, response: &QueryResponse) -> Result<(String, QueryProvider)> {
    if response.results.is_empty() {
        return Ok((
            "No relevant context found in the memory pack.".to_string(),
            QueryProvider::Llama,
        ));
    }

    let config = OntologyConfig::from_env();

    if !std::path::Path::new(&config.model).exists() {
        anyhow::bail!(
            "Model file not found: {}. Set MEMKIT_ONTOLOGY_MODEL to a GGUF path, or build with `cargo build --features llama-embedded` for in-process inference.",
            config.model
        );
    }

    let prompt = build_prompt(query, response);
    let out = generate_completion(&prompt, &config, Some(MAX_ANSWER_TOKENS))
        .with_context(|| format!("Llama failed (model: {})", config.model))?;

    Ok((truncate_answer(&out), QueryProvider::Llama))
}

fn build_prompt(query: &str, response: &QueryResponse) -> String {
    let mut context = String::with_capacity(MAX_CONTEXT_CHARS + 512);
    for hit in response.results.iter().take(MAX_CHUNKS) {
        let block = format!(
            "{}\n(source: {})\n---\n",
            hit.content.trim(),
            hit.file_path
        );
        if context.len() + block.len() > MAX_CONTEXT_CHARS {
            break;
        }
        context.push_str(&block);
    }

    let inner = format!(
        "Answer in 2-3 short sentences only. Do NOT quote or copy the context. Synthesize a direct answer.\n\nQuestion: {query}\n\nContext:\n---\n{context}\n\nAnswer:"
    );
    format!("<|user|>\n{inner}\n<|assistant|>\n")
}

fn truncate_answer(s: &str) -> String {
    const MAX_CHARS: usize = 400;
    let trimmed = s.trim();
    if trimmed.len() <= MAX_CHARS {
        return trimmed.to_string();
    }
    let cut = trimmed.char_indices().nth(MAX_CHARS).map(|(i, _)| i).unwrap_or(MAX_CHARS);
    let truncated = &trimmed[..cut];
    match truncated.rfind(|c: char| c.is_whitespace() || c == '.') {
        Some(i) => format!("{}...", truncated[..=i].trim()),
        None => format!("{}...", truncated),
    }
}
