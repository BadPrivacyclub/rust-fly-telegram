//! Background jobs for long account operations (cleanup, bulk actions) with progress,
//! a short log, and cancellation. Jobs are kept in memory; finished ones are pruned.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::RwLock;

const MAX_JOBS: usize = 50;
const MAX_LOG_LINES: usize = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobSnapshot {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub account: String,
    pub status: JobStatus,
    pub done: u64,
    pub total: u64,
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
    pub log: Vec<String>,
    pub summary: Option<String>,
}

struct JobState {
    snapshot: JobSnapshot,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct JobManager {
    jobs: RwLock<VecDeque<JobState>>,
}

#[derive(Clone)]
pub struct Job {
    id: String,
    manager: Arc<JobManager>,
    cancel: Arc<AtomicBool>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl JobManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn start(self: &Arc<Self>, kind: &str, title: &str, account: &str) -> Job {
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut jobs = self.jobs.write().await;
        jobs.push_front(JobState {
            snapshot: JobSnapshot {
                id: id.clone(),
                kind: kind.to_string(),
                title: title.to_string(),
                account: account.to_string(),
                status: JobStatus::Running,
                done: 0,
                total: 0,
                started_ms: now_ms(),
                finished_ms: None,
                log: Vec::new(),
                summary: None,
            },
            cancel: Arc::clone(&cancel),
        });
        // Drop the oldest finished jobs beyond the limit; running jobs are always kept.
        while jobs.len() > MAX_JOBS {
            let Some(index) = jobs
                .iter()
                .rposition(|job| job.snapshot.status != JobStatus::Running)
            else {
                break;
            };
            jobs.remove(index);
        }
        Job {
            id,
            manager: Arc::clone(self),
            cancel,
        }
    }

    pub async fn list(&self) -> Vec<JobSnapshot> {
        self.jobs
            .read()
            .await
            .iter()
            .map(|job| job.snapshot.clone())
            .collect()
    }

    pub async fn get(&self, id: &str) -> Option<JobSnapshot> {
        self.jobs
            .read()
            .await
            .iter()
            .find(|job| job.snapshot.id == id)
            .map(|job| job.snapshot.clone())
    }

    pub async fn cancel(&self, id: &str) -> bool {
        match self
            .jobs
            .read()
            .await
            .iter()
            .find(|job| job.snapshot.id == id && job.snapshot.status == JobStatus::Running)
        {
            Some(job) => {
                job.cancel.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    pub async fn running(&self) -> usize {
        self.jobs
            .read()
            .await
            .iter()
            .filter(|job| job.snapshot.status == JobStatus::Running)
            .count()
    }

    async fn update(&self, id: &str, apply: impl FnOnce(&mut JobSnapshot)) {
        if let Some(job) = self
            .jobs
            .write()
            .await
            .iter_mut()
            .find(|job| job.snapshot.id == id)
        {
            apply(&mut job.snapshot);
        }
    }
}

impl Job {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub async fn set_total(&self, total: u64) {
        self.manager.update(&self.id, |job| job.total = total).await;
    }

    pub async fn advance(&self) {
        self.manager.update(&self.id, |job| job.done += 1).await;
    }

    pub async fn log(&self, line: impl Into<String>) {
        let line = line.into();
        tracing::debug!("job {}: {line}", self.id);
        self.manager
            .update(&self.id, |job| {
                job.log.push(line);
                if job.log.len() > MAX_LOG_LINES {
                    let overflow = job.log.len() - MAX_LOG_LINES;
                    job.log.drain(..overflow);
                }
            })
            .await;
    }

    pub async fn finish(&self, status: JobStatus, summary: impl Into<String>) {
        let summary = summary.into();
        self.manager
            .update(&self.id, |job| {
                job.status = status;
                job.finished_ms = Some(now_ms());
                job.summary = Some(summary);
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn job_lifecycle_and_cancel() {
        let manager = JobManager::new();
        let job = manager.start("cleanup", "Cleanup", "acc").await;
        job.set_total(3).await;
        job.advance().await;
        job.log("left group").await;
        assert_eq!(manager.running().await, 1);
        let snapshot = manager.get(job.id()).await.unwrap();
        assert_eq!((snapshot.done, snapshot.total), (1, 3));
        assert!(manager.cancel(job.id()).await);
        assert!(job.is_cancelled());
        job.finish(JobStatus::Cancelled, "stopped").await;
        assert_eq!(manager.running().await, 0);
        assert!(!manager.cancel(job.id()).await);
        assert_eq!(manager.list().await[0].summary.as_deref(), Some("stopped"));
    }

    #[tokio::test]
    async fn prunes_finished_jobs() {
        let manager = JobManager::new();
        for _ in 0..(MAX_JOBS + 5) {
            let job = manager.start("x", "x", "a").await;
            job.finish(JobStatus::Done, "").await;
        }
        manager.start("x", "running", "a").await;
        let list = manager.list().await;
        assert!(list.len() <= MAX_JOBS);
        assert_eq!(list[0].status, JobStatus::Running);
    }
}
