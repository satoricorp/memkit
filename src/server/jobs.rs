use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JobType {
    IndexSources,
    /// New pack: copy dir into sources, register, then run index. Job stays active for entire flow.
    IndexNewPack,
    AddDocuments,
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
    /// For AddDocuments: { "pack_path": string, "items": [ { "content": string, "source_path": string } ] }
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add_payload: Option<Value>,
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
