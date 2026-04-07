use anyhow::{Context, Result, anyhow};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use serde::Deserialize;

pub trait EmbeddingProvider {
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn embed_query(&mut self, query: &str) -> Result<Vec<f32>>;
}

pub struct HashEmbeddingProvider {
    dim: usize,
}

impl HashEmbeddingProvider {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

impl EmbeddingProvider for HashEmbeddingProvider {
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| hash_embed(t, self.dim)).collect())
    }

    fn embed_query(&mut self, query: &str) -> Result<Vec<f32>> {
        Ok(hash_embed(query, self.dim))
    }
}

pub struct FastEmbedProvider {
    model: TextEmbedding,
}

impl FastEmbedProvider {
    pub fn try_new(model_name: &str) -> Result<Self> {
        let model = match model_name {
            "BAAI/bge-small-en-v1.5" => EmbeddingModel::BGESmallENV15,
            "sentence-transformers/all-MiniLM-L6-v2" => EmbeddingModel::AllMiniLML6V2,
            _ => EmbeddingModel::BGESmallENV15,
        };
        let mut opts = InitOptions::new(model).with_show_download_progress(true);
        if let Ok(cache_dir) = std::env::var("MEMKIT_MODEL_CACHE") {
            opts = opts.with_cache_dir(cache_dir.into());
        }
        let mut emb = TextEmbedding::try_new(opts)?;
        let probe = emb.embed(vec!["probe"], None)?;
        if probe.first().is_none() {
            return Err(anyhow!("failed to initialize embedding model"));
        }
        Ok(Self { model: emb })
    }
}

impl EmbeddingProvider for FastEmbedProvider {
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        Ok(self.model.embed(refs, None)?)
    }

    fn embed_query(&mut self, query: &str) -> Result<Vec<f32>> {
        let mut out = self.model.embed(vec![query], None)?;
        out.pop()
            .ok_or_else(|| anyhow!("no embedding generated for query"))
    }
}

pub struct OpenAIEmbeddingProvider {
    model: String,
    dim: usize,
    client: reqwest::blocking::Client,
    api_key: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIEmbeddingResponse {
    data: Vec<OpenAIEmbeddingItem>,
}

#[derive(Debug, Deserialize)]
struct OpenAIEmbeddingItem {
    embedding: Vec<f32>,
}

impl OpenAIEmbeddingProvider {
    fn try_new(model_name: &str, dim: usize) -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .context("OPENAI_API_KEY is required for openai embeddings")?;
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("build reqwest client for OpenAI embeddings")?;
        Ok(Self {
            model: model_name.to_string(),
            dim,
            client,
            api_key: api_key.trim().to_string(),
        })
    }

    fn embed_inputs(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let batch_size = std::env::var("OPENAI_EMBED_BATCH")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(64);
        let mut out = Vec::new();
        for batch in inputs.chunks(batch_size) {
            let response = self.request_embeddings(batch)?;
            if response.data.len() != batch.len() {
                return Err(anyhow!(
                    "OpenAI embeddings response length mismatch: expected {}, got {}",
                    batch.len(),
                    response.data.len()
                ));
            }
            for item in response.data {
                let mut embedding = item.embedding;
                if self.dim > 0 && embedding.len() != self.dim {
                    return Err(anyhow!(
                        "OpenAI embedding dimension mismatch: expected {}, got {}",
                        self.dim,
                        embedding.len()
                    ));
                }
                normalize(&mut embedding);
                out.push(embedding);
            }
        }
        Ok(out)
    }

    fn request_embeddings(&self, inputs: &[String]) -> Result<OpenAIEmbeddingResponse> {
        let mut body = serde_json::json!({
            "model": self.model,
            "input": inputs,
        });
        if self.dim > 0 {
            body["dimensions"] = serde_json::json!(self.dim);
        }
        let response = self
            .client
            .post("https://api.openai.com/v1/embeddings")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .context("OpenAI embeddings request failed")?;
        let status = response.status();
        let text = response
            .text()
            .context("read OpenAI embeddings response body")?;
        if !status.is_success() {
            anyhow::bail!("OpenAI embeddings error ({}): {}", status, text);
        }
        serde_json::from_str::<OpenAIEmbeddingResponse>(&text)
            .context("parse OpenAI embeddings JSON")
    }
}

impl EmbeddingProvider for OpenAIEmbeddingProvider {
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_inputs(texts)
    }

    fn embed_query(&mut self, query: &str) -> Result<Vec<f32>> {
        let mut embedded = self.embed_inputs(&[query.to_string()])?;
        embedded
            .pop()
            .ok_or_else(|| anyhow!("no OpenAI embedding generated for query"))
    }
}

pub fn provider_from_name(
    provider: &str,
    model: &str,
    dim: usize,
) -> Result<Box<dyn EmbeddingProvider>> {
    match provider {
        "fastembed" => Ok(Box::new(FastEmbedProvider::try_new(model)?)),
        "hash" => Ok(Box::new(HashEmbeddingProvider::new(dim))),
        "openai" => Ok(Box::new(OpenAIEmbeddingProvider::try_new(model, dim)?)),
        _ => Err(anyhow!("unsupported embedding provider: {}", provider)),
    }
}

fn hash_embed(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for token in text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|s| !s.is_empty())
    {
        let mut h = 1469598103934665603u64;
        for b in token.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        let idx = (h as usize) % dim;
        v[idx] += 1.0;
    }
    normalize(&mut v);
    v
}

fn normalize(v: &mut [f32]) {
    let mut sum = 0f32;
    for x in v.iter() {
        sum += x * x;
    }
    let n = sum.sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::provider_from_name;

    #[test]
    fn rejects_unknown_embedding_provider() {
        let err = provider_from_name("nope", "model", 384)
            .err()
            .expect("should fail");
        assert!(err.to_string().contains("unsupported embedding provider"));
    }
}
