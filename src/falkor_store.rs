use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use redis::{ConnectionLike, Value};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

use crate::types::QueryHit;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRelation {
    pub source: String,
    pub relation: String,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct ChunkGraphPayload {
    pub chunk_id: String,
    pub file_path: String,
    pub chunk_index: usize,
    pub content_hash: String,
    pub content: String,
    pub entities: Vec<String>,
    pub relations: Vec<GraphRelation>,
}

fn escape_cypher(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', " ")
}

fn tokenize_query(q: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    q.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3)
        .filter_map(|t| {
            if seen.insert(t.to_string()) {
                Some(t.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn connect_unix_socket(socket_path: &str) -> Result<redis::Connection> {
    let url = format!("unix://{socket_path}");
    let client = redis::Client::open(url).context("failed to open redis unix client")?;
    client
        .get_connection()
        .context("failed to connect to falkor unix socket")
}

fn graph_query_raw<C: ConnectionLike>(
    con: &mut C,
    graph_name: &str,
    cypher: &str,
) -> Result<Value> {
    let out = redis::cmd("GRAPH.QUERY")
        .arg(graph_name)
        .arg(cypher)
        .arg("--compact")
        .query(con)
        .with_context(|| format!("falkor GRAPH.QUERY failed for graph={graph_name}"))?;
    Ok(out)
}

fn value_to_json(v: &Value) -> JsonValue {
    match v {
        Value::Nil => JsonValue::Null,
        Value::Int(i) => json!(i),
        Value::BulkString(b) => json!(String::from_utf8_lossy(b).to_string()),
        Value::SimpleString(s) => json!(s),
        Value::Array(a) => JsonValue::Array(a.iter().map(value_to_json).collect()),
        Value::Okay => json!("OK"),
        Value::Map(m) => JsonValue::Array(
            m.iter()
                .map(|(k, v)| json!({"key": value_to_json(k), "value": value_to_json(v)}))
                .collect(),
        ),
        Value::Attribute { .. } => json!(format!("{v:?}")),
        Value::Set(values) => JsonValue::Array(values.iter().map(value_to_json).collect()),
        Value::Double(f) => json!(f),
        Value::Boolean(b) => json!(b),
        Value::VerbatimString { text, .. } => json!(text),
        Value::BigNumber(n) => json!(n.to_string()),
        Value::Push { .. } => json!(format!("{v:?}")),
        _ => json!(format!("{v:?}")),
    }
}

fn compact_cell_to_json(v: &Value) -> JsonValue {
    match v {
        Value::Array(parts) if parts.len() == 2 => match &parts[0] {
            Value::Int(_) => value_to_json(&parts[1]),
            _ => value_to_json(v),
        },
        _ => value_to_json(v),
    }
}

fn rows_from_compact(value: &Value) -> Vec<Vec<JsonValue>> {
    let Value::Array(top) = value else {
        return Vec::new();
    };
    if top.is_empty() {
        return Vec::new();
    }
    if let Some(Value::Array(data_rows)) = top.get(1) {
        let mut rows = Vec::new();
        for row in data_rows {
            if let Value::Array(cols) = row {
                rows.push(cols.iter().map(compact_cell_to_json).collect());
            }
        }
        if !rows.is_empty() {
            return rows;
        }
    }
    if top.len() < 3 {
        return Vec::new();
    }
    let mut rows = Vec::new();
    for row in &top[1..top.len().saturating_sub(1)] {
        if let Value::Array(cols) = row {
            rows.push(cols.iter().map(compact_cell_to_json).collect());
        }
    }
    rows
}

pub fn graph_name_from_env() -> String {
    std::env::var("FALKOR_GRAPH").unwrap_or_else(|_| "memkit".to_string())
}

pub fn socket_from_env() -> Option<String> {
    std::env::var("FALKORDB_SOCKET")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

pub fn upsert_chunks(
    socket_path: &str,
    graph_name: &str,
    chunks: &[ChunkGraphPayload],
) -> Result<usize> {
    if chunks.is_empty() {
        return Ok(0);
    }

    let mut con = connect_unix_socket(socket_path)?;
    for chunk in chunks {
        let chunk_id = escape_cypher(&chunk.chunk_id);
        let file_path = escape_cypher(&chunk.file_path);
        let content_hash = escape_cypher(&chunk.content_hash);
        let content = escape_cypher(&chunk.content);
        let query = format!(
            "MERGE (c:Chunk {{id:'{chunk_id}'}}) \
             SET c.file_path='{file_path}', c.chunk_index={}, c.content_hash='{content_hash}', c.content='{content}'",
            chunk.chunk_index
        );
        graph_query_raw(&mut con, graph_name, &query)?;

        for e in &chunk.entities {
            let ent = escape_cypher(e);
            let mention = format!(
                "MATCH (c:Chunk {{id:'{chunk_id}'}}) \
                 MERGE (e:Entity {{name:'{ent}'}}) \
                 MERGE (c)-[:MENTIONS]->(e)"
            );
            graph_query_raw(&mut con, graph_name, &mention)?;
        }

        for rel in &chunk.relations {
            let src = escape_cypher(&rel.source);
            let dst = escape_cypher(&rel.target);
            let kind = escape_cypher(&rel.relation);
            let rel_q = format!(
                "MERGE (s:Entity {{name:'{src}'}}) \
                 MERGE (t:Entity {{name:'{dst}'}}) \
                 MERGE (s)-[:RELATED {{kind:'{kind}'}}]->(t)"
            );
            graph_query_raw(&mut con, graph_name, &rel_q)?;
        }
    }

    Ok(chunks.len())
}

pub fn delete_chunks_for_paths(
    socket_path: &str,
    graph_name: &str,
    file_paths: &[String],
) -> Result<usize> {
    if file_paths.is_empty() {
        return Ok(0);
    }
    let mut con = connect_unix_socket(socket_path)?;
    let mut deleted = 0usize;
    for path in file_paths {
        let escaped = escape_cypher(path);
        let query = format!(
            "MATCH (c:Chunk {{file_path:'{escaped}'}}) \
             DETACH DELETE c"
        );
        graph_query_raw(&mut con, graph_name, &query)?;
        deleted += 1;
    }
    Ok(deleted)
}

pub fn query_chunks(
    socket_path: &str,
    graph_name: &str,
    query: &str,
    top_k: usize,
) -> Result<Vec<QueryHit>> {
    let terms = tokenize_query(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }

    let where_clause = terms
        .iter()
        .map(|t| format!("toLower(e.name) CONTAINS '{}'", escape_cypher(t)))
        .collect::<Vec<_>>()
        .join(" OR ");
    let cypher = format!(
        "MATCH (c:Chunk)-[:MENTIONS]->(e:Entity) \
         WHERE {where_clause} \
         RETURN c.id, c.file_path, c.chunk_index, c.content, count(e) \
         ORDER BY count(e) DESC \
         LIMIT {}",
        top_k * 4
    );

    let mut con = connect_unix_socket(socket_path)?;
    let raw = graph_query_raw(&mut con, graph_name, &cypher)?;
    let mut out = Vec::new();
    for row in rows_from_compact(&raw) {
        if row.len() < 5 {
            continue;
        }
        let chunk_id = row[0].as_str().unwrap_or_default().to_string();
        if chunk_id.is_empty() {
            continue;
        }
        let file_path = row[1].as_str().unwrap_or_default().to_string();
        let chunk_index = row[2]
            .as_i64()
            .or_else(|| row[2].as_u64().map(|v| v as i64))
            .unwrap_or(0) as usize;
        let content = row[3].as_str().unwrap_or_default().to_string();
        let score = row[4]
            .as_f64()
            .or_else(|| row[4].as_i64().map(|v| v as f64))
            .unwrap_or(0.0) as f32;
        out.push(QueryHit {
            score,
            file_path,
            chunk_id,
            chunk_index,
            content,
            start_offset: None,
            end_offset: None,
            source: "graph".to_string(),
            group_key: None,
        });
    }
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(top_k);
    Ok(out)
}

pub fn graph_counts(socket_path: &str, graph_name: &str) -> Result<(usize, usize)> {
    let mut con = connect_unix_socket(socket_path)?;
    let entities = graph_query_raw(&mut con, graph_name, "MATCH (e:Entity) RETURN count(e)")?;
    let rels = graph_query_raw(&mut con, graph_name, "MATCH ()-[r:RELATED]->() RETURN count(r)")?;
    let entity_count = rows_from_compact(&entities)
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|x| x as u64)))
        .unwrap_or(0) as usize;
    let rel_count = rows_from_compact(&rels)
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|x| x as u64)))
        .unwrap_or(0) as usize;
    Ok((entity_count, rel_count))
}

pub fn graph_schema(socket_path: &str, graph_name: &str) -> Result<JsonValue> {
    let mut con = connect_unix_socket(socket_path)?;
    let labels = graph_query_raw(&mut con, graph_name, "CALL db.labels()")?;
    let rel_types = graph_query_raw(&mut con, graph_name, "CALL db.relationshipTypes()")?;
    Ok(json!({
        "graph": graph_name,
        "labels_raw": value_to_json(&labels),
        "relationship_types_raw": value_to_json(&rel_types)
    }))
}

pub fn graph_subgraph(
    socket_path: &str,
    graph_name: &str,
    query: &str,
    depth: usize,
    limit: usize,
) -> Result<JsonValue> {
    let terms = tokenize_query(query);
    let mut con = connect_unix_socket(socket_path)?;

    let chunk_ids = if terms.is_empty() {
        Vec::new()
    } else {
        let where_clause = terms
            .iter()
            .map(|t| format!("toLower(e.name) CONTAINS '{}'", escape_cypher(t)))
            .collect::<Vec<_>>()
            .join(" OR ");
        let lookup = format!(
            "MATCH (c:Chunk)-[:MENTIONS]->(e:Entity) \
             WHERE {where_clause} \
             RETURN c.id \
             LIMIT {}",
            limit.max(1)
        );
        let raw = graph_query_raw(&mut con, graph_name, &lookup)?;
        let mut ids = rows_from_compact(&raw)
            .into_iter()
            .filter_map(|row| {
                row.first()
                    .and_then(JsonValue::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<Vec<_>>();
        if ids.is_empty() {
            let chunk_where = terms
                .iter()
                .map(|t| {
                    format!(
                        "toLower(c.content) CONTAINS '{v}' OR toLower(c.file_path) CONTAINS '{v}'",
                        v = escape_cypher(t)
                    )
                })
                .collect::<Vec<_>>()
                .join(" OR ");
            let chunk_lookup = format!(
                "MATCH (c:Chunk) \
                 WHERE {chunk_where} \
                 RETURN c.id \
                 LIMIT {}",
                limit.max(1)
            );
            let raw_chunks = graph_query_raw(&mut con, graph_name, &chunk_lookup)?;
            ids = rows_from_compact(&raw_chunks)
                .into_iter()
                .filter_map(|row| {
                    row.first()
                        .and_then(JsonValue::as_str)
                        .map(ToOwned::to_owned)
                })
                .collect::<Vec<_>>();
        }
        ids
    };

    if chunk_ids.is_empty() {
        return Ok(json!({"nodes": [], "edges": []}));
    }

    let ids_list = chunk_ids
        .iter()
        .map(|id| format!("'{}'", escape_cypher(id)))
        .collect::<Vec<_>>()
        .join(",");
    let seed_cypher = format!(
        "MATCH (c:Chunk) \
         WHERE c.id IN [{}] \
         RETURN c.id, c.file_path, c.chunk_index \
         LIMIT {}",
        ids_list,
        limit.max(1) * 4
    );
    let seed_raw = graph_query_raw(&mut con, graph_name, &seed_cypher)?;
    let seed_rows = rows_from_compact(&seed_raw);

    let cypher = format!(
        "MATCH (c:Chunk)-[r*1..{}]-(n) \
         WHERE c.id IN [{}] \
         RETURN c.id, c.file_path, c.chunk_index, n.name, labels(n) \
         LIMIT {}",
        depth.max(1),
        ids_list,
        limit.max(1) * 20
    );
    let raw = graph_query_raw(&mut con, graph_name, &cypher)?;
    let rows = rows_from_compact(&raw);

    let mut nodes = serde_json::Map::new();
    let mut edge_counts: HashMap<(String, String, String), usize> = HashMap::new();
    for row in seed_rows {
        if row.len() < 3 {
            continue;
        }
        let c_id = row[0].as_str().unwrap_or_default().to_string();
        let c_path = row[1].as_str().unwrap_or_default().to_string();
        let c_idx = row[2].as_i64().unwrap_or(0);
        if !c_id.is_empty() {
            nodes.insert(
                c_id.clone(),
                json!({"id": c_id, "label": format!("{c_path}:{c_idx}"), "kind": "Chunk"}),
            );
        }
    }
    for row in rows {
        if row.len() < 5 {
            continue;
        }
        let c_id = row[0].as_str().unwrap_or_default().to_string();
        let c_path = row[1].as_str().unwrap_or_default().to_string();
        let c_idx = row[2].as_i64().unwrap_or(0);
        if !c_id.is_empty() {
            nodes.insert(
                c_id.clone(),
                json!({"id": c_id, "label": format!("{c_path}:{c_idx}"), "kind": "Chunk"}),
            );
        }

        let n_name = row[3].as_str().unwrap_or_default().to_string();
        if !n_name.is_empty() {
            nodes.insert(
                n_name.clone(),
                json!({"id": n_name, "label": n_name, "kind": "Entity"}),
            );
            if !c_id.is_empty() {
                let key = (c_id, n_name, "related".to_string());
                *edge_counts.entry(key).or_insert(0) += 1;
            }
        }
    }

    let mut edges = edge_counts
        .into_iter()
        .map(|((source, target, kind), count)| {
            json!({"source": source, "target": target, "kind": kind, "count": count})
        })
        .collect::<Vec<_>>();
    edges.sort_by(|a, b| {
        let a_source = a.get("source").and_then(JsonValue::as_str).unwrap_or_default();
        let b_source = b.get("source").and_then(JsonValue::as_str).unwrap_or_default();
        let a_target = a.get("target").and_then(JsonValue::as_str).unwrap_or_default();
        let b_target = b.get("target").and_then(JsonValue::as_str).unwrap_or_default();
        a_source
            .cmp(b_source)
            .then_with(|| a_target.cmp(b_target))
    });

    Ok(json!({
        "nodes": nodes.into_values().collect::<Vec<_>>(),
        "edges": edges
    }))
}

