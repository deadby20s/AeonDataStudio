//! Tauri command handlers. Keep these thin: validate input, call into the core modules,
//! and start a job for anything that may take longer than ~100 ms.

use std::path::PathBuf;
use std::time::Duration;

use aeon_shared::{
    AppInfo, DatasetManifest, DatasetSummary, Document, DocumentPage, ImportRequest, JobId, JobKind, JobStatus,
    WorkspaceInfo,
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::dataset::{self, DatasetReader};
use crate::error::{AppError, AppResult};
use crate::{import, jobs};
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
    // Rebuilding from components normalizes separators (`C:/a/b` → `C:\a\b` on Windows), so
    // the same folder is not listed twice in recent workspaces.
    let path: PathBuf = PathBuf::from(path.trim().trim_matches('"')).components().collect();
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

/// Runs blocking file work off the async runtime's worker threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| AppError::Internal(e.to_string()))?
}

/// Shows a native multi-file picker for data files. Returns an empty list if cancelled.
#[tauri::command]
pub async fn pick_import_files(app: AppHandle) -> AppResult<Vec<String>> {
    let picked = blocking(move || {
        Ok(app
            .dialog()
            .file()
            .set_title("Choose files to import")
            .add_filter(
                "Data files",
                &["jsonl", "ndjson", "json", "txt", "md", "csv", "tsv", "parquet", "gz", "zst"],
            )
            .add_filter("All files", &["*"])
            .blocking_pick_files())
    })
    .await?;
    picked
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.into_path().map(|p| p.display().to_string()).map_err(|e| AppError::InvalidInput(e.to_string())))
        .collect()
}

/// Shows a native folder picker for an import source. Returns `None` if cancelled.
#[tauri::command]
pub async fn pick_import_folder(app: AppHandle) -> AppResult<Option<String>> {
    let picked = blocking(move || Ok(app.dialog().file().set_title("Choose a folder to import").blocking_pick_folder()))
        .await?;
    picked
        .map(|p| p.into_path().map(|p| p.display().to_string()).map_err(|e| AppError::InvalidInput(e.to_string())))
        .transpose()
}

#[tauri::command]
pub fn start_import(request: ImportRequest, state: State<'_, AppState>) -> AppResult<JobId> {
    import::validate(&request)?;
    let ws = state.workspace()?;
    let title = format!("Import \"{}\"", request.name.trim());
    state.jobs.spawn(JobKind::Import, title, move |ctx| import::run(ctx, ws, request))
}

#[tauri::command]
pub async fn get_dataset(dataset_id: String, state: State<'_, AppState>) -> AppResult<DatasetManifest> {
    let dir = state.workspace()?.dataset_dir(&dataset_id)?;
    blocking(move || dataset::read_manifest(&dir)).await
}

#[tauri::command]
pub async fn browse_documents(
    dataset_id: String,
    start_row: u64,
    limit: u64,
    query: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<DocumentPage> {
    let dir = state.workspace()?.dataset_dir(&dataset_id)?;
    blocking(move || DatasetReader::open(&dir)?.browse(start_row, limit, query.as_deref())).await
}

#[tauri::command]
pub async fn get_document(dataset_id: String, row: u64, state: State<'_, AppState>) -> AppResult<Document> {
    let dir = state.workspace()?.dataset_dir(&dataset_id)?;
    blocking(move || DatasetReader::open(&dir)?.document(row)).await
}

/// Removes a dataset from the catalog and deletes its files.
#[tauri::command]
pub async fn delete_dataset(dataset_id: String, state: State<'_, AppState>) -> AppResult<()> {
    let ws = state.workspace()?;
    let dir = ws.dataset_dir(&dataset_id)?;
    if !ws.catalog()?.delete_dataset(&dataset_id)? && !dir.exists() {
        return Err(AppError::DatasetNotFound(dataset_id));
    }
    blocking(move || {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    })
    .await
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
