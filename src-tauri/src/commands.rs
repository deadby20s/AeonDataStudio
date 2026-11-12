//! Tauri command handlers. Keep these thin: validate input, call into the core modules,
//! and start a job for anything that may take longer than ~100 ms.

use std::path::PathBuf;
use std::time::Duration;

use aeon_shared::{AppInfo, DatasetSummary, JobId, JobKind, JobStatus, WorkspaceInfo};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::error::{AppError, AppResult};
use crate::jobs;
use crate::state::AppState;

#[tauri::command]
pub fn get_app_info(app: AppHandle, state: State<'_, AppState>) -> AppResult<AppInfo> {
    let workspace = state.current_workspace().map(|w| w.info()).transpose()?;
    Ok(AppInfo {
        version: app.package_info().version.to_string(),
        workspace,
        recent_workspaces: state.recent_workspaces(),
    })
}

/// Shows a native folder picker. Returns `None` if the user cancels.
#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> AppResult<Option<String>> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title("Choose a workspace folder").blocking_pick_folder()
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    match picked {
        None => Ok(None),
        Some(path) => {
            let path = path.into_path().map_err(|e| AppError::InvalidInput(e.to_string()))?;
            Ok(Some(path.display().to_string()))
        }
    }
}

#[tauri::command]
pub async fn open_workspace(path: String, state: State<'_, AppState>) -> AppResult<WorkspaceInfo> {
    let path = PathBuf::from(path.trim());
    if path.as_os_str().is_empty() {
        return Err(AppError::InvalidInput("No folder given".into()));
    }
    state.open_workspace(&path)?.info()
}

#[tauri::command]
pub fn close_workspace(state: State<'_, AppState>) -> AppResult<()> {
    state.close_workspace()
}

#[tauri::command]
pub fn forget_recent_workspace(path: String, state: State<'_, AppState>) -> AppResult<Vec<String>> {
    state.update_settings(|s| s.forget(&path))?;
    Ok(state.recent_workspaces())
}

#[tauri::command]
pub fn list_datasets(state: State<'_, AppState>) -> AppResult<Vec<DatasetSummary>> {
    state.workspace()?.catalog()?.list_datasets()
}

#[tauri::command]
pub fn list_jobs(state: State<'_, AppState>) -> AppResult<Vec<JobStatus>> {
    state.jobs.list()
}

#[tauri::command]
pub fn cancel_job(job_id: JobId, state: State<'_, AppState>) -> AppResult<()> {
    state.jobs.cancel(job_id)
}

#[tauri::command]
pub fn clear_finished_jobs(state: State<'_, AppState>) -> AppResult<()> {
    state.jobs.clear_finished()
}

/// Starts a diagnostic job that only reports progress, to check the job runner end to end.
#[tauri::command]
pub fn start_self_test_job(steps: u64, state: State<'_, AppState>) -> AppResult<JobId> {
    if !(1..=10_000).contains(&steps) {
        return Err(AppError::InvalidInput("steps must be between 1 and 10000".into()));
    }
    state
        .jobs
        .spawn(JobKind::SelfTest, "Job runner self-test", jobs::self_test(steps, Duration::from_millis(50)))
}
