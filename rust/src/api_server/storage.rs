use std::sync::Arc;

use chrono::Utc;
use dashmap::DashMap;

use super::models::DownloadStatus;

macro_rules! update_task {
    ($store:expr, $task_id:expr, $task:ident => $body:block) => {
        if let Some(mut $task) = $store.inner.get_mut($task_id) {
            $body
        }
    };
}

#[derive(Clone, Default)]
pub struct TaskStore {
    inner: Arc<DashMap<String, DownloadStatus>>,
}

impl TaskStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_queued(&self, task_id: String, source_url: String) {
        self.inner
            .insert(task_id.clone(), DownloadStatus::queued(task_id, source_url));
    }

    pub fn get(&self, task_id: &str) -> Option<DownloadStatus> {
        self.inner.get(task_id).map(|entry| entry.clone())
    }

    pub fn list(&self) -> Vec<DownloadStatus> {
        let mut tasks = self
            .inner
            .iter()
            .map(|entry| entry.value().clone())
            .collect::<Vec<_>>();
        tasks.sort_by_key(|task| std::cmp::Reverse(task.created_at));
        tasks
    }

    pub fn update_progress(
        &self,
        task_id: &str,
        status: &str,
        message: &str,
        progress_percent: Option<f64>,
        output_path: Option<String>,
    ) {
        update_task!(self, task_id, task => {
            task.status = status.to_string();
            task.message = message.to_string();
            task.progress_percent = progress_percent;
            if let Some(output_path) = output_path {
                task.output_path = Some(output_path);
            }
        });
    }

    pub fn mark_completed(&self, task_id: &str, output_path: String) {
        update_task!(self, task_id, task => {
            task.status = "completed".to_string();
            task.message = "Download completed".to_string();
            task.progress_percent = Some(100.0);
            task.output_path = Some(output_path);
            task.error = None;
            task.completed_at = Some(Utc::now());
        });
    }

    pub fn mark_failed(&self, task_id: &str, error: &str) {
        update_task!(self, task_id, task => {
            task.status = "failed".to_string();
            task.message = "Download failed".to_string();
            task.error = Some(error.to_string());
            task.completed_at = Some(Utc::now());
        });
    }
}
