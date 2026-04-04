use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};

use crate::helix_store::remove_helix_for_pack;
use crate::indexer::run_index;
use crate::pack::{load_manifest, remove_source_root, resolve_source_roots, scrub_pack_from_dir};
use crate::registry::remove_pack_by_path;

use super::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JobType {
    IndexSources,
    RemovePack,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct JobRecord {
    pub id: String,
    pub job_type: JobType,
    pub state: JobState,
    pub trigger: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pack_path: Option<String>,
    /// (temp_path_to_remove, pack_path) for iCloud: remove source root and delete temp after index
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_after_index: Option<(String, String)>,
    /// Directory or source roots being indexed in this job (for CLI status).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexing_sources: Option<Vec<String>>,
    pub enqueued_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub result: Option<Value>,
    pub error: Option<String>,
}

pub(crate) struct JobRegistry {
    pub(crate) jobs: Vec<JobRecord>,
    pub(crate) queue: std::collections::VecDeque<String>,
    pub(crate) running: Option<String>,
    pub(crate) next_id: u64,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            queue: std::collections::VecDeque::new(),
            running: None,
            next_id: 1,
        }
    }

    pub fn trim_history(&mut self, keep_last: usize) {
        if self.jobs.len() <= keep_last {
            return;
        }
        let drop_n = self.jobs.len() - keep_last;
        self.jobs.drain(0..drop_n);
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut JobRecord> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn find(&self, id: &str) -> Option<&JobRecord> {
        self.jobs.iter().find(|j| j.id == id)
    }
}

pub(super) fn job_targets_this_pack(
    j: &JobRecord,
    pack_root: Option<&str>,
    pack_dir: Option<&str>,
) -> bool {
    let Some(ref jp) = j.pack_path else {
        return false;
    };
    if Some(jp.as_str()) == pack_root {
        return true;
    }
    if Some(jp.as_str()) == pack_dir {
        return true;
    }
    false
}

pub(super) fn job_is_index_work(j: &JobRecord) -> bool {
    matches!(j.job_type, JobType::IndexSources)
}

pub(super) async fn enqueue_index_job(
    state: &AppState,
    trigger: &str,
    pack_path: Option<String>,
    cleanup_after_index: Option<(String, String)>,
    indexing_sources: Option<Vec<String>>,
) -> Value {
    let mut jobs = state.jobs.lock().await;
    let id = format!("job-{}", jobs.next_id);
    jobs.next_id += 1;
    let record = JobRecord {
        id: id.clone(),
        job_type: JobType::IndexSources,
        state: JobState::Queued,
        trigger: trigger.to_string(),
        pack_path: pack_path.clone(),
        cleanup_after_index: cleanup_after_index.clone(),
        indexing_sources,
        enqueued_at: Utc::now(),
        started_at: None,
        finished_at: None,
        result: None,
        error: None,
    };
    jobs.queue.push_back(id);
    jobs.jobs.push(record.clone());
    json!(record)
}

pub(super) async fn enqueue_remove_job(state: &AppState, pack_root: String) -> Value {
    let mut jobs = state.jobs.lock().await;
    let id = format!("job-{}", jobs.next_id);
    jobs.next_id += 1;
    let record = JobRecord {
        id: id.clone(),
        job_type: JobType::RemovePack,
        state: JobState::Queued,
        trigger: "manual_remove".to_string(),
        pack_path: Some(pack_root),
        cleanup_after_index: None,
        indexing_sources: None,
        enqueued_at: Utc::now(),
        started_at: None,
        finished_at: None,
        result: None,
        error: None,
    };
    jobs.queue.push_back(id);
    jobs.jobs.push(record.clone());
    json!(record)
}

pub(super) fn start_next_job_if_idle(state: AppState) {
    tokio::spawn(async move {
        type CleanupAfterIndex = Option<(String, String)>;
        type JobRunOutcome = Result<(Value, CleanupAfterIndex), (anyhow::Error, CleanupAfterIndex)>;

        enum JobWork {
            Index {
                packs: Vec<PathBuf>,
                cleanup: CleanupAfterIndex,
            },
            RemovePack {
                pack_root: PathBuf,
            },
        }
        let (maybe_job_id, work) = {
            let mut jobs = state.jobs.lock().await;
            if jobs.running.is_some() {
                return;
            }
            let Some(id) = jobs.queue.pop_front() else {
                return;
            };
            let job = jobs.find(&id).cloned();
            jobs.running = Some(id.clone());
            if let Some(ref mut job) = jobs.find_mut(&id) {
                job.state = JobState::Running;
                job.started_at = Some(Utc::now());
            }
            let work = match job.as_ref() {
                Some(j) if matches!(j.job_type, JobType::RemovePack) => {
                    let pack_root = j
                        .pack_path
                        .as_ref()
                        .map(PathBuf::from)
                        .unwrap_or_else(PathBuf::new);
                    JobWork::RemovePack { pack_root }
                }
                _ => {
                    let pack_path = job.as_ref().and_then(|j| j.pack_path.clone());
                    let cleanup = job.as_ref().and_then(|j| j.cleanup_after_index.clone());
                    let packs: Vec<PathBuf> = pack_path
                        .map(|p| vec![PathBuf::from(p)])
                        .unwrap_or_else(|| state.packs.iter().cloned().collect());
                    JobWork::Index { packs, cleanup }
                }
            };
            (id, work)
        };

        let run_outcome: JobRunOutcome = match work {
            JobWork::Index {
                packs: packs_to_index,
                cleanup: cleanup_after_index,
            } => {
                let run_result = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
                    let mut total_scanned = 0usize;
                    let mut total_updated = 0usize;
                    let mut total_chunks = 0usize;
                    let mut all_warnings: Vec<String> = Vec::new();
                    for pack in &packs_to_index {
                        let manifest = load_manifest(pack)?;
                        let sources = resolve_source_roots(pack, &manifest);
                        let (scanned, updated, chunks, warnings) = run_index(pack, &sources)?;
                        total_scanned += scanned;
                        total_updated += updated;
                        total_chunks += chunks;
                        all_warnings.extend(warnings);
                    }
                    Ok(json!({
                        "scanned": total_scanned,
                        "updated_files": total_updated,
                        "chunks": total_chunks,
                        "warnings": all_warnings
                    }))
                })
                .await;
                match run_result {
                    Ok(Ok(v)) => Ok((v, cleanup_after_index)),
                    Ok(Err(e)) => Err((e, cleanup_after_index)),
                    Err(e) => Err((
                        anyhow::anyhow!("job task failed: {}", e),
                        cleanup_after_index,
                    )),
                }
            }
            JobWork::RemovePack { pack_root } => {
                let run_result = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
                    remove_helix_for_pack(&pack_root)?;
                    remove_pack_by_path(&pack_root)?;
                    scrub_pack_from_dir(&pack_root)?;
                    Ok(json!({ "status": "removed" }))
                })
                .await;
                match run_result {
                    Ok(Ok(v)) => Ok((v, None)),
                    Ok(Err(e)) => Err((e, None)),
                    Err(e) => Err((anyhow::anyhow!("job task failed: {}", e), None)),
                }
            }
        };

        let (state_value, result_value, error_value, cleanup_after_index) = match run_outcome {
            Ok((v, cleanup)) => (JobState::Succeeded, Some(v), None, cleanup),
            Err((e, cleanup)) => (JobState::Failed, None, Some(e.to_string()), cleanup),
        };

        let mut jobs = state.jobs.lock().await;
        let finished_at = Utc::now();
        if let Some(job) = jobs.find_mut(&maybe_job_id) {
            job.state = state_value;
            job.result = result_value;
            job.error = error_value;
            job.finished_at = Some(finished_at);
        }
        jobs.running = None;
        jobs.trim_history(100);
        drop(jobs);

        if let Some((temp_path, pack_path)) = cleanup_after_index {
            let pack = PathBuf::from(&pack_path);
            let _ = remove_source_root(&pack, &temp_path);
            let _ = std::fs::remove_dir_all(&temp_path);
        }

        start_next_job_if_idle(state.clone());
    });
}
