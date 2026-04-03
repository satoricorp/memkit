use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::file_tree::format_file_tree;
use crate::helix_store::{
    helix_graph_counts, helix_read_index_warnings, helix_try_cached_index_status,
};
use crate::pack::{load_manifest, resolve_pack_dir, resolve_source_roots};
use crate::registry::pack_dir_for_path;

use super::{
    AppState, JobRecord, JobState, JobType, job_is_index_work, job_targets_this_pack,
    load_pack_docs, resolve_strict_local_pack_root,
};

#[derive(Deserialize, Default)]
pub(super) struct StatusQuery {
    path: Option<String>,
}

pub(super) async fn status(
    State(state): State<AppState>,
    Query(q): Query<StatusQuery>,
) -> Json<Value> {
    let (
        pack_str,
        sources,
        vector_count,
        indexed,
        file_paths,
        pack_for_helix,
        source_root_paths,
        index_warnings,
    ) = if let Some(ref path) = q.path {
        match resolve_strict_local_pack_root(path) {
            Ok(pack_root) => {
                let pack_dir = resolve_pack_dir(&pack_root);
                let manifest = load_manifest(&pack_dir).ok();
                let sources = manifest
                    .as_ref()
                    .map(|m| m.sources.clone())
                    .unwrap_or_default();
                let source_root_paths: Vec<String> = manifest
                    .as_ref()
                    .map(|m| {
                        resolve_source_roots(&pack_dir, m)
                            .into_iter()
                            .map(|p| p.to_string_lossy().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                let (vector_count, file_paths, indexed, index_warnings) =
                    if let Some((vc, mut fp, w)) = helix_try_cached_index_status(&pack_dir) {
                        fp.sort_unstable();
                        fp.dedup();
                        (vc, fp, vc > 0, w)
                    } else {
                        let docs = manifest
                            .as_ref()
                            .and_then(|m| load_pack_docs(&pack_dir, m.embedding.dimension).ok())
                            .unwrap_or_default();
                        let mut fp: Vec<String> =
                            docs.iter().map(|d| d.source_path.clone()).collect();
                        fp.sort_unstable();
                        fp.dedup();
                        let n = docs.len();
                        let iw = helix_read_index_warnings(&pack_dir);
                        (n, fp, n > 0, iw)
                    };
                let display = display_pack_path(&pack_root);
                (
                    display,
                    sources,
                    vector_count,
                    indexed,
                    file_paths,
                    Some(pack_dir),
                    source_root_paths,
                    index_warnings,
                )
            }
            Err(e) => {
                return Json(json!({
                    "status":"error",
                    "error":{"code":"INVALID_PACK","message":e.to_string()}
                }));
            }
        }
    } else {
        let mut all_sources = Vec::new();
        let mut all_paths = Vec::new();
        let mut total_vectors = 0usize;
        for pack_root in state.packs.iter() {
            let pack_dir = pack_dir_for_path(pack_root);
            if let Ok(m) = load_manifest(&pack_dir) {
                all_sources.extend(m.sources);
            }
            if let Some((n, paths, _)) = helix_try_cached_index_status(&pack_dir) {
                total_vectors += n;
                all_paths.extend(paths);
            } else if let Ok(m) = load_manifest(&pack_dir) {
                if let Ok(docs) = load_pack_docs(&pack_dir, m.embedding.dimension) {
                    total_vectors += docs.len();
                    all_paths.extend(docs.iter().map(|d| d.source_path.clone()));
                }
            }
        }
        let pack_str = if state.packs.len() == 1 {
            display_pack_path(&state.packs[0])
        } else {
            format!("{} packs", state.packs.len())
        };
        let pack_for_helix = state.packs.first().map(|r| pack_dir_for_path(r));
        (
            pack_str,
            all_sources,
            total_vectors,
            total_vectors > 0,
            all_paths,
            pack_for_helix,
            Vec::<String>::new(),
            Vec::<String>::new(),
        )
    };

    let (entities, relationships) = pack_for_helix
        .as_ref()
        .map(|p| helix_graph_counts(p.as_path()))
        .unwrap_or((0, 0));
    let base_path: String = state
        .packs
        .first()
        .and_then(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .filter(|n| *n == ".memkit")
                .and_then(|_| p.parent())
                .map(|pa| pa.display().to_string())
        })
        .unwrap_or_else(|| pack_str.clone());
    let file_tree = format_file_tree(&file_paths, &base_path);

    let (active_job, last_job, queued_list) = {
        let jobs = state.jobs.lock().await;
        let active = jobs.running.as_ref().and_then(|id| jobs.find(id).cloned());
        let last = jobs
            .jobs
            .iter()
            .rev()
            .find(|j| !matches!(j.state, JobState::Queued | JobState::Running))
            .cloned();
        let queued_list: Vec<Value> = jobs
            .queue
            .iter()
            .filter_map(|id| jobs.find(id))
            .map(job_summary)
            .collect();
        (active, last, queued_list)
    };

    let pack_root_opt = pack_for_helix.as_ref().and_then(|pack_dir| {
        pack_dir
            .parent()
            .map(PathBuf::from)
            .or_else(|| Some(pack_dir.clone()))
            .and_then(|p| p.canonicalize().ok())
            .map(|p| p.to_string_lossy().to_string())
    });
    let pack_dir_opt = pack_for_helix
        .as_ref()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.to_string_lossy().to_string());
    let pr = pack_root_opt.as_deref();
    let pd = pack_dir_opt.as_deref();

    let (pending_removal, pending_add) = if q.path.is_some() {
        let path_matches_pack = |p: &str| Some(p) == pr || Some(p) == pd;
        let active_remove = active_job.as_ref().is_some_and(|j| {
            matches!(j.job_type, JobType::RemovePack)
                && j.pack_path
                    .as_deref()
                    .map(path_matches_pack)
                    .unwrap_or(false)
        });
        let queued_remove = queued_list.iter().any(|j| {
            j.get("job_type").and_then(Value::as_str) == Some("remove_pack")
                && j.get("pack_path")
                    .and_then(Value::as_str)
                    .map(path_matches_pack)
                    .unwrap_or(false)
        });
        let active_add = active_job
            .as_ref()
            .is_some_and(|j| job_is_index_work(j) && job_targets_this_pack(j, pr, pd));
        let queued_add = queued_list.iter().any(|j| {
            let jt = j.get("job_type").and_then(Value::as_str);
            let is_index = matches!(
                jt,
                Some("index_sources") | Some("index_new_pack") | Some("add_documents")
            );
            is_index
                && j.get("pack_path")
                    .and_then(Value::as_str)
                    .map(path_matches_pack)
                    .unwrap_or(false)
        });
        (active_remove || queued_remove, active_add || queued_add)
    } else {
        (false, false)
    };

    let pack_indexing_busy = if q.path.is_some() {
        let active_busy = active_job
            .as_ref()
            .is_some_and(|j| job_is_index_work(j) && job_targets_this_pack(j, pr, pd));
        let queued_busy = queued_list.iter().any(|j| {
            let jt = j.get("job_type").and_then(Value::as_str);
            matches!(
                jt,
                Some("index_sources") | Some("index_new_pack") | Some("add_documents")
            ) && j
                .get("pack_path")
                .and_then(Value::as_str)
                .map(|p| Some(p) == pr || Some(p) == pd)
                .unwrap_or(false)
        });
        active_busy || queued_busy
    } else {
        false
    };

    let active_for_this_pack = if q.path.is_some() {
        active_job
            .as_ref()
            .filter(|j| job_targets_this_pack(j, pr, pd))
            .cloned()
    } else {
        None
    };
    let queued_jobs_for_this_pack: Vec<Value> = if q.path.is_some() {
        queued_list
            .iter()
            .filter(|j| {
                j.get("pack_path")
                    .and_then(Value::as_str)
                    .map(|p| Some(p) == pr || Some(p) == pd)
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    let pack_paths: Vec<String> = state.packs.iter().map(|p| display_pack_path(p)).collect();
    Json(json!({
        "status": "ok",
        "pack_path": pack_str,
        "pack_paths": pack_paths,
        "indexed": indexed,
        "vector_count": vector_count,
        "entities": entities,
        "relationships": relationships,
        "file_tree": file_tree,
        "sources": sources,
        "source_root_paths": source_root_paths,
        "index_warnings": index_warnings,
        "pending_removal": pending_removal,
        "pending_add": pending_add,
        "pack_indexing_busy": pack_indexing_busy,
        "jobs": {
            "active": active_job,
            "active_for_this_pack": active_for_this_pack,
            "last_completed": last_job,
            "queued": queued_list.len(),
            "queued_jobs": queued_list,
            "queued_jobs_for_this_pack": queued_jobs_for_this_pack
        }
    }))
}

fn display_pack_path(pack_root: &Path) -> String {
    let is_home = dirs::home_dir()
        .as_ref()
        .and_then(|h| h.canonicalize().ok())
        .as_ref()
        == pack_root.canonicalize().as_ref().ok();
    if is_home {
        "~/.memkit".to_string()
    } else {
        pack_root.display().to_string()
    }
}

fn job_summary(j: &JobRecord) -> Value {
    let mut v = json!({
        "id": j.id,
        "job_type": j.job_type,
        "pack_path": j.pack_path,
        "state": j.state,
    });
    if let Some(ref s) = j.indexing_sources {
        v["indexing_sources"] = json!(s);
    }
    v
}
