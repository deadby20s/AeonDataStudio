//! Types exchanged between the frontend (WASM) and the backend (Tauri).
//!
//! Keep this crate free of platform-specific dependencies: it must compile for both
//! `wasm32-unknown-unknown` and the native target.

use serde::{Deserialize, Serialize};

/// Name of the Tauri event emitted whenever a job's status changes.
/// The payload is a [`JobStatus`].
pub const JOB_UPDATE_EVENT: &str = "job://update";

/// Startup information for the frontend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    pub version: String,
    pub workspace: Option<WorkspaceInfo>,
    /// Recently opened workspace folders, most recent first.
    pub recent_workspaces: Vec<String>,
}

/// The currently open workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub name: String,
    pub path: String,
    pub created_at_ms: i64,
    pub dataset_count: u64,
    pub run_count: u64,
}

/// One row of the dataset catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at_ms: i64,
    pub doc_count: Option<u64>,
    pub byte_size: Option<u64>,
    /// The run that produced this dataset, if it was not imported directly.
    pub parent_run_id: Option<String>,
}

pub type JobId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobKind {
    /// Diagnostic job that only reports progress; used to check the job runner.
    SelfTest,
    Import,
    PipelineRun,
    Export,
}

impl JobKind {
    pub fn label(self) -> &'static str {
        match self {
            JobKind::SelfTest => "Self-test",
            JobKind::Import => "Import",
            JobKind::PipelineRun => "Pipeline run",
            JobKind::Export => "Export",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn is_finished(self) -> bool {
        matches!(self, JobState::Completed | JobState::Failed | JobState::Cancelled)
    }

    pub fn label(self) -> &'static str {
        match self {
            JobState::Queued => "Queued",
            JobState::Running => "Running",
            JobState::Completed => "Completed",
            JobState::Failed => "Failed",
            JobState::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Progress {
    pub done: u64,
    /// `None` when the total amount of work is not known yet.
    pub total: Option<u64>,
}

impl Progress {
    /// Fraction complete in `0.0..=1.0`, or `None` if the total is unknown.
    pub fn fraction(&self) -> Option<f64> {
        match self.total {
            Some(0) => Some(1.0),
            Some(total) => Some((self.done as f64 / total as f64).clamp(0.0, 1.0)),
            None => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobStatus {
    pub id: JobId,
    pub kind: JobKind,
    pub title: String,
    pub state: JobState,
    pub progress: Progress,
    /// Latest human-readable status line, or the error message for failed jobs.
    pub message: Option<String>,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}
