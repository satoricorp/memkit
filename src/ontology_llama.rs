use anyhow::{Result, anyhow};
use serde::Deserialize;

#[cfg(not(feature = "llama-embedded"))]
use std::path::Path;

use crate::ontology::{LlmConfig, OntologyExtraction, OntologyProvider};
use crate::types::GraphRelation;

#[cfg(feature = "llama-embedded")]
use llama_cpp_2::context::params::LlamaContextParams;
#[cfg(feature = "llama-embedded")]
use llama_cpp_2::llama_backend::LlamaBackend;
#[cfg(feature = "llama-embedded")]
use llama_cpp_2::llama_batch::LlamaBatch;
#[cfg(feature = "llama-embedded")]
use llama_cpp_2::model::params::LlamaModelParams;
#[cfg(feature = "llama-embedded")]
use llama_cpp_2::model::{AddBos, LlamaModel};
#[cfg(feature = "llama-embedded")]
use llama_cpp_2::sampling::LlamaSampler;
#[cfg(feature = "llama-embedded")]
use std::collections::HashMap;
#[cfg(feature = "llama-embedded")]
use std::num::NonZeroU32;
#[cfg(feature = "llama-embedded")]
use std::sync::{Arc, Mutex, Once, OnceLock};
#[cfg(feature = "llama-embedded")]
use std::time::Instant;

#[cfg_attr(not(feature = "llama-embedded"), derive(Debug))]
pub struct LlamaOntologyProvider {
    config: LlmConfig,
    #[cfg(feature = "llama-embedded")]
    model: Arc<LlamaModel>,
}

#[derive(Debug, Deserialize)]
struct LlamaOutput {
    entities: Vec<String>,
    relations: Vec<LlamaRelation>,
    confidence: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct LlamaRelation {
    source: String,
    relation: String,
    target: String,
}

/// Run generic prompt completion using the Llama stack. Used by query synthesis.
/// Reuses MEMKIT_LLM_MODEL, MEMKIT_LLM_MAX_TOKENS, MEMKIT_LLM_TIMEOUT_MS.
/// If `max_tokens_override` is Some, limits output to that many tokens (e.g. 80 for short answers).
pub fn generate_completion(
    prompt: &str,
    config: &LlmConfig,
    max_tokens_override: Option<usize>,
) -> Result<String> {
    let provider = LlamaOntologyProvider::new(config.clone())?;
    provider.run_completion(prompt, max_tokens_override)
}

impl LlamaOntologyProvider {
    pub fn new(config: LlmConfig) -> Result<Self> {
        if config.model.trim().is_empty() {
            return Err(anyhow!(
                "MEMKIT_LLM_MODEL must point to a GGUF model for llama provider"
            ));
        }
        #[cfg(feature = "llama-embedded")]
        {
            let backend = llama_backend()?;
            let model = cached_llama_model(backend, &config.model)?;
            return Ok(Self { config, model });
        }

        #[cfg(not(feature = "llama-embedded"))]
        {
            Ok(Self { config })
        }
    }

    fn run_llama_inference(&self, content: &str, max_entities: usize) -> Result<String> {
        // Keep prompt bounded for predictable local latency.
        let bounded_content = if content.len() > 8000 {
            &content[..8000]
        } else {
            content
        };
        let prompt = format!(
            "Extract ontology as STRICT JSON only with shape {{\"entities\":[string],\"relations\":[{{\"source\":string,\"relation\":string,\"target\":string}}],\"confidence\":number}}. \
Keep at most {max_entities} entities and at most 24 relations. Output JSON only.\nContent:\n{bounded_content}"
        );
        self.run_completion(&prompt, None)
    }

    /// Run generic prompt completion. Used by ontology extraction and query synthesis.
    pub fn run_completion(
        &self,
        prompt: &str,
        max_tokens_override: Option<usize>,
    ) -> Result<String> {
        let max_tokens = max_tokens_override
            .unwrap_or(self.config.max_tokens)
            .max(64);
        #[cfg(feature = "llama-embedded")]
        {
            let backend = llama_backend()?;
            let n_ctx = self.config.n_ctx.max(256).min(32768);
            let mut ctx_params = LlamaContextParams::default()
                .with_n_ctx(Some(
                    NonZeroU32::new(n_ctx).ok_or_else(|| anyhow!("invalid n_ctx"))?,
                ))
                .with_n_batch(n_ctx);
            let threads = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4);
            ctx_params = ctx_params.with_n_threads(threads as i32);
            ctx_params = ctx_params.with_n_threads_batch(threads as i32);

            let mut ctx = self
                .model
                .new_context(backend, ctx_params)
                .map_err(|e| anyhow!("failed to initialize llama context: {}", e))?;

            let mut prompt_tokens = self
                .model
                .str_to_token(&prompt, AddBos::Always)
                .map_err(|e| anyhow!("failed to tokenize prompt: {}", e))?;
            if prompt_tokens.is_empty() {
                return Err(anyhow!("tokenized prompt is empty"));
            }
            let n_ctx = ctx.n_ctx() as usize;
            // Reserve KV cache slots for generation; otherwise decode loop hits NoKvCacheSlot.
            let max_prompt_tokens = n_ctx.saturating_sub(max_tokens);
            let truncated = prompt_tokens.len() > max_prompt_tokens;
            if truncated {
                // Only truncate when prompt exceeds context; keep start (instructions + question + context beginning).
                prompt_tokens = prompt_tokens[..max_prompt_tokens].to_vec();
            }

            let n_batch = ctx.n_batch() as usize;
            let mut batch = LlamaBatch::new(
                usize::max(n_batch.min(prompt_tokens.len()) + max_tokens + 8, 512),
                1,
            );
            // Decode prompt in chunks so we never exceed n_batch (GGML_ASSERT in llama_decode).
            let mut pos = 0i32;
            while pos < prompt_tokens.len() as i32 {
                let chunk_end = ((pos as usize) + n_batch).min(prompt_tokens.len());
                let last_index = (chunk_end - 1) as i32;
                batch.clear();
                for (i, token) in
                    (pos..).zip(prompt_tokens[pos as usize..chunk_end].iter().copied())
                {
                    batch
                        .add(token, i, &[0], i == last_index)
                        .map_err(|e| anyhow!("failed adding prompt token to batch: {}", e))?;
                }
                ctx.decode(&mut batch)
                    .map_err(|e| anyhow!("failed initial llama decode: {}", e))?;
                pos = chunk_end as i32;
            }

            let mut sampler =
                LlamaSampler::chain_simple([LlamaSampler::dist(42), LlamaSampler::greedy()]);
            let mut decoder = encoding_rs::UTF_8.new_decoder();
            let mut out = String::new();
            let start = Instant::now();
            let timeout = self.config.timeout_ms;
            let mut n_cur = prompt_tokens.len() as i32;

            for _ in 0..max_tokens {
                if start.elapsed().as_millis() as u64 > timeout {
                    break;
                }

                let token = sampler.sample(&ctx, batch.n_tokens() - 1);
                sampler.accept(token);
                if self.model.is_eog_token(token) {
                    break;
                }

                let piece = self
                    .model
                    .token_to_piece(token, &mut decoder, true, None)
                    .map_err(|e| anyhow!("failed converting token to text: {}", e))?;
                out.push_str(&piece);
                // Stop as soon as we generate a "next turn" marker (model continuing the chat pattern).
                if out.contains("|Human:")
                    || out.contains("|ASSISTANT:")
                    || out.contains("<|user|>")
                    || out.contains("<|assistant|>")
                {
                    break;
                }

                batch.clear();
                batch
                    .add(token, n_cur, &[0], true)
                    .map_err(|e| anyhow!("failed preparing decode batch: {}", e))?;
                ctx.decode(&mut batch)
                    .map_err(|e| anyhow!("failed llama decode loop: {}", e))?;
                n_cur += 1;
            }

            if out.trim().is_empty() {
                return Err(anyhow!("llama output was empty"));
            }
            Ok(out)
        }

        #[cfg(not(feature = "llama-embedded"))]
        {
            let llama_cli = resolve_llama_cli_path(&self.config.model);
            if !llama_cli.exists()
                && llama_cli.file_name().and_then(|n| n.to_str()) == Some("llama-cli")
            {
                return Err(anyhow!(
                    "llama-cli not found in PATH and no local binary at .local-runtime/llama-cli. \
                    Build with default features for in-process inference: cargo build (includes llama-embedded), \
                    or set MEMKIT_LLAMA_CLI to the full path to llama-cli."
                ));
            }
            let output = match std::process::Command::new(&llama_cli)
                .arg("-m")
                .arg(&self.config.model)
                .arg("-n")
                .arg(max_tokens.to_string())
                .arg("-p")
                .arg(prompt)
                .output()
            {
                Ok(o) => o,
                Err(e) => {
                    return Err(if e.kind() == std::io::ErrorKind::NotFound {
                        anyhow!(
                            "llama-cli not found (tried {}). Set MEMKIT_LLAMA_CLI to the full path, or build with --features llama-embedded for in-process inference.",
                            llama_cli.display()
                        )
                    } else {
                        anyhow!("failed to execute llama-cli fallback: {e}")
                    });
                }
            };
            if !output.status.success() {
                return Err(anyhow!(
                    "llama-cli fallback failed with status {} (enable `llama-embedded` feature for in-process mode)",
                    output.status.code().unwrap_or(-1)
                ));
            }
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            if text.trim().is_empty() {
                return Err(anyhow!("llama-cli fallback produced empty output"));
            }
            Ok(text)
        }
    }
}

#[cfg(feature = "llama-embedded")]
fn cached_llama_model(backend: &LlamaBackend, model_path: &str) -> Result<Arc<LlamaModel>> {
    static MODELS: OnceLock<Mutex<HashMap<String, Arc<LlamaModel>>>> = OnceLock::new();
    let cache = MODELS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(model) = cache
        .lock()
        .map_err(|_| anyhow!("llama model cache lock poisoned"))?
        .get(model_path)
        .cloned()
    {
        return Ok(model);
    }

    let model = Arc::new(
        LlamaModel::load_from_file(backend, model_path, &LlamaModelParams::default())
            .map_err(|e| anyhow!("failed to load llama model {}: {}", model_path, e))?,
    );
    let mut guard = cache
        .lock()
        .map_err(|_| anyhow!("llama model cache lock poisoned"))?;
    Ok(guard
        .entry(model_path.to_string())
        .or_insert_with(|| model.clone())
        .clone())
}

/// Resolve llama-cli binary path when not using llama-embedded.
#[cfg(not(feature = "llama-embedded"))]
fn resolve_llama_cli_path(model_path: &str) -> std::path::PathBuf {
    if let Ok(path) = std::env::var("MEMKIT_LLAMA_CLI") {
        return std::path::PathBuf::from(path);
    }
    // If model is e.g. /repo/.local-runtime/models/foo.gguf, try /repo/.local-runtime/llama-cli
    let model_dir = Path::new(model_path).parent();
    let runtime_dir = model_dir.and_then(Path::parent);
    if let Some(runtime) = runtime_dir {
        let candidate = runtime.join("llama-cli");
        if candidate.exists() {
            return candidate;
        }
        let bin_candidate = runtime.join("bin").join("llama-cli");
        if bin_candidate.exists() {
            return bin_candidate;
        }
    }
    std::path::PathBuf::from("llama-cli")
}

#[cfg(feature = "llama-embedded")]
fn llama_backend() -> Result<&'static LlamaBackend> {
    static BACKEND: OnceLock<LlamaBackend> = OnceLock::new();
    static INIT: Once = Once::new();
    static INIT_ERROR: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    INIT.call_once(|| match LlamaBackend::init() {
        Ok(b) => {
            BACKEND.set(b).ok();
        }
        Err(e) => {
            *INIT_ERROR.lock().unwrap() =
                Some(anyhow!("failed to initialize llama backend: {}", e));
        }
    });
    BACKEND.get().ok_or_else(|| {
        INIT_ERROR
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| anyhow!("unknown init error"))
    })
}

fn parse_llama_json(raw: &str, max_entities: usize) -> Result<OntologyExtraction> {
    let json_start = raw
        .find('{')
        .ok_or_else(|| anyhow!("llama output did not contain JSON object start"))?;
    let json_end = raw
        .rfind('}')
        .ok_or_else(|| anyhow!("llama output did not contain JSON object end"))?;
    let json_text = &raw[json_start..=json_end];
    let parsed: LlamaOutput = serde_json::from_str(json_text)
        .map_err(|e| anyhow!("failed to parse llama ontology JSON: {e}"))?;

    let entities = parsed
        .entities
        .into_iter()
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .take(max_entities.max(1))
        .collect::<Vec<_>>();
    let relations = parsed
        .relations
        .into_iter()
        .map(|r| GraphRelation {
            source: r.source.trim().to_ascii_lowercase(),
            relation: r.relation.trim().to_ascii_lowercase(),
            target: r.target.trim().to_ascii_lowercase(),
        })
        .filter(|r| !r.source.is_empty() && !r.relation.is_empty() && !r.target.is_empty())
        .collect::<Vec<_>>();

    Ok(OntologyExtraction {
        entities,
        relations,
        confidence: parsed.confidence.unwrap_or(0.7).clamp(0.0, 1.0),
        provider: "llama".to_string(),
    })
}

impl OntologyProvider for LlamaOntologyProvider {
    fn provider_name(&self) -> &'static str {
        "llama"
    }

    fn model_name(&self) -> String {
        self.config.model.clone()
    }

    fn extract(&mut self, content: &str, max_entities: usize) -> Result<OntologyExtraction> {
        let raw = self.run_llama_inference(content, max_entities)?;
        parse_llama_json(&raw, max_entities)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_llama_json;

    #[test]
    fn parses_and_normalizes_llama_json() {
        let raw = r#"prefix {"entities":["Rust","Helix","Graph"],"relations":[{"source":"Rust","relation":"uses","target":"Helix"}],"confidence":0.9} suffix"#;
        let out = parse_llama_json(raw, 2).expect("json should parse");
        assert_eq!(out.entities.len(), 2);
        assert_eq!(out.entities[0], "rust");
        assert_eq!(out.relations[0].relation, "uses");
    }
}
