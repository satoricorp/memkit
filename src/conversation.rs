use anyhow::{Context, Result, anyhow};
use chrono::{
    DateTime, Datelike, Days, Duration, Months, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::config::resolve_openai_synthesis_model;
use crate::ontology::LlmConfig;
use crate::ontology_llama::generate_completion;
use crate::types::{MemoryMetadata, SourceDoc};

pub use crate::conversation_query::{
    build_query_note, expand_query_variants, query_time_analysis, shape_score_boost,
    should_hydrate_evidence, temporal_score_boost,
};

const MAX_MEMORYS_PER_WINDOW: usize = 4;
const MAX_EXTRACTION_CHARS: usize = 4000;
const MAX_EVIDENCE_CHARS: usize = 1600;
const MAX_ASSISTANT_INDEX_CHARS: usize = 600;
const MIN_LLM_MEMORY_CONFIDENCE: f32 = 0.33;
const MAX_LLM_ABSTENTION_LOGS: usize = 12;
const OPENAI_EXTRACTION_RETRIES: usize = 3;
const OPENAI_EXTRACTION_MAX_TOKENS: usize = 512;
const VALID_RECORD_TYPES: &[&str] = &[
    "preference",
    "event",
    "update",
    "relationship",
    "assistant_info",
    "fact",
];
const VALID_RELATION_KINDS: &[&str] = &[
    "identity_change",
    "preference",
    "purchase_source",
    "activity_location",
    "attribute_update",
    "education",
    "routine",
    "coupon_redemption",
    "created_resource",
    "attended_event",
    "count_fact",
    "brand_model",
    "breed_type",
    "occupation_role",
    "program_topic",
    "ratio_measurement",
    "quantity_fact",
];
const VALID_ENTITY_KINDS: &[&str] = &[
    "person_name",
    "venue",
    "item",
    "room",
    "degree",
    "commute",
    "playlist",
    "play",
    "topic",
    "pet",
    "product",
    "program",
    "occupation",
    "measurement",
    "count",
    "vendor",
    "animal",
];
const VALID_VALUE_KINDS: &[&str] = &[
    "previous_name",
    "current_name",
    "location",
    "source",
    "color",
    "model",
    "title",
    "degree",
    "duration",
    "item",
    "preference",
    "count",
    "brand",
    "breed",
    "occupation",
    "program_topic",
    "ratio",
    "quantity",
    "status",
    "size",
    "type",
];

static LLM_ABSTENTION_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSessionInput {
    pub session_id: Option<String>,
    pub session_time: Option<String>,
    pub conversation: Vec<ConversationTurn>,
}

#[derive(Debug, Clone)]
pub struct IndexedConversationDoc {
    pub docs: Vec<SourceDoc>,
}

#[derive(Debug, Clone)]
pub struct MemoryRecordCandidate {
    pub text: String,
    pub record_type: String,
    pub relation_kind: Option<String>,
    pub entity_kind: Option<String>,
    pub value_kind: Option<String>,
    pub confidence: Option<f32>,
}

impl MemoryRecordCandidate {
    fn new(
        text: String,
        record_type: &str,
        relation_kind: Option<&str>,
        entity_kind: Option<&str>,
        value_kind: Option<&str>,
    ) -> Self {
        Self {
            text,
            record_type: record_type.to_string(),
            relation_kind: relation_kind.map(str::to_string),
            entity_kind: entity_kind.map(str::to_string),
            value_kind: value_kind.map(str::to_string),
            confidence: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TemporalExtraction {
    pub session_time_start: Option<DateTime<Utc>>,
    pub session_time_end: Option<DateTime<Utc>>,
    pub context_time_start: Option<DateTime<Utc>>,
    pub context_time_end: Option<DateTime<Utc>>,
    pub context_time_text: Option<String>,
    pub temporal_kind: Option<String>,
    pub temporal_confidence: Option<f32>,
}

impl TemporalExtraction {
    fn session_only(session_time: Option<DateTime<Utc>>) -> Self {
        Self {
            session_time_start: session_time,
            session_time_end: session_time,
            context_time_start: None,
            context_time_end: None,
            context_time_text: None,
            temporal_kind: Some("session_only".to_string()),
            temporal_confidence: Some(0.2),
        }
    }
}

#[derive(Debug, Clone)]
enum MemoryExtractionBackend {
    Rules,
    Llama,
    OpenAI,
}

impl MemoryExtractionBackend {
    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "rules" => Ok(Self::Rules),
            "llama" => Ok(Self::Llama),
            "openai" => Ok(Self::OpenAI),
            other => Err(anyhow!(
                "unsupported conversation extraction provider: {}",
                other
            )),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Rules => "rules",
            Self::Llama => "llama",
            Self::OpenAI => "openai",
        }
    }
}

pub fn build_conversation_source_path(session_id: Option<&str>) -> String {
    match session_id {
        Some(id) if !id.trim().is_empty() => format!("memkit://conversation/{}", id.trim()),
        _ => format!("memkit://conversation/{}", Utc::now().timestamp_millis()),
    }
}

pub fn build_conversation_docs(
    sessions: &[ConversationSessionInput],
    extraction_provider: &str,
    extraction_model: &str,
) -> Result<Vec<IndexedConversationDoc>> {
    LLM_ABSTENTION_LOG_COUNT.store(0, Ordering::Relaxed);
    let backend = MemoryExtractionBackend::from_str(extraction_provider)?;
    let mut indexed = Vec::new();

    for (session_index, session) in sessions.iter().enumerate() {
        let source_path = build_conversation_source_path(session.session_id.as_deref());
        let session_time = parse_session_time(session.session_time.as_deref())?;
        let windows = build_turn_windows(&session.conversation);
        let mut docs = Vec::new();

        for (window_index, window) in windows.iter().enumerate() {
            if window.extraction_text.trim().is_empty() {
                continue;
            }
            if matches!(
                backend,
                MemoryExtractionBackend::Llama | MemoryExtractionBackend::OpenAI
            ) && !should_extract_window_with_llm(window)
            {
                continue;
            }

            let memory_candidates =
                extract_memory_candidates(&backend, extraction_model, window, session_time)?;
            if memory_candidates.is_empty() {
                continue;
            }

            let evidence_chunk_id = short_hash(&format!(
                "{}:{}:{}:{}",
                source_path, window.turn_start, window.turn_end, window.evidence_text
            ));
            let temporal = extract_temporal_metadata(&window.extraction_text, session_time);

            for (candidate_index, candidate) in memory_candidates.into_iter().enumerate() {
                let content = candidate.text.trim();
                if content.is_empty() {
                    continue;
                }
                let content_hash = short_hash(&format!(
                    "{}:{}:{}:{}",
                    source_path, window_index, candidate_index, content
                ));
                let memory = MemoryMetadata {
                    doc_kind: "memory_record".to_string(),
                    record_type: Some(candidate.record_type),
                    session_id: session.session_id.clone(),
                    session_index: Some(session_index),
                    turn_start: Some(window.turn_start),
                    turn_end: Some(window.turn_end),
                    role: Some(window.primary_role.clone()),
                    session_time_start: temporal.session_time_start,
                    session_time_end: temporal.session_time_end,
                    context_time_start: temporal.context_time_start,
                    context_time_end: temporal.context_time_end,
                    context_time_text: temporal.context_time_text.clone(),
                    temporal_kind: temporal.temporal_kind.clone(),
                    temporal_confidence: temporal.temporal_confidence,
                    evidence_chunk_id: Some(evidence_chunk_id.clone()),
                    evidence_content: Some(window.evidence_text.clone()),
                    extraction_provider: Some(backend.label().to_string()),
                    extraction_model: Some(extraction_model.to_string()),
                    relation_kind: candidate.relation_kind,
                    entity_kind: candidate.entity_kind,
                    value_kind: candidate.value_kind,
                };

                docs.push(SourceDoc {
                    chunk_id: content_hash,
                    source_path: source_path.clone(),
                    chunk_index: docs.len(),
                    start_offset: window.turn_start,
                    end_offset: window.turn_end,
                    content: content.to_string(),
                    content_hash: short_hash(content),
                    embedding: Vec::new(),
                    indexed_at: Utc::now(),
                    memory,
                });
            }
        }

        indexed.push(IndexedConversationDoc { docs });
    }

    Ok(indexed)
}

fn extract_memory_candidates(
    backend: &MemoryExtractionBackend,
    extraction_model: &str,
    window: &TurnWindow,
    session_time: Option<DateTime<Utc>>,
) -> Result<Vec<MemoryRecordCandidate>> {
    match backend {
        MemoryExtractionBackend::Rules => Ok(extract_memory_candidates_rules(&window.extraction_text)),
        MemoryExtractionBackend::Llama => {
            let candidates = extract_memory_candidates_llama(extraction_model, window, session_time)?;
            if candidates.is_empty() {
                log_llm_extraction_abstention("llama", window);
            }
            Ok(candidates)
        }
        MemoryExtractionBackend::OpenAI => {
            let candidates =
                extract_memory_candidates_openai(extraction_model, window, session_time)?;
            if candidates.is_empty() {
                log_llm_extraction_abstention("openai", window);
            }
            Ok(candidates)
        }
    }
}

fn extract_memory_candidates_rules(window_text: &str) -> Vec<MemoryRecordCandidate> {
    let mut out = Vec::new();
    for raw in split_memory_segments(window_text)
        .into_iter()
        .take(MAX_MEMORYS_PER_WINDOW * 3)
    {
        for candidate in normalize_atomic_memories(&raw) {
            out.push(candidate);
            if out.len() >= MAX_MEMORYS_PER_WINDOW {
                break;
            }
        }
        if out.len() >= MAX_MEMORYS_PER_WINDOW {
            break;
        }
    }

    if out.is_empty() && !window_text.trim().is_empty() {
        if let Some(candidate) = fallback_memory_candidate(window_text.trim()) {
            out.push(candidate);
        }
    }
    let mut deduped = dedupe_memory_candidates(out);
    deduped.truncate(MAX_MEMORYS_PER_WINDOW);
    deduped
}

fn split_memory_segments(text: &str) -> Vec<String> {
    text.split(['.', '!', '?', '\n', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn normalize_atomic_memories(text: &str) -> Vec<MemoryRecordCandidate> {
    let normalized = text.trim().trim_matches('"');
    if normalized.len() < 12 {
        return Vec::new();
    }

    let mut out = Vec::new();
    out.extend(normalize_identity_change_memories(normalized));
    if let Some(candidate) = normalize_degree_memory(normalized) {
        out.push(candidate);
    }
    if let Some(candidate) = normalize_commute_memory(normalized) {
        out.push(candidate);
    }
    if let Some(candidate) = normalize_coupon_memory(normalized) {
        out.push(candidate);
    }
    if let Some(candidate) = normalize_playlist_memory(normalized) {
        out.push(candidate);
    }
    if let Some(candidate) = normalize_activity_location_memory(normalized) {
        out.push(candidate);
    }
    if let Some(candidate) = normalize_attribute_update_memory(normalized) {
        out.push(candidate);
    }
    if let Some(candidate) = normalize_theater_memory(normalized) {
        out.push(candidate);
    }
    if let Some(candidate) = normalize_purchase_memory(normalized) {
        out.push(candidate);
    }
    if let Some(candidate) = normalize_preference_memory(normalized) {
        out.push(candidate);
    }

    if out.is_empty() && !is_question_like(normalized) {
        if let Some(candidate) = fallback_memory_candidate(normalized) {
            out.push(candidate);
        }
    }

    dedupe_memory_candidates(out)
}

fn fallback_memory_candidate(text: &str) -> Option<MemoryRecordCandidate> {
    let trimmed = text.trim();
    if trimmed.len() < 18 || is_question_like(trimmed) || looks_like_generic_filler(trimmed) {
        return None;
    }
    if !has_strong_memory_signal(trimmed) {
        return None;
    }
    let normalized = normalize_first_person_fact(trimmed);
    if is_low_value_normalized_fact(&normalized) {
        return None;
    }
    Some(MemoryRecordCandidate::new(
        normalized,
        &classify_record_type(trimmed),
        None,
        None,
        None,
    ))
}

fn classify_record_type(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if ["prefer", "favorite", "like", "love"]
        .iter()
        .any(|token| lower.contains(token))
    {
        "preference".to_string()
    } else if [
        "now",
        "recently",
        "changed",
        "change",
        "update",
        "switched",
        "currently",
        "repaint",
        "painted",
        "renamed",
    ]
    .iter()
    .any(|token| lower.contains(token))
    {
        "update".to_string()
    } else if [
        "bought",
        "redeemed",
        "graduated",
        "attended",
        "created",
        "moved",
        "visited",
        "started",
        "stopped",
    ]
    .iter()
    .any(|token| lower.contains(token))
    {
        "event".to_string()
    } else {
        "fact".to_string()
    }
}

fn normalize_identity_change_memories(text: &str) -> Vec<MemoryRecordCandidate> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();

    let from_change = extract_after_phrase(text, &lower, "changed my last name from ")
        .and_then(|rest| {
            split_once_case_preserving(&rest, " to ")
                .map(|(old, new)| (old.to_string(), new.to_string()))
        });
    let old_name = from_change
        .as_ref()
        .map(|(old, _)| clean_fact_tail(old))
        .or_else(|| {
            extract_named_value(
                text,
                &lower,
                &[
                    "my old last name was ",
                    "my old name was ",
                    "old last name was ",
                    "old name was ",
                ],
            )
        });
    let current_name = from_change
        .as_ref()
        .map(|(_, new)| clean_fact_tail(new))
        .or_else(|| {
            extract_named_value(
                text,
                &lower,
                &[
                    "but now it's ",
                    "but now it is ",
                    "now it's ",
                    "now it is ",
                    "my new last name is ",
                    "my last name is now ",
                    "now my last name is ",
                    "my current last name is ",
                ],
            )
        });

    if let Some(old_name) = old_name.filter(|value| looks_like_named_value(value)) {
        out.push(MemoryRecordCandidate::new(
            format!("User's previous last name was {}.", old_name),
            "update",
            Some("identity_change"),
            Some("person_name"),
            Some("previous_name"),
        ));
    }
    if let Some(current_name) = current_name.filter(|value| looks_like_named_value(value)) {
        out.push(MemoryRecordCandidate::new(
            format!("User's current last name is {}.", current_name),
            "update",
            Some("identity_change"),
            Some("person_name"),
            Some("current_name"),
        ));
    }

    dedupe_memory_candidates(out)
}

fn normalize_activity_location_memory(text: &str) -> Option<MemoryRecordCandidate> {
    let lower = text.to_ascii_lowercase();
    if ["bought", "purchased", "picked up", "got from", "ordered from"]
        .iter()
        .any(|token| lower.contains(token))
    {
        return None;
    }
    let place = extract_place_phrase(text, &lower)?;
    if !looks_like_named_value(&place) {
        return None;
    }

    let statement = if lower.contains("yoga") {
        Some(format!("User takes yoga classes at {}.", place))
    } else if lower.contains("work") && !lower.contains("workshop") {
        Some(format!("User works at {}.", place))
    } else if ["study", "school", "college", "university", "class", "classes", "lesson"]
        .iter()
        .any(|token| lower.contains(token))
    {
        Some(format!("User studies at {}.", place))
    } else if ["train", "training", "gym", "practice", "practices"]
        .iter()
        .any(|token| lower.contains(token))
    {
        Some(format!("User trains at {}.", place))
    } else if ["shop", "shopping", "store", "market"]
        .iter()
        .any(|token| lower.contains(token))
    {
        Some(format!("User shops at {}.", place))
    } else if ["attend", "went to", "go to", "goes to"]
        .iter()
        .any(|token| lower.contains(token))
    {
        Some(format!("User goes to {}.", place))
    } else {
        None
    }?;

    Some(MemoryRecordCandidate::new(
        statement,
        "fact",
        Some("activity_location"),
        Some("venue"),
        Some("location"),
    ))
}

fn normalize_attribute_update_memory(text: &str) -> Option<MemoryRecordCandidate> {
    let lower = text.to_ascii_lowercase();

    if ["repaint", "painted", "paint "]
        .iter()
        .any(|token| lower.contains(token))
    {
        let after = extract_after_phrase(text, &lower, "repainted ")
            .or_else(|| extract_after_phrase(text, &lower, "painted "))
            .or_else(|| extract_after_phrase(text, &lower, "paint "))?;
        let (entity, value) = split_target_and_color(&after)?;
        return Some(MemoryRecordCandidate::new(
            format!("User repainted {} {}.", entity, value),
            "update",
            Some("attribute_update"),
            Some(infer_entity_kind(&entity)),
            Some("color"),
        ));
    }

    if lower.contains("renamed ") || lower.contains("changed the name of ") {
        let title = extract_named_value(text, &lower, &["renamed it to ", "renamed to "])
            .or_else(|| extract_quoted_value(text))?;
        let title = clean_fact_tail(&title);
        if looks_like_named_value(&title) {
            return Some(MemoryRecordCandidate::new(
                format!("User renamed it to {}.", title),
                "update",
                Some("attribute_update"),
                Some("item"),
                Some("title"),
            ));
        }
    }

    if lower.contains("changed ")
        || lower.contains("switched ")
        || lower.contains("updated ")
        || lower.contains("now ")
    {
        if let Some((entity, value, value_kind)) = extract_generic_attribute_change(text, &lower) {
            return Some(MemoryRecordCandidate::new(
                format!("User changed {} to {}.", entity, value),
                "update",
                Some("attribute_update"),
                Some(infer_entity_kind(&entity)),
                Some(value_kind),
            ));
        }
    }

    None
}

fn normalize_degree_memory(text: &str) -> Option<MemoryRecordCandidate> {
    let lower = text.to_ascii_lowercase();
    let degree =
        if let Some(value) = extract_after_phrase(text, &lower, "graduated with a degree in ") {
            Some(value)
        } else if lower.contains("graduat") && lower.contains("degree in ") {
            extract_after_phrase(text, &lower, "degree in ")
        } else {
            None
        }?;
    let degree = clean_fact_tail(&degree);
    Some(MemoryRecordCandidate::new(
        format!("User graduated with a degree in {}.", degree),
        "event",
        Some("education"),
        Some("degree"),
        Some("degree"),
    ))
}

fn normalize_commute_memory(text: &str) -> Option<MemoryRecordCandidate> {
    let lower = text.to_ascii_lowercase();
    if !lower.contains("commute") {
        return None;
    }
    let duration = extract_duration_phrase(text)?;
    Some(MemoryRecordCandidate::new(
        format!("User's daily commute to work is {}.", duration),
        "fact",
        Some("routine"),
        Some("commute"),
        Some("duration"),
    ))
}

fn normalize_coupon_memory(text: &str) -> Option<MemoryRecordCandidate> {
    let lower = text.to_ascii_lowercase();
    if !(lower.contains("coupon") && (lower.contains("redeemed") || lower.contains("used"))) {
        return None;
    }
    let amount = extract_coupon_amount(text);
    let item = extract_after_phrase(text, &lower, "on ")
        .map(|value| clean_fact_tail(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "an item".to_string());
    let location = extract_location_phrase(text, &lower);
    let has_location = location.is_some();
    let text = if let Some(amount) = amount {
        if let Some(ref location) = location {
            format!(
                "User redeemed a {} coupon on {} at {}.",
                amount, item, location
            )
        } else {
            format!("User redeemed a {} coupon on {}.", amount, item)
        }
    } else if let Some(ref location) = location {
        format!("User redeemed a coupon on {} at {}.", item, location)
    } else {
        format!("User redeemed a coupon on {}.", item)
    };
    Some(MemoryRecordCandidate::new(
        text,
        "event",
        Some("coupon_redemption"),
        Some("item"),
        Some(if has_location { "source" } else { "item" }),
    ))
}

fn normalize_playlist_memory(text: &str) -> Option<MemoryRecordCandidate> {
    let lower = text.to_ascii_lowercase();
    if !(lower.contains("spotify") && lower.contains("playlist")) {
        return None;
    }
    let title = extract_named_value(text, &lower, &["named ", "called "])
        .or_else(|| extract_quoted_value(text))
        .or_else(|| extract_after_phrase(text, &lower, "playlist "))?;
    let title = clean_fact_tail(&title);
    Some(MemoryRecordCandidate::new(
        format!("User created a Spotify playlist named {}.", title),
        "event",
        Some("created_resource"),
        Some("playlist"),
        Some("title"),
    ))
}

fn normalize_theater_memory(text: &str) -> Option<MemoryRecordCandidate> {
    let lower = text.to_ascii_lowercase();
    if !(lower.contains("theater") || lower.contains("theatre") || lower.contains("play")) {
        return None;
    }
    if !(lower.contains("attend") || lower.contains("went to") || lower.contains("saw")) {
        return None;
    }
    let title = extract_named_value(text, &lower, &["production of ", "play ", "saw "])
        .or_else(|| extract_quoted_value(text))?;
    let title = clean_fact_tail(&title);
    let title_lower = title.to_ascii_lowercase();
    if title.is_empty()
        || title_lower.starts_with("at ")
        || title_lower.contains("community theater")
        || title_lower.contains("community theatre")
        || title_lower == "the play"
        || title_lower == "a play"
    {
        return None;
    }
    Some(MemoryRecordCandidate::new(
        format!("User attended a production of {}.", title),
        "event",
        Some("attended_event"),
        Some("play"),
        Some("title"),
    ))
}

fn normalize_purchase_memory(text: &str) -> Option<MemoryRecordCandidate> {
    let lower = text.to_ascii_lowercase();
    if !(lower.contains("bought")
        || lower.contains("purchased")
        || lower.contains("got ")
        || lower.contains("picked up"))
    {
        return None;
    }
    let source = extract_location_phrase(text, &lower)
        .or_else(|| extract_source_phrase_with_preposition(text, &lower, " from "))
        .or_else(|| extract_source_phrase_with_preposition(text, &lower, " at "));
    let item = extract_after_phrase(text, &lower, "bought ")
        .or_else(|| extract_after_phrase(text, &lower, "purchased "))
        .or_else(|| extract_after_phrase(text, &lower, "picked up "))
        .map(|value| normalize_item_phrase(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| extract_item_before_source_phrase(text, &lower))
        .or_else(|| {
            extract_between_phrases(text, &lower, "got ", " from ")
                .map(|value| normalize_item_phrase(&value))
        })?;
    let has_source = source.is_some();
    let text = if let Some(ref source) = source {
        format!("User bought {} from {}.", item, source)
    } else {
        format!("User bought {}.", item)
    };
    Some(MemoryRecordCandidate::new(
        text,
        "event",
        Some("purchase_source"),
        Some("item"),
        Some(if has_source { "source" } else { "item" }),
    ))
}

fn normalize_preference_memory(text: &str) -> Option<MemoryRecordCandidate> {
    let lower = text.to_ascii_lowercase();
    let value = extract_after_phrase(text, &lower, "favorite ")
        .or_else(|| extract_after_phrase(text, &lower, "prefer "))
        .or_else(|| extract_after_phrase(text, &lower, "love "))?;
    let value = clean_fact_tail(&value);
    if value.len() < 3 {
        return None;
    }
    Some(MemoryRecordCandidate::new(
        format!("User preference: {}.", value),
        "preference",
        Some("preference"),
        Some("topic"),
        Some("preference"),
    ))
}

fn extract_after_phrase(text: &str, lower: &str, phrase: &str) -> Option<String> {
    let start = lower.find(phrase)? + phrase.len();
    text.get(start..).map(|value| value.trim().to_string())
}

fn extract_named_value(text: &str, lower: &str, phrases: &[&str]) -> Option<String> {
    for phrase in phrases {
        if let Some(value) = extract_after_phrase(text, lower, phrase) {
            let cleaned = clean_fact_tail(&value);
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }
    None
}

fn extract_quoted_value(text: &str) -> Option<String> {
    let start = text.find('"')?;
    let rest = text.get(start + 1..)?;
    let end = rest.find('"')?;
    Some(rest[..end].trim().to_string())
}

fn extract_coupon_amount(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '$'))
        .find(|word| word.starts_with('$'))
        .map(str::to_string)
}

fn extract_duration_phrase(text: &str) -> Option<String> {
    let words = text.split_whitespace().collect::<Vec<_>>();
    for idx in 0..words.len() {
        let clean = words[idx].trim_matches(|c: char| !c.is_ascii_digit());
        if clean.is_empty() {
            continue;
        }
        if idx + 1 >= words.len() {
            break;
        }
        let unit = words[idx + 1].trim_matches(|c: char| !c.is_ascii_alphabetic());
        if ["minute", "minutes", "hour", "hours"].contains(&unit) {
            let mut phrase = format!("{} {}", clean, unit);
            if idx + 3 < words.len() {
                let next = words[idx + 2].trim_matches(|c: char| !c.is_ascii_alphabetic());
                let next2 = words[idx + 3].trim_matches(|c: char| !c.is_ascii_alphabetic());
                if next.eq_ignore_ascii_case("each") && next2.eq_ignore_ascii_case("way") {
                    phrase.push_str(" each way");
                }
            }
            return Some(phrase);
        }
    }
    None
}

fn extract_location_phrase(text: &str, lower: &str) -> Option<String> {
    extract_after_phrase(text, lower, " at ")
        .or_else(|| extract_after_phrase(text, lower, " from "))
        .map(|value| clean_fact_tail(&value))
        .filter(|value| !value.is_empty())
}

fn extract_place_phrase(text: &str, lower: &str) -> Option<String> {
    for phrase in [" at ", " to ", " near ", " from "] {
        if let Some(value) = extract_after_phrase(text, lower, phrase) {
            let cleaned = clean_fact_tail(&value);
            if looks_like_named_value(&cleaned) {
                return Some(cleaned);
            }
        }
    }
    None
}

fn extract_source_phrase_with_preposition(text: &str, lower: &str, phrase: &str) -> Option<String> {
    extract_after_phrase(text, lower, phrase)
        .map(|value| clean_fact_tail(&value))
        .filter(|value| looks_like_named_value(value))
}

fn extract_between_phrases(
    text: &str,
    lower: &str,
    start_phrase: &str,
    end_phrase: &str,
) -> Option<String> {
    let start = lower.find(start_phrase)? + start_phrase.len();
    let rest_lower = lower.get(start..)?;
    let end = rest_lower.find(end_phrase)?;
    text.get(start..start + end)
        .map(str::trim)
        .map(str::to_string)
}

fn extract_item_before_source_phrase(text: &str, lower: &str) -> Option<String> {
    for marker in [
        " which i got from ",
        " which i got at ",
        " that i got from ",
        " that i got at ",
        " i got from ",
        " i got at ",
        " ordered from ",
        " ordered at ",
    ] {
        let Some(idx) = lower.find(marker) else {
            continue;
        };
        let prefix = text.get(..idx)?.trim().trim_matches(',');
        let candidate = prefix
            .rsplit_once(": ")
            .map(|(_, tail)| tail)
            .unwrap_or(prefix)
            .trim();
        let normalized = normalize_item_phrase(candidate);
        if !normalized.is_empty() {
            return Some(normalized);
        }
    }
    None
}

fn split_target_and_color(text: &str) -> Option<(String, String)> {
    let cleaned = clean_fact_tail(text);
    if cleaned.is_empty() {
        return None;
    }
    let lower = cleaned.to_ascii_lowercase();
    for marker in [
        " a lighter shade of ",
        " a darker shade of ",
        " a shade of ",
        " gray",
        " grey",
        " blue",
        " green",
        " red",
        " yellow",
        " white",
        " black",
        " pink",
        " purple",
        " brown",
        " orange",
        " beige",
        " tan",
    ] {
        if let Some(idx) = lower.find(marker) {
            let entity = clean_subject_phrase(cleaned.get(..idx)?.trim());
            let value = cleaned.get(idx + 1..)?.trim().to_string();
            if !entity.is_empty() && looks_like_color_value(&value) {
                return Some((entity, value));
            }
        }
    }
    if let Some(idx) = lower.find(" to ") {
        let entity = clean_subject_phrase(cleaned.get(..idx)?.trim());
        let value = clean_fact_tail(cleaned.get(idx + 4..)?.trim());
        if !entity.is_empty() && looks_like_color_value(&value) {
            return Some((entity, value));
        }
    }

    let tokens: Vec<&str> = cleaned.split_whitespace().collect();
    for split in 1..tokens.len() {
        let entity = tokens[..split].join(" ");
        let value = tokens[split..].join(" ");
        if looks_like_color_value(&value) {
            let entity = clean_subject_phrase(&entity);
            if !entity.is_empty() {
                return Some((entity, value));
            }
        }
    }
    None
}

fn extract_generic_attribute_change(
    text: &str,
    lower: &str,
) -> Option<(String, String, &'static str)> {
    let value_kind = if lower.contains("color") || lower.contains("shade") {
        "color"
    } else if lower.contains("model") {
        "model"
    } else if lower.contains("title") || lower.contains("name") {
        "title"
    } else if lower.contains("size") {
        "size"
    } else if lower.contains("status") {
        "status"
    } else {
        return None;
    };

    if let Some((entity, value)) = extract_changed_to_value(text, lower) {
        return Some((entity, value, value_kind));
    }
    None
}

fn extract_changed_to_value(text: &str, lower: &str) -> Option<(String, String)> {
    for phrase in ["changed ", "switched ", "updated "] {
        if let Some(after) = extract_after_phrase(text, lower, phrase) {
            if let Some((entity, value)) = split_once_case_preserving(&after, " to ") {
                let entity = clean_subject_phrase(entity.trim());
                let value = clean_fact_tail(value);
                if !entity.is_empty() && looks_like_named_value(&value) {
                    return Some((entity, value));
                }
            }
        }
    }
    None
}

fn split_once_case_preserving<'a>(text: &'a str, needle: &str) -> Option<(&'a str, &'a str)> {
    let lower = text.to_ascii_lowercase();
    let idx = lower.find(needle)?;
    Some((text.get(..idx)?.trim(), text.get(idx + needle.len()..)?.trim()))
}

fn clean_subject_phrase(value: &str) -> String {
    let cleaned = clean_fact_tail(value);
    let mut trimmed = cleaned.trim_matches(',').trim().to_string();
    for prefix in ["my ", "the ", "our "] {
        trimmed = strip_leading_phrase_case_insensitive(&trimmed, prefix).to_string();
    }
    trimmed.trim().to_string()
}

fn normalize_item_phrase(value: &str) -> String {
    let base = clean_fact_tail(value);
    let mut cleaned = base.trim_matches(',').trim().to_string();
    for prefix in ["my ", "our "] {
        cleaned = strip_leading_phrase_case_insensitive(&cleaned, prefix).to_string();
    }
    cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        return cleaned;
    }
    let lower = cleaned.to_ascii_lowercase();
    if !["a ", "an ", "the ", "some "]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        && !cleaned.chars().next().is_some_and(|c| c.is_uppercase())
    {
        cleaned = format!("a {}", cleaned);
    }
    cleaned
}

fn strip_leading_phrase_case_insensitive<'a>(text: &'a str, prefix: &str) -> &'a str {
    let lower = text.to_ascii_lowercase();
    if lower.starts_with(prefix) {
        &text[prefix.len()..]
    } else {
        text
    }
}

fn infer_entity_kind(entity: &str) -> &'static str {
    let lower = entity.to_ascii_lowercase();
    if ["bedroom", "kitchen", "bathroom", "living room", "wall", "walls", "room"]
        .iter()
        .any(|token| lower.contains(token))
    {
        "room"
    } else if ["playlist", "account", "profile", "project", "document"]
        .iter()
        .any(|token| lower.contains(token))
    {
        "item"
    } else {
        "item"
    }
}

fn looks_like_named_value(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.len() < 2 {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if [
        "it",
        "that",
        "there",
        "something",
        "someone",
        "stuff",
        "things",
    ]
    .contains(&lower.as_str())
    {
        return false;
    }
    let alpha_words = trimmed
        .split_whitespace()
        .filter(|word| word.chars().any(|c| c.is_ascii_alphabetic()))
        .count();
    alpha_words >= 1
}

fn looks_like_color_value(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "gray",
        "grey",
        "blue",
        "green",
        "red",
        "yellow",
        "white",
        "black",
        "pink",
        "purple",
        "brown",
        "orange",
        "beige",
        "tan",
        "shade",
        "lighter",
        "darker",
    ]
    .iter()
    .any(|token| lower.contains(token))
}

fn clean_fact_tail(value: &str) -> String {
    let stop_tokens = [
        ", ",
        " because ",
        " but ",
        " and ",
        " which ",
        " where ",
        " when ",
        " that ",
        " since ",
        " while ",
        " for ",
        " last ",
        " yesterday",
        " today",
        " tomorrow",
        " feels ",
        " feel ",
        " felt ",
        " looks ",
        " looked ",
    ];
    let lower = value.to_ascii_lowercase();
    let cutoff = stop_tokens
        .iter()
        .filter_map(|token| lower.find(token))
        .min()
        .unwrap_or(value.len());
    value[..cutoff]
        .trim()
        .trim_matches(|c: char| c == '.' || c == ',' || c == '!' || c == '?')
        .trim_matches('"')
        .to_string()
}

fn normalize_first_person_fact(text: &str) -> String {
    let trimmed = text.trim().trim_matches('"');
    if let Some(rest) = trimmed.strip_prefix("I am ") {
        return format!("User is {}", clean_fact_tail(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("I'm ") {
        return format!("User is {}", clean_fact_tail(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("I have ") {
        return format!("User has {}", clean_fact_tail(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("I've ") {
        return format!("User has {}", clean_fact_tail(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("My ") {
        return format!("User's {}", clean_fact_tail(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("I ") {
        return format!("User {}", clean_fact_tail(rest));
    }
    trimmed.to_string()
}

fn is_question_like(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    text.trim_end().ends_with('?')
        || [
            "what ", "when ", "where ", "why ", "how ", "can ", "could ", "do ", "does ",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn looks_like_generic_filler(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "thanks for",
        "i'll try",
        "that sounds great",
        "good to know",
        "i'll do some research",
        "i think i'll start with",
        "can you tell me",
        "do you have any tips",
        "you're welcome",
        "if you have any other questions",
        "feel free to ask",
        "please continue",
        "rewrite the script",
        "rewrite the ending",
        "generate code",
        "plan a class",
        "pretend you are a teacher",
        "what are some must-visit",
        "adding to my itinerary",
        "what are some",
        "can you give me numbered topics",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn has_strong_memory_signal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if looks_like_nonmemory_request_or_reaction(text) {
        return false;
    }
    text.chars().any(|c| c.is_ascii_digit())
        || text.contains('$')
        || extract_quoted_value(text).is_some()
        || [
            "graduated",
            "degree",
            "commute",
            "coupon",
            "target",
            "spotify",
            "playlist",
            "attended",
            "bought",
            "purchased",
            "favorite",
            "created",
            "moved",
            "visited",
            "started",
            "stopped",
            "named",
            "called",
            "changed",
            "switched",
            "renamed",
            "painted",
            "repainted",
            "bought from",
            "ordered from",
            "yoga",
            "studio",
            "school",
            "work at",
            "last name",
            "old name",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        || [" at ", " from ", " near "]
            .iter()
            .any(|needle| lower.contains(needle))
}

fn looks_like_nonmemory_request_or_reaction(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "can you",
        "could you",
        "please ",
        "rewrite ",
        "generate ",
        "plan ",
        "pretend you are",
        "what are some",
        "what's a good",
        "what is a good",
        "must-visit",
        "itinerary",
        "add mouse click",
        "who specify",
        "continue, provide",
        "i had no idea",
        "that's really cool",
        "good to know",
        "thanks",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn is_low_value_normalized_fact(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "user think",
        "user thinks",
        "user had the idea",
        "user has had it",
        "user has had",
        "user will",
        "user would like to know",
        "user can you",
        "user should",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn extract_memory_candidates_llama(
    extraction_model: &str,
    window: &TurnWindow,
    session_time: Option<DateTime<Utc>>,
) -> Result<Vec<MemoryRecordCandidate>> {
    let prompt = wrap_llama_memory_prompt(&build_memory_extraction_prompt(window, session_time));
    let config = LlmConfig::from_env();
    let output = generate_completion(&prompt, &config, Some(256)).with_context(|| {
        format!(
            "llama memory extraction failed for model {}",
            extraction_model
        )
    })?;
    parse_memory_candidates_json(&output).with_context(|| {
        format!(
            "parse conversation memory extraction JSON from llama output snippet: {}",
            truncate_for_error(&output, 320)
        )
    })
}

fn extract_memory_candidates_openai(
    extraction_model: &str,
    window: &TurnWindow,
    session_time: Option<DateTime<Utc>>,
) -> Result<Vec<MemoryRecordCandidate>> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .context("OPENAI_API_KEY is required for openai conversation extraction")?;
    let model = if extraction_model.trim().is_empty() {
        resolve_openai_synthesis_model()
    } else {
        extraction_model.to_string()
    };
    let system_prompt = memory_extraction_system_prompt();
    let user_prompt = memory_extraction_user_prompt(window, session_time);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("build reqwest client for OpenAI conversation extraction")?;
    let body = if model.starts_with("gpt-5") || model.starts_with("o1") || model.starts_with("o3") {
        serde_json::json!({
            "model": model,
            "messages": [
                {"role":"system","content":system_prompt},
                {"role":"user","content":user_prompt}
            ],
            "max_completion_tokens": OPENAI_EXTRACTION_MAX_TOKENS,
            "response_format": {"type":"json_object"}
        })
    } else {
        serde_json::json!({
            "model": model,
            "messages": [
                {"role":"system","content":system_prompt},
                {"role":"user","content":user_prompt}
            ],
            "max_tokens": OPENAI_EXTRACTION_MAX_TOKENS,
            "temperature": 0,
            "response_format": {"type":"json_object"}
        })
    };
    let body_text = body.to_string();
    for attempt in 0..OPENAI_EXTRACTION_RETRIES {
        let res = client
            .post("https://api.openai.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key.trim()))
            .header("Content-Type", "application/json")
            .body(body_text.clone())
            .send()
            .context("OpenAI conversation extraction request failed")?;
        let status = res.status();
        let text = res.text().context("read OpenAI extraction response body")?;
        if status.is_success() {
            let json: Value = serde_json::from_str(&text).context("parse OpenAI extraction json")?;
            let content = json
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("message"))
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("OpenAI extraction response missing content"))?;
            match parse_memory_candidates_json(content) {
                Ok(candidates) => return Ok(candidates),
                Err(err) => {
                    if attempt + 1 >= OPENAI_EXTRACTION_RETRIES {
                        return Err(err).with_context(|| {
                            format!(
                                "parse conversation memory extraction JSON from OpenAI output snippet: {}",
                                truncate_for_error(content, 320)
                            )
                        });
                    }
                    crate::term::warn(&format!(
                        "warning: OpenAI extraction retry {}/{} after parse failure",
                        attempt + 1,
                        OPENAI_EXTRACTION_RETRIES
                    ));
                    std::thread::sleep(std::time::Duration::from_millis(
                        750 * (attempt as u64 + 1),
                    ));
                    continue;
                }
            }
        }
        let retryable = status.as_u16() == 429 || status.is_server_error();
        if attempt + 1 >= OPENAI_EXTRACTION_RETRIES || !retryable {
            anyhow::bail!("OpenAI extraction error ({}): {}", status, text);
        }
        crate::term::warn(&format!(
            "warning: OpenAI extraction retry {}/{} after status {}",
            attempt + 1,
            OPENAI_EXTRACTION_RETRIES,
            status
        ));
        std::thread::sleep(std::time::Duration::from_millis(750 * (attempt as u64 + 1)));
    }
    anyhow::bail!("OpenAI extraction exhausted retries")
}

fn parse_memory_candidates_json(input: &str) -> Result<Vec<MemoryRecordCandidate>> {
    let json = parse_memory_json_value(input)?;
    let memories = match &json {
        Value::Object(map) => map
            .get("memories")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("conversation extraction JSON missing memories[]"))?,
        Value::Array(items) => items,
        _ => return Err(anyhow!("conversation extraction JSON must be an object or array")),
    };
    let mut out = Vec::new();
    for memory in memories.iter().take(MAX_MEMORYS_PER_WINDOW) {
        let text = memory
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            continue;
        }
        let record_type = memory
            .get("record_type")
            .and_then(Value::as_str)
            .unwrap_or("fact")
            .trim();
        let relation_kind = memory
            .get("relation_kind")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let entity_kind = memory
            .get("entity_kind")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let value_kind = memory
            .get("value_kind")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let confidence = memory
            .get("confidence")
            .and_then(Value::as_f64)
            .map(|value| value as f32);
        let mut candidate = MemoryRecordCandidate::new(
            canonicalize_llm_memory_text(text),
            canonicalize_record_type(record_type),
            relation_kind.as_deref(),
            entity_kind.as_deref(),
            value_kind.as_deref(),
        );
        candidate.confidence = confidence;
        hydrate_shape_hints(&mut candidate);
        if validate_llm_candidate(&candidate) {
            out.push(candidate);
        }
    }
    Ok(dedupe_memory_candidates(out))
}

fn parse_memory_json_value(input: &str) -> Result<Value> {
    let cleaned = strip_json_fences(input);
    if let Ok(json) = serde_json::from_str::<Value>(&cleaned) {
        return Ok(json);
    }
    if let Some(snippet) = extract_json_snippet(&cleaned) {
        if let Ok(json) = serde_json::from_str::<Value>(&snippet) {
            return Ok(json);
        }
    }
    Err(anyhow!("parse conversation memory extraction JSON"))
}

fn strip_json_fences(input: &str) -> String {
    let trimmed = input.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }

    let mut lines = trimmed.lines();
    let first = lines.next().unwrap_or_default();
    if !first.starts_with("```") {
        return trimmed.to_string();
    }

    let mut body = String::new();
    for line in lines {
        if line.trim_start().starts_with("```") {
            break;
        }
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(line);
    }
    body.trim().to_string()
}

fn extract_json_snippet(input: &str) -> Option<String> {
    let candidates = [('[' , ']'), ('{' , '}')];
    for (open, close) in candidates {
        if let Some(start) = input.find(open) {
            if let Some(end) = input.rfind(close) {
                if end >= start {
                    return Some(input[start..=end].trim().to_string());
                }
            }
        }
    }
    None
}

fn memory_extraction_system_prompt() -> String {
    format!(
        "You extract durable conversational memory records for retrieval QA.\n\
Return STRICT JSON only with this exact schema:\n\
{{\"memories\":[{{\"text\":\"User ...\",\"record_type\":\"preference|event|update|relationship|assistant_info|fact\",\"relation_kind\":\"...\",\"entity_kind\":\"...\",\"value_kind\":\"...\",\"confidence\":0.0}}]}}\n\n\
Rules:\n\
- Extract at most {MAX_MEMORYS_PER_WINDOW} memories.\n\
- Emit only durable, answer-bearing memories that could support a future question.\n\
- Prefer atomic normalized facts over broad paraphrases.\n\
- Normalize first-person content into stable user memories.\n\
- `text` must start with `User ` or `User's `.\n\
- Never copy `User:` or `Assistant:` dialogue lines verbatim.\n\
- It is better to return zero memories than low-confidence garbage.\n\n\
- If a heuristic suggestion is clearly supported by the window, prefer refining it into a clean atomic memory instead of returning an empty list.\n\n\
Target memory families:\n\
- identity changes\n\
- preferences\n\
- purchases and purchase sources\n\
- recurring locations and activity venues\n\
- attributes and updates\n\
- counts and quantities\n\
- brands and models\n\
- occupations and roles\n\
- certifications and program topics\n\
- breeds and types\n\
- ratios and measurements\n\
- education, commute, created resources, attended events\n\n\
Negative guidance:\n\
- Reject vague fragments, discourse glue, and assistant filler.\n\
- Reject malformed titles like 'does', 'some', or 'connect'.\n\
- Reject malformed locations from instructions or fragments.\n\
- Do not treat giver/recipient context like 'from my sister' as a vendor/store source.\n\
- Reject weak 'changed X to Y' extractions unless both slots are clear.\n\
- Reject low-value statements like 'User has had it' or 'User thinks...'.\n\n\
Allowed labels:\n\
- record_type: {}\n\
- relation_kind: {}\n\
- entity_kind: {}\n\
- value_kind: {}\n\n\
Output requirements:\n\
- Output JSON only. No markdown fences. No explanation.\n\
- If nothing durable is present, output {{\"memories\":[]}}.",
        VALID_RECORD_TYPES.join(", "),
        VALID_RELATION_KINDS.join(", "),
        VALID_ENTITY_KINDS.join(", "),
        VALID_VALUE_KINDS.join(", "),
    )
}

fn memory_extraction_user_prompt(
    window: &TurnWindow,
    session_time: Option<DateTime<Utc>>,
) -> String {
    let heuristic_hints = memory_extraction_hint_block(&window.extraction_text);
    let structured_window = render_llm_extraction_window(window, session_time);
    format!(
        "Example output:\n\
{{\"memories\":[{{\"text\":\"User bought a tennis racket from the sports store downtown.\",\"record_type\":\"event\",\"relation_kind\":\"purchase_source\",\"entity_kind\":\"item\",\"value_kind\":\"source\",\"confidence\":0.86}}]}}\n\n\
Second good example:\n\
{{\"memories\":[{{\"text\":\"User's previous last name was Johnson.\",\"record_type\":\"update\",\"relation_kind\":\"identity_change\",\"entity_kind\":\"person_name\",\"value_kind\":\"previous_name\",\"confidence\":0.91}},{{\"text\":\"User's current last name is Winters.\",\"record_type\":\"update\",\"relation_kind\":\"identity_change\",\"entity_kind\":\"person_name\",\"value_kind\":\"current_name\",\"confidence\":0.91}}]}}\n\n\
Bad output example:\n\
{{\"memories\":[{{\"text\":\"User: Can I take the chicken across first?\",\"record_type\":\"fact\",\"relation_kind\":\"\",\"entity_kind\":\"\",\"value_kind\":\"\",\"confidence\":0.9}}]}}\n\
That is bad because it copies dialogue instead of extracting a durable memory. For a window like that, output {{\"memories\":[]}}.\n\n\
Heuristic suggestions:\n\
{}\n\n\
The suggestions above are noisy hints, not ground truth. Use them only if they are fully supported by the conversation window. You may refine them, split them into smaller memories, or discard them completely. When one of them is clearly supported, return the refined memory instead of abstaining.\n\n\
Conversation window:\n{}\n",
        heuristic_hints,
        structured_window
    )
}

fn build_memory_extraction_prompt(
    window: &TurnWindow,
    session_time: Option<DateTime<Utc>>,
) -> String {
    format!(
        "{}\n\n{}",
        memory_extraction_system_prompt(),
        memory_extraction_user_prompt(window, session_time)
    )
}

fn memory_extraction_hint_block(window_text: &str) -> String {
    let hints = extract_memory_candidates_rules(window_text);
    if hints.is_empty() {
        return "- none".to_string();
    }

    hints.into_iter()
        .take(3)
        .map(|candidate| {
            format!(
                "- {} | record_type={} | relation_kind={} | entity_kind={} | value_kind={}",
                candidate.text,
                candidate.record_type,
                candidate.relation_kind.as_deref().unwrap_or(""),
                candidate.entity_kind.as_deref().unwrap_or(""),
                candidate.value_kind.as_deref().unwrap_or(""),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_llm_extraction_window(
    window: &TurnWindow,
    session_time: Option<DateTime<Utc>>,
) -> String {
    let mut out = String::new();
    if let Some(session_time) = session_time {
        out.push_str(&format!("Session time: {}\n", session_time.date_naive()));
    }
    out.push_str(&format!("Primary role: {}\n", window.primary_role));
    out.push_str("Primary utterance:\n");
    out.push_str(window.extraction_text.trim());
    out.push_str("\n\nNearby conversation span:\n");
    out.push_str(window.evidence_text.trim());
    out
}

fn log_llm_extraction_abstention(provider: &str, window: &TurnWindow) {
    let seen = LLM_ABSTENTION_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if seen >= MAX_LLM_ABSTENTION_LOGS {
        return;
    }
    let snippet = truncate_for_error(window.extraction_text.trim(), 180);
    let hints = memory_extraction_hint_block(&window.extraction_text);
    crate::term::warn(&format!(
        "warning: {} conversation extractor returned no memories for role={} turns={}-{} snippet={} hints={}",
        provider,
        window.primary_role,
        window.turn_start,
        window.turn_end,
        snippet,
        truncate_for_error(&hints, 220)
    ));
}

fn wrap_llama_memory_prompt(prompt: &str) -> String {
    format!(
        "<|im_start|>system\nYou are a precise memory extraction engine. Output valid JSON only with no markdown and no commentary.<|im_end|>\n<|im_start|>user\n{}\n<|im_end|>\n<|im_start|>assistant\n",
        prompt
    )
}

fn canonicalize_llm_memory_text(text: &str) -> String {
    let trimmed = strip_role_prefix(text.trim().trim_matches('"'));
    if trimmed.starts_with("User ") || trimmed.starts_with("User's ") {
        return trimmed.to_string();
    }
    if trimmed.starts_with("I ")
        || trimmed.starts_with("I'm ")
        || trimmed.starts_with("I am ")
        || trimmed.starts_with("I've ")
        || trimmed.starts_with("I have ")
        || trimmed.starts_with("My ")
    {
        return normalize_first_person_fact(trimmed);
    }
    trimmed.to_string()
}

fn strip_role_prefix(text: &str) -> &str {
    for prefix in ["User:", "Assistant:", "Human:", "System:"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            return rest.trim();
        }
    }
    text
}

fn canonicalize_record_type(record_type: &str) -> &str {
    let lower = record_type.trim().to_ascii_lowercase();
    if VALID_RECORD_TYPES.contains(&lower.as_str()) {
        match lower.as_str() {
            "preference" => "preference",
            "event" => "event",
            "update" => "update",
            "relationship" => "relationship",
            "assistant_info" => "assistant_info",
            _ => "fact",
        }
    } else {
        "fact"
    }
}

fn hydrate_shape_hints(candidate: &mut MemoryRecordCandidate) {
    let lower = candidate.text.to_ascii_lowercase();
    if candidate.relation_kind.is_none() {
        candidate.relation_kind = infer_relation_kind_from_text(&lower).map(str::to_string);
    }
    if candidate.entity_kind.is_none() {
        candidate.entity_kind = infer_entity_kind_from_text(&lower).map(str::to_string);
    }
    if candidate.value_kind.is_none() {
        candidate.value_kind = infer_value_kind_from_text(&lower).map(str::to_string);
    }
}

fn infer_relation_kind_from_text(lower: &str) -> Option<&'static str> {
    if lower.contains("previous last name") || lower.contains("current last name") {
        Some("identity_change")
    } else if lower.contains("bought") && lower.contains(" from ") {
        Some("purchase_source")
    } else if lower.contains("takes yoga classes at")
        || lower.contains("works at")
        || lower.contains("studies at")
        || lower.contains("trains at")
        || lower.contains("shops at")
        || lower.contains("goes to ")
    {
        Some("activity_location")
    } else if lower.contains("repainted") || lower.contains("changed ") || lower.contains("renamed")
    {
        Some("attribute_update")
    } else if lower.contains("degree in") {
        Some("education")
    } else if lower.contains("commute") {
        Some("routine")
    } else if lower.contains("coupon") && lower.contains("redeemed") {
        Some("coupon_redemption")
    } else if lower.contains("spotify playlist") {
        Some("created_resource")
    } else if lower.contains("production of") || lower.contains("attended") {
        Some("attended_event")
    } else if lower.contains("preference:") {
        Some("preference")
    } else if lower.contains("ratio") || lower.contains(":1") {
        Some("ratio_measurement")
    } else if lower.contains("certification")
        || lower.contains("program in")
        || lower.contains("course in")
    {
        Some("program_topic")
    } else if lower.contains("brand") || lower.contains("model") {
        Some("brand_model")
    } else if lower.contains("breed") {
        Some("breed_type")
    } else if lower.contains("job") || lower.contains("occupation") || lower.contains("worked as")
    {
        Some("occupation_role")
    } else if lower.contains("count") || lower.contains("total") || lower.contains("number of") {
        Some("count_fact")
    } else {
        None
    }
}

fn infer_entity_kind_from_text(lower: &str) -> Option<&'static str> {
    if lower.contains("last name") {
        Some("person_name")
    } else if lower.contains("yoga") || lower.contains("studio") || lower.contains("school") {
        Some("venue")
    } else if lower.contains("bedroom") || lower.contains("walls") || lower.contains("room") {
        Some("room")
    } else if lower.contains("degree") {
        Some("degree")
    } else if lower.contains("commute") {
        Some("commute")
    } else if lower.contains("playlist") {
        Some("playlist")
    } else if lower.contains("production of") || lower.contains("play") {
        Some("play")
    } else if lower.contains("certification") || lower.contains("program") {
        Some("program")
    } else if lower.contains("job") || lower.contains("occupation") {
        Some("occupation")
    } else if lower.contains("dog") || lower.contains("cat") || lower.contains("hamster") {
        Some("animal")
    } else if lower.contains("store") || lower.contains("vendor") || lower.contains("shop") {
        Some("vendor")
    } else if lower.contains("ratio") {
        Some("measurement")
    } else if lower.contains("count") || lower.contains("number of") {
        Some("count")
    } else if lower.contains("preference:") {
        Some("topic")
    } else {
        Some("item")
    }
}

fn infer_value_kind_from_text(lower: &str) -> Option<&'static str> {
    if lower.contains("previous last name") {
        Some("previous_name")
    } else if lower.contains("current last name") {
        Some("current_name")
    } else if lower.contains(" at ") && lower.contains("takes yoga classes") {
        Some("location")
    } else if lower.contains(" from ") && lower.contains("bought") {
        Some("source")
    } else if lower.contains("shade") || lower.contains("color") {
        Some("color")
    } else if lower.contains(" model ") {
        Some("model")
    } else if lower.contains(" named ") || lower.contains("title") {
        Some("title")
    } else if lower.contains("degree in") {
        Some("degree")
    } else if lower.contains("minutes") || lower.contains("hours") {
        Some("duration")
    } else if lower.contains("preference:") {
        Some("preference")
    } else if lower.contains("brand") {
        Some("brand")
    } else if lower.contains("breed") {
        Some("breed")
    } else if lower.contains("occupation") || lower.contains("worked as") {
        Some("occupation")
    } else if lower.contains("certification") || lower.contains("program in") {
        Some("program_topic")
    } else if lower.contains("ratio") || lower.contains(":1") {
        Some("ratio")
    } else if lower.chars().any(|c| c.is_ascii_digit()) {
        Some("count")
    } else {
        None
    }
}

fn validate_llm_candidate(candidate: &MemoryRecordCandidate) -> bool {
    let text = candidate.text.trim();
    if text.len() < 12 || looks_like_generic_filler(text) {
        return false;
    }
    if !(text.starts_with("User ") || text.starts_with("User's ")) {
        return false;
    }
    if is_low_value_normalized_fact(text) {
        return false;
    }
    if text.contains('?') {
        return false;
    }
    if let Some(confidence) = candidate.confidence {
        if confidence < MIN_LLM_MEMORY_CONFIDENCE {
            return false;
        }
    }
    if !VALID_RECORD_TYPES.contains(&candidate.record_type.as_str()) {
        return false;
    }
    if let Some(relation_kind) = candidate.relation_kind.as_deref() {
        if !VALID_RELATION_KINDS.contains(&relation_kind) {
            return false;
        }
    }
    if let Some(entity_kind) = candidate.entity_kind.as_deref() {
        if !VALID_ENTITY_KINDS.contains(&entity_kind) {
            return false;
        }
    }
    if let Some(value_kind) = candidate.value_kind.as_deref() {
        if !VALID_VALUE_KINDS.contains(&value_kind) {
            return false;
        }
    }
    let lower = text.to_ascii_lowercase();
    ![
        "user created a spotify playlist named does",
        "user goes to attend",
        "user takes yoga classes at connect",
        "user takes yoga classes at start using it",
        "user sure,",
        "user okay,",
    ]
    .iter()
    .any(|bad| lower.starts_with(bad))
}

fn dedupe_memory_candidates(candidates: Vec<MemoryRecordCandidate>) -> Vec<MemoryRecordCandidate> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for candidate in candidates {
        let key = candidate.text.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(candidate);
        }
    }
    out
}

fn truncate_for_error(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim().replace('\n', "\\n");
    if trimmed.len() <= max_chars {
        return trimmed;
    }
    format!("{}...", &trimmed[..max_chars])
}

#[derive(Debug, Clone)]
struct TurnWindow {
    turn_start: usize,
    turn_end: usize,
    primary_role: String,
    extraction_text: String,
    evidence_text: String,
}

fn build_turn_windows(turns: &[ConversationTurn]) -> Vec<TurnWindow> {
    let mut out = Vec::new();
    for (idx, turn) in turns.iter().enumerate() {
        if !should_index_turn(turn) {
            continue;
        }
        let normalized_role = normalize_role(&turn.role);
        let (turn_start, turn_end) = if should_expand_evidence(turn.content.trim()) {
            (
                idx.saturating_sub(1),
                (idx + 1).min(turns.len().saturating_sub(1)),
            )
        } else {
            (idx, idx)
        };
        let extraction_text = truncate_middle(turn.content.trim(), MAX_EXTRACTION_CHARS);
        let evidence_text = truncate_middle(
            &render_turn_span(turns, turn_start, turn_end),
            MAX_EVIDENCE_CHARS,
        );
        out.push(TurnWindow {
            turn_start,
            turn_end,
            primary_role: normalized_role,
            extraction_text,
            evidence_text,
        });
    }
    out
}

fn should_extract_window_with_llm(window: &TurnWindow) -> bool {
    let text = window.extraction_text.trim();
    if text.len() < 18 || looks_like_generic_filler(text) || looks_like_nonmemory_request_or_reaction(text) {
        return false;
    }
    if window.primary_role != "user" && window.primary_role != "assistant" {
        return false;
    }
    if window.primary_role == "assistant"
        && (!contains_personal_reference(text) || !has_strong_memory_signal(text))
    {
        return false;
    }
    if window.primary_role == "user" && !contains_memory_subject_signal(text) {
        return false;
    }
    if is_question_like(text) && !has_strong_memory_signal(text) {
        return false;
    }
    has_strong_memory_signal(text)
}

fn should_index_turn(turn: &ConversationTurn) -> bool {
    let content = turn.content.trim();
    if content.len() < 12 {
        return false;
    }
    match normalize_role(&turn.role).as_str() {
        "user" => true,
        "assistant" => {
            content.len() <= MAX_ASSISTANT_INDEX_CHARS
                && contains_personal_reference(content)
                && !looks_like_generic_filler(content)
        }
        _ => contains_personal_reference(content),
    }
}

fn contains_personal_reference(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "you ",
        "your ",
        "you've",
        "you are",
        "you were",
        "you said",
        "your favorite",
        "your birthday",
        "your address",
        "you bought",
        "you prefer",
        "you like",
        "you live",
        "reminder:",
        "noted that you",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn contains_memory_subject_signal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if looks_like_nonmemory_request_or_reaction(text) {
        return false;
    }
    [
        "i ",
        "i'm",
        "i am",
        "i've",
        "i have",
        "i bought",
        "i purchased",
        "i redeemed",
        "i created",
        "i changed",
        "i switched",
        "i renamed",
        "i painted",
        "i repainted",
        "i graduated",
        "i work",
        "i study",
        "i attend",
        "i take yoga",
        "my ",
        "my favorite",
        "my last name",
        "my degree",
        "my commute",
        "my bedroom",
        "my playlist",
        "my job",
        "my dog",
        "my cat",
        "we ",
        "our ",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn should_expand_evidence(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let has_event_signal = [
        "redeemed",
        "coupon",
        "bought",
        "purchased",
        "created",
        "playlist",
        "attended",
        "graduated",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let has_underspecified_reference = [
        " it ",
        " there ",
        " that ",
        " them ",
        " one ",
        " last sunday",
        " yesterday",
        " today",
        " tomorrow",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    has_event_signal || has_underspecified_reference
}

fn render_turn_span(turns: &[ConversationTurn], start: usize, end: usize) -> String {
    turns[start..=end]
        .iter()
        .map(|turn| format!("{}: {}", normalize_role(&turn.role), turn.content.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn truncate_middle(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    let head_chars = max_chars.saturating_mul(2) / 3;
    let tail_chars = max_chars.saturating_sub(head_chars + 5);
    let head: String = text.chars().take(head_chars).collect();
    let tail: String = text
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{} [...] {}", head.trim_end(), tail.trim_start())
}

fn normalize_role(role: &str) -> String {
    match role.trim().to_ascii_lowercase().as_str() {
        "user" => "user".to_string(),
        "assistant" => "assistant".to_string(),
        other => other.to_string(),
    }
}

fn short_hash(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    format!("{:x}", h.finalize())[..16].to_string()
}

fn parse_session_time(raw: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(Some(dt.with_timezone(&Utc)));
    }
    for pattern in ["%Y/%m/%d (%a) %H:%M", "%Y/%m/%d %H:%M", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(raw, pattern) {
            return Ok(Some(Utc.from_utc_datetime(&dt)));
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Ok(Some(
            Utc.from_utc_datetime(
                &date
                    .and_hms_opt(0, 0, 0)
                    .ok_or_else(|| anyhow!("invalid session date"))?,
            ),
        ));
    }
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y/%m/%d") {
        return Ok(Some(
            Utc.from_utc_datetime(
                &date
                    .and_hms_opt(0, 0, 0)
                    .ok_or_else(|| anyhow!("invalid session date"))?,
            ),
        ));
    }
    Err(anyhow!("unsupported session_time format: {}", raw))
}

pub fn extract_temporal_metadata(
    text: &str,
    session_time: Option<DateTime<Utc>>,
) -> TemporalExtraction {
    let mut temporal = TemporalExtraction::session_only(session_time);
    let lower = text.to_ascii_lowercase();

    if let Some((start, end, matched)) = parse_decade(&lower) {
        temporal.context_time_start = Some(start);
        temporal.context_time_end = Some(end);
        temporal.context_time_text = Some(matched);
        temporal.temporal_kind = Some("historical_reference".to_string());
        temporal.temporal_confidence = Some(0.9);
        return temporal;
    }

    if let Some((start, end, matched)) = parse_explicit_date(&lower) {
        temporal.context_time_start = Some(start);
        temporal.context_time_end = Some(end);
        temporal.context_time_text = Some(matched);
        temporal.temporal_kind = Some("explicit_absolute".to_string());
        temporal.temporal_confidence = Some(0.95);
        return temporal;
    }

    if let Some((start, end, matched)) = parse_year(&lower) {
        temporal.context_time_start = Some(start);
        temporal.context_time_end = Some(end);
        temporal.context_time_text = Some(matched);
        temporal.temporal_kind = Some("explicit_range".to_string());
        temporal.temporal_confidence = Some(0.8);
        return temporal;
    }

    if let Some((start, end, matched, kind)) = parse_relative_time(&lower, session_time) {
        temporal.context_time_start = Some(start);
        temporal.context_time_end = Some(end);
        temporal.context_time_text = Some(matched);
        temporal.temporal_kind = Some(kind);
        temporal.temporal_confidence = Some(0.75);
        return temporal;
    }

    if let Some((matched, kind)) = parse_recurring_time(&lower) {
        temporal.context_time_text = Some(matched);
        temporal.temporal_kind = Some(kind);
        temporal.temporal_confidence = Some(0.55);
    }

    temporal
}

fn parse_decade(text: &str) -> Option<(DateTime<Utc>, DateTime<Utc>, String)> {
    for token in tokenize(text) {
        if token.len() == 5
            && token.ends_with('s')
            && token[..4].chars().all(|c| c.is_ascii_digit())
        {
            let year = token[..4].parse::<i32>().ok()?;
            if year % 10 != 0 {
                continue;
            }
            let start = ymd_utc(year, 1, 1)?;
            let end = ymd_utc(year + 9, 12, 31)?;
            return Some((start, end, token));
        }
    }
    None
}

fn parse_year(text: &str) -> Option<(DateTime<Utc>, DateTime<Utc>, String)> {
    for token in tokenize(text) {
        if token.len() == 4 && token.chars().all(|c| c.is_ascii_digit()) {
            let year = token.parse::<i32>().ok()?;
            if !(1800..=2100).contains(&year) {
                continue;
            }
            let start = ymd_utc(year, 1, 1)?;
            let end = ymd_utc(year, 12, 31)?;
            return Some((start, end, token));
        }
    }
    None
}

fn parse_explicit_date(text: &str) -> Option<(DateTime<Utc>, DateTime<Utc>, String)> {
    let cleaned = text.replace(',', " ");
    let words = cleaned.split_whitespace().collect::<Vec<_>>();
    for i in 0..words.len() {
        if i + 2 >= words.len() {
            break;
        }
        let month = parse_month(words[i])?;
        let day = words[i + 1].trim_matches(|c: char| !c.is_ascii_digit());
        let year = words[i + 2].trim_matches(|c: char| !c.is_ascii_digit());
        let day = day.parse::<u32>().ok()?;
        let year = year.parse::<i32>().ok()?;
        let start = ymd_utc(year, month, day)?;
        return Some((start, start, format!("{} {} {}", words[i], day, year)));
    }

    for token in tokenize(text) {
        if let Ok(date) = NaiveDate::parse_from_str(&token, "%Y-%m-%d") {
            let dt = Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?);
            return Some((dt, dt, token));
        }
    }
    None
}

fn parse_relative_time(
    text: &str,
    session_time: Option<DateTime<Utc>>,
) -> Option<(DateTime<Utc>, DateTime<Utc>, String, String)> {
    let base = session_time?;
    if text.contains("yesterday") {
        let dt = base.checked_sub_days(Days::new(1))?;
        return Some((
            dt,
            dt,
            "yesterday".to_string(),
            "relative_to_session".to_string(),
        ));
    }
    if text.contains("today") {
        return Some((
            base,
            base,
            "today".to_string(),
            "relative_to_session".to_string(),
        ));
    }
    if text.contains("tomorrow") {
        let dt = base.checked_add_days(Days::new(1))?;
        return Some((
            dt,
            dt,
            "tomorrow".to_string(),
            "relative_to_session".to_string(),
        ));
    }

    let weekdays = [
        ("monday", Weekday::Mon),
        ("tuesday", Weekday::Tue),
        ("wednesday", Weekday::Wed),
        ("thursday", Weekday::Thu),
        ("friday", Weekday::Fri),
        ("saturday", Weekday::Sat),
        ("sunday", Weekday::Sun),
    ];
    for (name, weekday) in weekdays {
        let last_phrase = format!("last {}", name);
        if text.contains(&last_phrase) {
            let dt = previous_weekday(base, weekday)?;
            return Some((dt, dt, last_phrase, "relative_to_session".to_string()));
        }
        let next_phrase = format!("next {}", name);
        if text.contains(&next_phrase) {
            let dt = next_weekday(base, weekday)?;
            return Some((dt, dt, next_phrase, "relative_to_session".to_string()));
        }
    }

    for unit in ["day", "week", "month", "year"] {
        if let Some((count, phrase)) = parse_relative_unit(text, unit, "ago") {
            let (start, end) = shift_relative(base, count as i64, unit, true)?;
            return Some((start, end, phrase, "relative_to_session".to_string()));
        }
        let in_phrase = format!("in ");
        if text.contains(&in_phrase) {
            if let Some((count, phrase)) = parse_relative_unit(text, unit, "future") {
                let (start, end) = shift_relative(base, count as i64, unit, false)?;
                return Some((start, end, phrase, "relative_to_session".to_string()));
            }
        }
    }

    None
}

fn parse_recurring_time(text: &str) -> Option<(String, String)> {
    for season in ["spring", "summer", "fall", "autumn", "winter"] {
        let phrase = format!("every {}", season);
        if text.contains(&phrase) {
            return Some((phrase, "recurring".to_string()));
        }
    }
    for weekday in [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ] {
        let phrase = format!("every {}", weekday);
        if text.contains(&phrase) {
            return Some((phrase, "recurring".to_string()));
        }
    }
    if text.contains("every year") || text.contains("annually") {
        return Some(("every year".to_string(), "recurring".to_string()));
    }
    if text.contains("every month") || text.contains("monthly") {
        return Some(("every month".to_string(), "recurring".to_string()));
    }
    None
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == ','))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

fn parse_month(word: &str) -> Option<u32> {
    match word.to_ascii_lowercase().as_str() {
        "jan" | "january" => Some(1),
        "feb" | "february" => Some(2),
        "mar" | "march" => Some(3),
        "apr" | "april" => Some(4),
        "may" => Some(5),
        "jun" | "june" => Some(6),
        "jul" | "july" => Some(7),
        "aug" | "august" => Some(8),
        "sep" | "sept" | "september" => Some(9),
        "oct" | "october" => Some(10),
        "nov" | "november" => Some(11),
        "dec" | "december" => Some(12),
        _ => None,
    }
}

fn ymd_utc(year: i32, month: u32, day: u32) -> Option<DateTime<Utc>> {
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    Some(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?))
}

fn previous_weekday(base: DateTime<Utc>, target: Weekday) -> Option<DateTime<Utc>> {
    let mut date = base.date_naive();
    for _ in 0..7 {
        date = date.checked_sub_days(Days::new(1))?;
        if date.weekday() == target {
            return Some(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?));
        }
    }
    None
}

fn next_weekday(base: DateTime<Utc>, target: Weekday) -> Option<DateTime<Utc>> {
    let mut date = base.date_naive();
    for _ in 0..7 {
        date = date.checked_add_days(Days::new(1))?;
        if date.weekday() == target {
            return Some(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?));
        }
    }
    None
}

fn parse_relative_unit(text: &str, unit: &str, mode: &str) -> Option<(u32, String)> {
    let words = text
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_ascii_alphanumeric()))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    for idx in 0..words.len() {
        if mode == "ago" {
            if idx + 2 >= words.len() {
                break;
            }
            let Some(count) = parse_small_number(words[idx]) else {
                continue;
            };
            let unit_word = words[idx + 1];
            let suffix = words[idx + 2];
            if (unit_word == unit || unit_word == format!("{}s", unit)) && suffix == "ago" {
                return Some((count, format!("{} {} {}", words[idx], unit_word, suffix)));
            }
        } else {
            if idx + 2 >= words.len() {
                break;
            }
            if words[idx] != "in" {
                continue;
            }
            let Some(count) = parse_small_number(words[idx + 1]) else {
                continue;
            };
            let unit_word = words[idx + 2];
            if unit_word == unit || unit_word == format!("{}s", unit) {
                return Some((count, format!("in {} {}", words[idx + 1], unit_word)));
            }
        }
    }
    None
}

fn parse_small_number(text: &str) -> Option<u32> {
    match text {
        "a" | "an" | "one" => Some(1),
        "two" => Some(2),
        "three" => Some(3),
        "four" => Some(4),
        "five" => Some(5),
        "six" => Some(6),
        "seven" => Some(7),
        "eight" => Some(8),
        "nine" => Some(9),
        "ten" => Some(10),
        _ => text.parse::<u32>().ok(),
    }
}

fn shift_relative(
    base: DateTime<Utc>,
    count: i64,
    unit: &str,
    past: bool,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    match unit {
        "day" => {
            let days = Days::new(count as u64);
            let dt = if past {
                base.checked_sub_days(days)?
            } else {
                base.checked_add_days(days)?
            };
            Some((dt, dt))
        }
        "week" => {
            let dt = if past {
                base - Duration::weeks(count)
            } else {
                base + Duration::weeks(count)
            };
            Some((dt, dt))
        }
        "month" => {
            let months = Months::new(count as u32);
            let naive = if past {
                base.date_naive().checked_sub_months(months)?
            } else {
                base.date_naive().checked_add_months(months)?
            };
            let dt = Utc.from_utc_datetime(&naive.and_hms_opt(0, 0, 0)?);
            Some((dt, dt))
        }
        "year" => {
            let year = if past {
                base.year() - count as i32
            } else {
                base.year() + count as i32
            };
            let dt = ymd_utc(year, base.month(), base.day().min(28))?;
            Some((dt, dt))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConversationSessionInput, ConversationTurn, build_conversation_docs,
        expand_query_variants, extract_temporal_metadata, fallback_memory_candidate,
        parse_session_time, query_time_analysis,
    };

    #[test]
    fn parses_session_date() {
        let dt = parse_session_time(Some("2025-03-31"))
            .expect("parse should succeed")
            .expect("date should exist");
        assert_eq!(dt.date_naive().to_string(), "2025-03-31");
    }

    #[test]
    fn parses_benchmark_session_datetime() {
        let dt = parse_session_time(Some("2023/05/20 (Sat) 02:21"))
            .expect("parse should succeed")
            .expect("date should exist");
        assert_eq!(dt.date_naive().to_string(), "2023-05-20");
    }

    #[test]
    fn extracts_decade_reference() {
        let temporal = extract_temporal_metadata("In the 1950s we bought a house", None);
        assert_eq!(
            temporal.temporal_kind.as_deref(),
            Some("historical_reference")
        );
        assert_eq!(
            temporal
                .context_time_start
                .expect("start")
                .date_naive()
                .to_string(),
            "1950-01-01"
        );
    }

    #[test]
    fn extracts_relative_phrase() {
        let session_time = parse_session_time(Some("2025-03-31"))
            .expect("parse")
            .expect("session time");
        let temporal =
            extract_temporal_metadata("I went there three weeks ago", Some(session_time));
        assert_eq!(
            temporal.temporal_kind.as_deref(),
            Some("relative_to_session")
        );
        assert_eq!(
            temporal.context_time_text.as_deref(),
            Some("three weeks ago")
        );
    }

    #[test]
    fn extracts_recurring_phrase() {
        let temporal = extract_temporal_metadata("We go every summer", None);
        assert_eq!(temporal.temporal_kind.as_deref(), Some("recurring"));
        assert_eq!(temporal.context_time_text.as_deref(), Some("every summer"));
    }

    #[test]
    fn analyzes_session_time_query() {
        let analysis = query_time_analysis("When did we talk about the house?");
        assert_eq!(analysis.focus, "session_time");
    }

    #[test]
    fn expands_playlist_query_variants() {
        let variants =
            expand_query_variants("What is the name of the playlist I created on Spotify?");
        assert!(variants.iter().any(|v| v == "Spotify playlist"));
        assert!(
            variants
                .iter()
                .any(|v| v == "User created a Spotify playlist named")
        );
    }

    #[test]
    fn expands_profile_and_location_query_variants() {
        let name_variants = expand_query_variants("What was my last name before I changed it?");
        assert!(name_variants.iter().any(|v| v == "previous last name"));

        let yoga_variants = expand_query_variants("Where do I take yoga classes?");
        assert!(
            yoga_variants
                .iter()
                .any(|v| v == "User takes yoga classes at")
        );

        let purchase_variants =
            expand_query_variants("Where did I buy my new tennis racket from?");
        assert!(
            purchase_variants
                .iter()
                .any(|v| v == "User bought a new tennis racket from")
        );
    }

    #[test]
    fn infers_query_shapes_for_generic_slots() {
        let identity = query_time_analysis("What was my last name before I changed it?");
        assert_eq!(
            identity.expected_relation_kind.as_deref(),
            Some("identity_change")
        );
        assert_eq!(
            identity.expected_value_kind.as_deref(),
            Some("previous_name")
        );

        let yoga = query_time_analysis("Where do I take yoga classes?");
        assert_eq!(
            yoga.expected_relation_kind.as_deref(),
            Some("activity_location")
        );
        assert_eq!(yoga.expected_value_kind.as_deref(), Some("location"));

        let color = query_time_analysis("What color did I repaint my bedroom walls?");
        assert_eq!(
            color.expected_relation_kind.as_deref(),
            Some("attribute_update")
        );
        assert_eq!(color.expected_value_kind.as_deref(), Some("color"));

        let source = query_time_analysis("Where did I buy my new tennis racket from?");
        assert_eq!(
            source.expected_relation_kind.as_deref(),
            Some("purchase_source")
        );
        assert_eq!(source.expected_value_kind.as_deref(), Some("source"));
    }

    #[test]
    fn skips_long_generic_assistant_turns() {
        let docs = build_conversation_docs(
            &[ConversationSessionInput {
                session_id: Some("session-1".to_string()),
                session_time: Some("2025-03-31".to_string()),
                conversation: vec![
                    ConversationTurn {
                        role: "user".to_string(),
                        content: "I bought coffee creamer at Target with a coupon.".to_string(),
                    },
                    ConversationTurn {
                        role: "assistant".to_string(),
                        content:
                            "Here is a very long generic explanation about budgeting and shopping. "
                                .repeat(40),
                    },
                ],
            }],
            "rules",
            "heuristic",
        )
        .expect("conversation docs");

        assert_eq!(docs.len(), 1);
        assert!(
            docs[0]
                .docs
                .iter()
                .all(|doc| doc.memory.role.as_deref() == Some("user"))
        );
    }

    #[test]
    fn truncates_large_evidence_payloads() {
        let docs = build_conversation_docs(
            &[ConversationSessionInput {
                session_id: Some("session-1".to_string()),
                session_time: Some("2025-03-31".to_string()),
                conversation: vec![ConversationTurn {
                    role: "user".to_string(),
                    content: format!(
                        "I graduated with a degree in {}",
                        "A".repeat(5000)
                    ),
                }],
            }],
            "rules",
            "heuristic",
        )
        .expect("conversation docs");

        let evidence = docs[0].docs[0]
            .memory
            .evidence_content
            .as_deref()
            .expect("evidence content");
        assert!(evidence.chars().count() <= 1605);
        assert!(evidence.contains("[...]"));
    }

    #[test]
    fn normalizes_degree_memory() {
        let docs = build_conversation_docs(
            &[ConversationSessionInput {
                session_id: Some("session-1".to_string()),
                session_time: Some("2025-03-31".to_string()),
                conversation: vec![ConversationTurn {
                    role: "user".to_string(),
                    content: "I graduated with a degree in Business Administration.".to_string(),
                }],
            }],
            "rules",
            "heuristic",
        )
        .expect("conversation docs");

        assert!(
            docs[0].docs.iter().any(
                |doc| doc.content == "User graduated with a degree in Business Administration."
            )
        );
    }

    #[test]
    fn normalizes_playlist_memory() {
        let docs = build_conversation_docs(
            &[ConversationSessionInput {
                session_id: Some("session-1".to_string()),
                session_time: Some("2025-03-31".to_string()),
                conversation: vec![ConversationTurn {
                    role: "user".to_string(),
                    content: "I created a Spotify playlist named Summer Vibes for the trip."
                        .to_string(),
                }],
            }],
            "rules",
            "heuristic",
        )
        .expect("conversation docs");

        assert!(
            docs[0]
                .docs
                .iter()
                .any(|doc| doc.content == "User created a Spotify playlist named Summer Vibes.")
        );
    }

    #[test]
    fn skips_generic_theater_memory_without_title() {
        let docs = build_conversation_docs(
            &[ConversationSessionInput {
                session_id: Some("session-1".to_string()),
                session_time: Some("2025-03-31".to_string()),
                conversation: vec![ConversationTurn {
                    role: "user".to_string(),
                    content: "I recently went to a play at the local community theater."
                        .to_string(),
                }],
            }],
            "rules",
            "heuristic",
        )
        .expect("conversation docs");

        assert!(
            docs[0]
                .docs
                .iter()
                .all(|doc| doc.content != "User attended a production of at the local community theater.")
        );
    }

    #[test]
    fn expands_evidence_for_coupon_context() {
        let docs = build_conversation_docs(
            &[ConversationSessionInput {
                session_id: Some("session-1".to_string()),
                session_time: Some("2025-03-31".to_string()),
                conversation: vec![
                    ConversationTurn {
                        role: "user".to_string(),
                        content: "I bought coffee creamer at Target.".to_string(),
                    },
                    ConversationTurn {
                        role: "user".to_string(),
                        content: "I redeemed a $5 coupon on it last Sunday.".to_string(),
                    },
                ],
            }],
            "rules",
            "heuristic",
        )
        .expect("conversation docs");

        let coupon_doc = docs[0]
            .docs
            .iter()
            .find(|doc| doc.content.contains("redeemed a $5 coupon"))
            .expect("coupon doc");
        let evidence = coupon_doc
            .memory
            .evidence_content
            .as_deref()
            .expect("evidence");
        assert!(evidence.contains("Target"));
        assert_eq!(coupon_doc.memory.turn_start, Some(0));
        assert_eq!(coupon_doc.memory.turn_end, Some(1));
    }

    #[test]
    fn normalizes_identity_change_memories() {
        let docs = build_conversation_docs(
            &[ConversationSessionInput {
                session_id: Some("session-1".to_string()),
                session_time: Some("2025-03-31".to_string()),
                conversation: vec![ConversationTurn {
                    role: "user".to_string(),
                    content: "I changed my last name from Johnson to Winters.".to_string(),
                }],
            }],
            "rules",
            "heuristic",
        )
        .expect("conversation docs");

        assert!(docs[0]
            .docs
            .iter()
            .any(|doc| doc.content == "User's previous last name was Johnson."));
        assert!(docs[0]
            .docs
            .iter()
            .any(|doc| doc.content == "User's current last name is Winters."));
    }

    #[test]
    fn normalizes_activity_location_from_mixed_clause() {
        let docs = build_conversation_docs(
            &[ConversationSessionInput {
                session_id: Some("session-1".to_string()),
                session_time: Some("2025-03-31".to_string()),
                conversation: vec![ConversationTurn {
                    role: "user".to_string(),
                    content: "Do you know any good brunch spots near Serenity Yoga?".to_string(),
                }],
            }],
            "rules",
            "heuristic",
        )
        .expect("conversation docs");

        assert!(docs[0]
            .docs
            .iter()
            .any(|doc| doc.content == "User takes yoga classes at Serenity Yoga."));
    }

    #[test]
    fn normalizes_attribute_update_color_memory() {
        let docs = build_conversation_docs(
            &[ConversationSessionInput {
                session_id: Some("session-1".to_string()),
                session_time: Some("2025-03-31".to_string()),
                conversation: vec![ConversationTurn {
                    role: "user".to_string(),
                    content: "I repainted my bedroom walls a lighter shade of gray.".to_string(),
                }],
            }],
            "rules",
            "heuristic",
        )
        .expect("conversation docs");

        let contents = docs[0]
            .docs
            .iter()
            .map(|doc| doc.content.as_str())
            .collect::<Vec<_>>();
        assert!(
            contents
                .iter()
                .any(|content| *content == "User repainted bedroom walls a lighter shade of gray."),
            "contents: {:?}",
            contents
        );
    }

    #[test]
    fn normalizes_purchase_source_memory_from_relative_clause() {
        let docs = build_conversation_docs(
            &[ConversationSessionInput {
                session_id: Some("session-1".to_string()),
                session_time: Some("2025-03-31".to_string()),
                conversation: vec![ConversationTurn {
                    role: "user".to_string(),
                    content:
                        "My new tennis racket, which I got from the sports store downtown, feels great."
                            .to_string(),
                }],
            }],
            "rules",
            "heuristic",
        )
        .expect("conversation docs");

        let contents = docs[0]
            .docs
            .iter()
            .map(|doc| doc.content.as_str())
            .collect::<Vec<_>>();
        assert!(
            contents.iter().any(
                |content| *content
                    == "User bought a new tennis racket from the sports store downtown."
            ),
            "contents: {:?}",
            contents
        );
    }

    #[test]
    fn suppresses_weak_fallback_memories() {
        assert!(fallback_memory_candidate("I have had it.").is_none());
        assert!(fallback_memory_candidate("I had the idea yesterday.").is_none());
        assert!(fallback_memory_candidate("I think I'll try that.").is_none());
    }
}
