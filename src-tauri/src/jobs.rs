//! Background job runner.
//!
//! Long-running work (import, pipeline runs, export) runs on its own thread. Each job reports
//! progress through [`JobCtx`], which emits [`JOB_UPDATE_EVENT`] to the frontend, and checks
//! [`JobCtx::is_cancelled`] between batches so the user can stop it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aeon_shared::{JOB_UPDATE_EVENT, JobId, JobKind, JobLink, JobState, JobStatus, Progress};

use crate::error::{AppError, AppResult, now_ms};

/// Minimum time between progress events for one job, so fast loops don't flood the UI.
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(100);

/// Finished jobs kept in the list before the oldest are dropped.
const MAX_FINISHED_JOBS: usize = 50;

/// Where job status updates are sent. Implemented for `tauri::AppHandle`; tests use a recorder.
pub trait JobEvents: Send + Sync + 'static {
    fn job_updated(&self, status: &JobStatus);
}

impl<R: tauri::Runtime> JobEvents for tauri::AppHandle<R> {
    fn job_updated(&self, status: &JobStatus) {
        use tauri::Emitter;
        if let Err(e) = self.emit(JOB_UPDATE_EVENT, status) {
            log::warn!("failed to emit job update: {e}");
        }
    }
}

struct JobEntry {
    status: JobStatus,
    cancel: Arc<AtomicBool>,
}

pub struct JobManager {
    next_id: AtomicU64,
    jobs: Mutex<HashMap<JobId, JobEntry>>,
    events: Arc<dyn JobEvents>,
}

/// Outcome a job function returns.
pub enum JobOutcome {
    Completed(Option<String>),
    /// Completed, with a link to what the job produced.
    Produced(String, JobLink),
    Cancelled,
}

impl JobManager {
    pub fn new(events: Arc<dyn JobEvents>) -> Arc<Self> {
        Arc::new(Self { next_id: AtomicU64::new(1), jobs: Mutex::new(HashMap::new()), events })
    }

    /// Starts `work` on a background thread and returns its id immediately.
    pub fn spawn<F>(self: &Arc<Self>, kind: JobKind, title: impl Into<String>, work: F) -> AppResult<JobId>
    where
        F: FnOnce(&mut JobCtx) -> AppResult<JobOutcome> + Send + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        let status = JobStatus {
            id,
            kind,
            title: title.into(),
            state: JobState::Queued,
            progress: Progress::default(),
            message: None,
            started_at_ms: now_ms(),
            finished_at_ms: None,
            link: None,
        };
        self.lock()?.insert(id, JobEntry { status: status.clone(), cancel: cancel.clone() });
        self.events.job_updated(&status);

        let manager = Arc::clone(self);
        std::thread::Builder::new()
            .name(format!("job-{id}"))
            .spawn(move || {
                let mut ctx = JobCtx { id, manager: Arc::clone(&manager), cancel, last_emit: None };
                manager.update(id, |s| s.state = JobState::Running);

                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&mut ctx)));
                manager.update(id, |s| {
                    s.finished_at_ms = Some(now_ms());
                    match result {
                        Ok(Ok(JobOutcome::Completed(msg))) => {
                            s.state = JobState::Completed;
                            if let Some(total) = s.progress.total {
                                s.progress.done = total;
                            }
                            if msg.is_some() {
                                s.message = msg;
                            }
                        }
                        Ok(Ok(JobOutcome::Produced(msg, link))) => {
                            s.state = JobState::Completed;
                            if let Some(total) = s.progress.total {
                                s.progress.done = total;
                            }
                            s.message = Some(msg);
                            s.link = Some(link);
                        }
                        Ok(Ok(JobOutcome::Cancelled)) => s.state = JobState::Cancelled,
                        Ok(Err(e)) => {
                            s.state = JobState::Failed;
                            s.message = Some(e.to_string());
                        }
                        Err(_) => {
                            s.state = JobState::Failed;
                            s.message = Some("The job crashed unexpectedly".into());
                        }
                    }
                });
                manager.prune_finished();
            })
            .map_err(AppError::Io)?;
        Ok(id)
    }

    /// Requests cancellation. The job stops at its next cancellation check.
    pub fn cancel(&self, id: JobId) -> AppResult<()> {
        let jobs = self.lock()?;
        let entry = jobs.get(&id).ok_or(AppError::JobNotFound(id))?;
        entry.cancel.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// All known jobs, newest first.
    pub fn list(&self) -> AppResult<Vec<JobStatus>> {
        let mut list: Vec<_> = self.lock()?.values().map(|e| e.status.clone()).collect();
        list.sort_by_key(|j| std::cmp::Reverse(j.id));
        Ok(list)
    }

    pub fn clear_finished(&self) -> AppResult<()> {
        self.lock()?.retain(|_, e| !e.status.state.is_finished());
        Ok(())
    }

    fn lock(&self) -> AppResult<std::sync::MutexGuard<'_, HashMap<JobId, JobEntry>>> {
        self.jobs.lock().map_err(|_| AppError::Internal("job table lock poisoned".into()))
    }

    fn update(&self, id: JobId, f: impl FnOnce(&mut JobStatus)) {
        let snapshot = {
            let Ok(mut jobs) = self.jobs.lock() else { return };
            let Some(entry) = jobs.get_mut(&id) else { return };
            f(&mut entry.status);
            entry.status.clone()
        };
        self.events.job_updated(&snapshot);
    }

    fn prune_finished(&self) {
        let Ok(mut jobs) = self.jobs.lock() else { return };
        let mut finished: Vec<JobId> =
            jobs.values().filter(|e| e.status.state.is_finished()).map(|e| e.status.id).collect();
        if finished.len() > MAX_FINISHED_JOBS {
            finished.sort_unstable();
            for id in &finished[..finished.len() - MAX_FINISHED_JOBS] {
                jobs.remove(id);
            }
        }
    }
}

/// Handle a running job uses to report progress and check for cancellation.
pub struct JobCtx {
    id: JobId,
    manager: Arc<JobManager>,
    cancel: Arc<AtomicBool>,
    last_emit: Option<Instant>,
}

impl JobCtx {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Reports progress. Events are throttled; the final state is always sent when the job ends.
    pub fn progress(&mut self, done: u64, total: Option<u64>, message: Option<String>) {
        let now = Instant::now();
        let due = self.last_emit.is_none_or(|t| now.duration_since(t) >= PROGRESS_EMIT_INTERVAL);
        if !due {
            return;
        }
        self.last_emit = Some(now);
        self.manager.update(self.id, |s| {
            s.progress = Progress { done, total };
            if message.is_some() {
                s.message = message;
            }
        });
    }
}

/// Diagnostic job: counts to `steps`, sleeping between steps. Used to check progress
/// reporting and cancellation end to end.
pub fn self_test(steps: u64, step_delay: Duration) -> impl FnOnce(&mut JobCtx) -> AppResult<JobOutcome> {
    move |ctx| {
        for i in 0..steps {
            if ctx.is_cancelled() {
                return Ok(JobOutcome::Cancelled);
            }
            std::thread::sleep(step_delay);
            ctx.progress(i + 1, Some(steps), Some(format!("Step {} of {steps}", i + 1)));
        }
        Ok(JobOutcome::Completed(Some(format!("Finished {steps} steps"))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<JobStatus>>);
    impl JobEvents for Recorder {
        fn job_updated(&self, status: &JobStatus) {
            self.0.lock().unwrap().push(status.clone());
        }
    }

    fn wait_until_finished(m: &JobManager, id: JobId) -> JobStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let s = m.list().unwrap().into_iter().find(|s| s.id == id).unwrap();
            if s.state.is_finished() {
                return s;
            }
            assert!(Instant::now() < deadline, "job did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn completes_and_reports_full_progress() {
        let rec = Arc::new(Recorder::default());
        let m = JobManager::new(rec.clone());
        let id = m.spawn(JobKind::SelfTest, "t", self_test(5, Duration::ZERO)).unwrap();
        let s = wait_until_finished(&m, id);
        assert_eq!(s.state, JobState::Completed);
        assert_eq!(s.progress, Progress { done: 5, total: Some(5) });
        let events = rec.0.lock().unwrap();
        assert_eq!(events.first().unwrap().state, JobState::Queued);
        assert_eq!(events.last().unwrap().state, JobState::Completed);
    }

    #[test]
    fn can_be_cancelled() {
        let m = JobManager::new(Arc::new(Recorder::default()));
        let id = m.spawn(JobKind::SelfTest, "t", self_test(10_000, Duration::from_millis(1))).unwrap();
        m.cancel(id).unwrap();
        assert_eq!(wait_until_finished(&m, id).state, JobState::Cancelled);
    }

    #[test]
    fn errors_and_panics_mark_job_failed() {
        let m = JobManager::new(Arc::new(Recorder::default()));
        let err = m.spawn(JobKind::SelfTest, "err", |_| Err(AppError::InvalidInput("bad".into()))).unwrap();
        let s = wait_until_finished(&m, err);
        assert_eq!(s.state, JobState::Failed);
        assert_eq!(s.message.as_deref(), Some("bad"));

        let panics = m.spawn(JobKind::SelfTest, "panic", |_| panic!("boom")).unwrap();
        assert_eq!(wait_until_finished(&m, panics).state, JobState::Failed);
    }

    #[test]
    fn cancel_unknown_job_errors() {
        let m = JobManager::new(Arc::new(Recorder::default()));
        assert!(matches!(m.cancel(42), Err(AppError::JobNotFound(42))));
    }
}
