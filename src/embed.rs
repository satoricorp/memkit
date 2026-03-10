use anyhow::{Result, anyhow};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

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

pub fn provider_from_name(
    provider: &str,
    model: &str,
    dim: usize,
) -> Result<Box<dyn EmbeddingProvider>> {
    match provider {
        "fastembed" => Ok(Box::new(FastEmbedProvider::try_new(model)?)),
        "hash" => Ok(Box::new(HashEmbeddingProvider::new(dim))),
        _ => Err(anyhow!("unsupported embedding provider: {}", provider)),
    }
}

fn hash_embed(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for token in text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
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
