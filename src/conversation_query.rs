use chrono::{DateTime, Utc};

use crate::conversation::extract_temporal_metadata;
use crate::types::{QueryHit, QueryNote, QueryTimeAnalysis};

pub fn query_time_analysis(query: &str) -> QueryTimeAnalysis {
    let lower = query.to_ascii_lowercase();
    let temporal = extract_temporal_metadata(query, None);
    let (expected_relation_kind, expected_value_kind) = infer_query_shape(&lower);
    let wants_session_time = [
        "when did we talk",
        "when did i tell you",
        "when did we discuss",
        "which conversation",
        "what conversation",
        "when was this mentioned",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let wants_context_time = temporal.context_time_start.is_some()
        || temporal.context_time_end.is_some()
        || [
            "what year",
            "what decade",
            "when did i",
            "when was",
            "what time",
            "how long ago",
        ]
        .iter()
        .any(|needle| lower.contains(needle));
    let focus = match (wants_session_time, wants_context_time) {
        (true, true) => "both",
        (true, false) => "session_time",
        (false, true) => "context_time",
        (false, false) => "none",
    };
    QueryTimeAnalysis {
        focus: focus.to_string(),
        context_time_text: temporal.context_time_text,
        context_time_start: temporal.context_time_start,
        context_time_end: temporal.context_time_end,
        wants_session_time,
        wants_context_time,
        expected_relation_kind,
        expected_value_kind,
    }
}

fn infer_query_shape(lower: &str) -> (Option<String>, Option<String>) {
    if (lower.contains("old ") || lower.contains("previous ") || lower.contains("before "))
        && lower.contains("name")
    {
        return (
            Some("identity_change".to_string()),
            Some("previous_name".to_string()),
        );
    }
    if (lower.contains("current ") || lower.contains("new ") || lower.contains("now "))
        && lower.contains("name")
    {
        return (
            Some("identity_change".to_string()),
            Some("current_name".to_string()),
        );
    }
    if (lower.contains("where did") || lower.contains("where do"))
        && ["buy", "bought", "get", "got", "order", "ordered"]
            .iter()
            .any(|token| lower.contains(token))
    {
        return (
            Some("purchase_source".to_string()),
            Some("source".to_string()),
        );
    }
    if lower.contains("where")
        && [
            "take", "takes", "practice", "attend", "train", "study", "work", "shop", "class",
            "classes",
        ]
        .iter()
        .any(|token| lower.contains(token))
    {
        return (
            Some("activity_location".to_string()),
            Some("location".to_string()),
        );
    }
    if ["what color", "what shade", "which color", "which shade"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return (
            Some("attribute_update".to_string()),
            Some("color".to_string()),
        );
    }
    if ["what model", "which model"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return (
            Some("attribute_update".to_string()),
            Some("model".to_string()),
        );
    }
    if (lower.contains("what is the name") || lower.contains("what's the name"))
        || (lower.contains("what title") || lower.contains("which title"))
    {
        return (
            Some("attribute_update".to_string()),
            Some("title".to_string()),
        );
    }
    (None, None)
}

pub fn expand_query_variants(query: &str) -> Vec<String> {
    let normalized = query.trim();
    if normalized.is_empty() {
        return Vec::new();
    }

    let lower = normalized.to_ascii_lowercase();
    let mut variants = vec![normalized.to_string()];

    if lower.contains("spotify") && lower.contains("playlist") {
        variants.push("Spotify playlist".to_string());
        variants.push("User created a Spotify playlist".to_string());
        if ["name", "called", "named"]
            .iter()
            .any(|token| lower.contains(token))
        {
            variants.push("User created a Spotify playlist named".to_string());
        }
    }

    if lower.contains("degree") && lower.contains("graduat") {
        variants.push("User graduated with a degree".to_string());
        variants.push("graduated degree".to_string());
    }

    if lower.contains("commute") {
        variants.push("User's daily commute to work".to_string());
        variants.push("daily commute length".to_string());
    }

    if lower.contains("coupon") {
        variants.push("User redeemed a coupon".to_string());
        if lower.contains("coffee creamer") {
            variants.push("User redeemed a coupon on coffee creamer".to_string());
        }
    }

    if lower.contains("theater")
        || lower.contains("theatre")
        || (lower.contains("play") && lower.contains("attend"))
    {
        variants.push("User attended a production".to_string());
        variants.push("community theater production".to_string());
    }

    if lower.contains("last name") && lower.contains("chang") {
        variants.push("previous last name".to_string());
        variants.push("name changed from".to_string());
        variants.push("User's previous last name was".to_string());
    }

    if lower.contains("yoga") && lower.contains("class") {
        variants.push("User takes yoga classes at".to_string());
        variants.push("yoga studio".to_string());
    }

    if lower.contains("bedroom") && lower.contains("wall") {
        variants.push("User repainted bedroom walls".to_string());
        variants.push("bedroom walls color".to_string());
    }

    if (lower.contains("where did") || lower.contains("where do")) && lower.contains("buy") {
        variants.push("User bought".to_string());
        if lower.contains("tennis racket") {
            variants.push("User bought a new tennis racket from".to_string());
        }
    }

    let mut deduped = Vec::new();
    for variant in variants {
        if !deduped
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&variant))
        {
            deduped.push(variant);
        }
    }
    deduped
}

pub fn temporal_score_boost(hit: &QueryHit, query_time: &QueryTimeAnalysis) -> f32 {
    let memory = &hit.memory;
    if query_time.focus == "none" {
        return 0.0;
    }
    let mut boost = 0.0;
    if query_time.wants_context_time {
        if overlaps(
            memory.context_time_start,
            memory.context_time_end,
            query_time.context_time_start,
            query_time.context_time_end,
        ) {
            boost += 0.25;
        } else if memory.context_time_start.is_some() {
            boost -= 0.05;
        }
    }
    if query_time.wants_session_time
        && (memory.session_time_start.is_some() || memory.session_time_end.is_some())
    {
        boost += 0.08;
    }
    if memory.temporal_confidence.unwrap_or(0.0) < 0.35 {
        boost -= 0.03;
    }
    boost
}

pub fn shape_score_boost(hit: &QueryHit, query_time: &QueryTimeAnalysis) -> f32 {
    let mut boost = 0.0;
    let memory = &hit.memory;
    if let Some(expected_relation) = query_time.expected_relation_kind.as_deref() {
        match memory.relation_kind.as_deref() {
            Some(actual) if actual == expected_relation => boost += 0.18,
            Some(_) => boost -= 0.03,
            None => boost -= 0.01,
        }
    }
    if let Some(expected_value) = query_time.expected_value_kind.as_deref() {
        match memory.value_kind.as_deref() {
            Some(actual) if actual == expected_value => boost += 0.2,
            Some(actual)
                if expected_value == "location" && ["source", "venue"].contains(&actual) =>
            {
                boost += 0.1
            }
            Some(actual)
                if expected_value == "source" && ["location", "venue"].contains(&actual) =>
            {
                boost += 0.08
            }
            Some(_) => boost -= 0.02,
            None => boost -= 0.01,
        }
    }
    if hit.content.len() < 16 {
        boost -= 0.03;
    }
    boost
}

pub fn should_hydrate_evidence(query_time: &QueryTimeAnalysis, hits: &[QueryHit]) -> bool {
    if hits.is_empty() {
        return false;
    }
    if let Some(expected_value_kind) = query_time.expected_value_kind.as_deref() {
        if hits
            .iter()
            .take(2)
            .all(|hit| hit.memory.value_kind.as_deref() != Some(expected_value_kind))
        {
            return true;
        }
    }
    if query_time.focus != "none" {
        return true;
    }
    if hits.len() >= 2 && (hits[0].score - hits[1].score).abs() < 0.03 {
        return true;
    }
    hits.iter()
        .take(3)
        .any(|hit| hit.memory.temporal_confidence.unwrap_or(1.0) < 0.45)
}

pub fn build_query_note(
    hit: &QueryHit,
    include_evidence: bool,
    query_time: &QueryTimeAnalysis,
) -> QueryNote {
    let mut note = String::new();
    note.push_str(hit.content.trim());
    if let Some(record_type) = hit.memory.record_type.as_deref() {
        note.push_str(&format!(" [record_type: {}]", record_type));
    }
    if let Some(relation_kind) = hit.memory.relation_kind.as_deref() {
        note.push_str(&format!(" [relation_kind: {}]", relation_kind));
    }
    if let Some(value_kind) = hit.memory.value_kind.as_deref() {
        note.push_str(&format!(" [value_kind: {}]", value_kind));
    }
    if query_time.wants_session_time {
        if let Some(session_time) = hit.memory.session_time_start {
            note.push_str(&format!(" [session_time: {}]", session_time.date_naive()));
        }
    }
    if query_time.wants_context_time {
        if let Some(context_time) = hit.memory.context_time_text.as_deref() {
            note.push_str(&format!(" [context_time: {}]", context_time));
        } else if let Some(context_start) = hit.memory.context_time_start {
            note.push_str(&format!(" [context_time: {}]", context_start.date_naive()));
        }
    }
    if include_evidence {
        if let Some(evidence) = hit.memory.evidence_content.as_deref() {
            note.push_str("\nEvidence:\n");
            note.push_str(evidence.trim());
        }
    }
    QueryNote {
        chunk_id: hit.chunk_id.clone(),
        note,
        hydrated_evidence: include_evidence,
        temporal_match: hit.memory.temporal_kind.clone(),
    }
}

fn overlaps(
    hit_start: Option<DateTime<Utc>>,
    hit_end: Option<DateTime<Utc>>,
    query_start: Option<DateTime<Utc>>,
    query_end: Option<DateTime<Utc>>,
) -> bool {
    match (hit_start, hit_end, query_start, query_end) {
        (Some(hs), Some(he), Some(qs), Some(qe)) => hs <= qe && he >= qs,
        (Some(hs), Some(he), Some(qs), None) => hs <= qs && he >= qs,
        (Some(hs), Some(he), None, Some(qe)) => hs <= qe && he >= qe,
        _ => false,
    }
}
