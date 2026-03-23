use anyhow::{Context, Result};

use crate::config::resolve_openai_synthesis_model;
use crate::types::QueryResponse;

const MAX_CONTEXT_CHARS: usize = 8000;
const MAX_CHUNKS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryProvider {
    /// No LLM call (e.g. empty retrieval).
    None,
    OpenAI(String),
}

impl QueryProvider {
    pub fn label(&self) -> String {
        match self {
            QueryProvider::None => "none".to_string(),
            QueryProvider::OpenAI(model) => format!("OpenAI: {}", model),
        }
    }
}

fn synthesis_max_tokens() -> usize {
    std::env::var("MEMKIT_LLM_MAX_TOKENS")
        .or_else(|_| std::env::var("MEMKIT_ONTOLOGY_MAX_TOKENS"))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(512)
}

/// Some chat models (e.g. `gpt-5.*`) return 400 if the body uses `max_tokens`; they require `max_completion_tokens`.
fn chat_completion_uses_max_completion_tokens(model: &str) -> bool {
    model.starts_with("gpt-5") || model.starts_with("o1") || model.starts_with("o3")
}

pub async fn synthesize_answer_async(
    query: &str,
    response: &QueryResponse,
) -> Result<(String, QueryProvider)> {
    if response.results.is_empty() {
        return Ok((
            "No relevant context found in the memory pack.".to_string(),
            QueryProvider::None,
        ));
    }

    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| anyhow::anyhow!(
            "OPENAI_API_KEY is not set. Query synthesis uses OpenAI only; set OPENAI_API_KEY (optional: MEMKIT_OPENAI_MODEL or memkit.json with openai:*)."
        ))?;
    let api_key = api_key.trim();
    if api_key.is_empty() {
        anyhow::bail!(
            "OPENAI_API_KEY is empty. Query synthesis uses OpenAI only; set OPENAI_API_KEY."
        );
    }

    let model = resolve_openai_synthesis_model();
    let prompt_inner = build_prompt_inner(query, response);
    let max_tokens = synthesis_max_tokens();
    let out = openai_completion_async(&prompt_inner, max_tokens, &model, api_key)
        .await
        .with_context(|| format!("OpenAI synthesis failed (model: {})", model))?;

    let answer = if std::env::var("MEMKIT_QUERY_RAW_ANSWER").as_deref() == Ok("1") {
        out.trim().to_string()
    } else {
        truncate_answer(&out)
    };
    Ok((answer, QueryProvider::OpenAI(model)))
}

fn build_prompt_inner(query: &str, response: &QueryResponse) -> String {
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
    format!(
        "Using only the context below, answer the question in 1-2 sentences. If the context contains relevant numbers, amounts, or facts, state them. Only say you cannot determine the answer if the context truly does not contain the information.\n\nQuestion: {query}\n\nContext:\n---\n{context}\n\nReply:"
    )
}

async fn openai_completion_async(
    user_message: &str,
    max_tokens: usize,
    model: &str,
    api_key: &str,
) -> Result<String> {
    let use_max_completion_tokens = chat_completion_uses_max_completion_tokens(model);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("build reqwest client for OpenAI")?;
    let body = if use_max_completion_tokens {
        serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": user_message}],
            "max_completion_tokens": max_tokens
        })
    } else {
        serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": user_message}],
            "max_tokens": max_tokens
        })
    };
    let res = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .context("OpenAI API request failed")?;
    let status = res.status();
    let text = res.text().await.context("read OpenAI response body")?;
    if !status.is_success() {
        anyhow::bail!("OpenAI API error ({}): {}", status, text);
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).context("parse OpenAI JSON response")?;
    let content = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| anyhow::anyhow!("OpenAI response missing choices[0].message.content"))?;
    Ok(content.to_string())
}

/// Strip chat-template tokens that sometimes appear in model output (e.g. |<|user|>|, <|//3//>|).
fn strip_template_tokens(s: &str) -> String {
    let mut out = s.to_string();
    while let Some(start) = out.find("|<|") {
        let rest = &out[start..];
        let end = rest.find("|>|").map(|i| start + i + 3).or_else(|| rest.find(">|").map(|i| start + i + 2));
        if let Some(e) = end {
            out.replace_range(start..e, " ");
        } else {
            break;
        }
    }
    while let Some(start) = out.find("<|") {
        let rest = &out[start..];
        let end = rest.find("|>").map(|i| start + i + 2).or_else(|| rest.find(">|").map(|i| start + i + 2));
        if let Some(e) = end {
            out.replace_range(start..e, " ");
        } else {
            break;
        }
    }
    out = out.replace("|_|", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ").trim().to_string()
}

fn cut_at_next_turn(s: &str) -> &str {
    const MARKERS: &[&str] = &[
        "|Human:",
        "|human:",
        "|ASSISTANT:",
        "|Assistant:",
        "|assistant:",
        "Human:",
        "ASSISTANT:",
        "<|user|>",
        "<|assistant|>",
    ];
    let mut cut = s.len();
    for m in MARKERS {
        if let Some(i) = s.find(m) {
            cut = cut.min(i);
        }
    }
    s[..cut].trim_end()
}

fn truncate_answer(s: &str) -> String {
    let after_turn = cut_at_next_turn(s);
    let mut trimmed = strip_template_tokens(after_turn).trim().to_string();
    if trimmed.is_empty() {
        trimmed = after_turn.trim().to_string();
    }
    if let Some(first) = trimmed.lines().next() {
        let first = first.trim();
        if let Some(rest) = first.strip_prefix(|c: char| c.is_ascii_digit()) {
            let rest = rest.trim_start_matches(". \"").trim_start_matches(". ");
            let rest = rest.strip_suffix('"').unwrap_or(rest).trim();
            if !rest.is_empty() {
                trimmed = rest.to_string();
            }
        }
    }
    trimmed
}
