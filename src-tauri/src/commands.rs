//! Tauri command handlers. Keep these thin: validate input, call into the core modules,
//! and start a job for anything that may take longer than ~100 ms.

use std::path::PathBuf;
use std::time::Duration;

use aeon_shared::{
    AppInfo, ChangedPage, DatasetManifest, ExportRequest, ExportSummary, ImportFormat, ImportPreview, MixRequest,
    TextMode, DatasetSummary, Document, DocumentPage, ImportRequest, JobId, JobKind,
    JobStatus,
    PipelinePreview, PipelineSpec, PipelineSummary, RemovedDoc, RemovedPage, RunReport, RunSummary, WorkspaceInfo,
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::dataset::{self, DatasetReader};
use crate::error::{AppError, AppResult};
use crate::{demo, export, import, jobs, mix, pipeline};
use crate::state::AppState;

#[tauri::command]
pub fn get_app_info(app: AppHandle, state: State<'_, AppState>) -> AppResult<AppInfo> {
    let workspace = state.current_workspace().map(|w| w.info()).transpose()?;
    Ok(AppInfo {
        updates_configured: crate::updates::configured(),
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

// ---------------------------------------------------------------------------------------------
// Pipelines and runs
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn list_pipelines(state: State<'_, AppState>) -> AppResult<Vec<PipelineSummary>> {
    let dir = state.workspace()?.pipelines_dir();
    blocking(move || pipeline::list_pipelines(&dir)).await
}

#[tauri::command]
pub async fn get_pipeline(file: String, state: State<'_, AppState>) -> AppResult<PipelineSpec> {
    let dir = state.workspace()?.pipelines_dir();
    blocking(move || pipeline::read_pipeline(&dir, &file)).await
}

/// Saves a pipeline. Without `file`, a new file is created from the name. Returns the file name.
#[tauri::command]
pub async fn save_pipeline(spec: PipelineSpec, file: Option<String>, state: State<'_, AppState>) -> AppResult<String> {
    let dir = state.workspace()?.pipelines_dir();
    blocking(move || pipeline::save_pipeline(&dir, &spec, file.as_deref())).await
}

#[tauri::command]
pub async fn delete_pipeline(file: String, state: State<'_, AppState>) -> AppResult<()> {
    let dir = state.workspace()?.pipelines_dir();
    blocking(move || pipeline::delete_pipeline(&dir, &file)).await
}

/// Runs the pipeline on a sample of the dataset and returns statistics and examples.
#[tauri::command]
pub async fn preview_pipeline(
    spec: PipelineSpec,
    dataset_id: String,
    sample_size: u64,
    state: State<'_, AppState>,
) -> AppResult<PipelinePreview> {
    let dir = state.workspace()?.dataset_dir(&dataset_id)?;
    // A new preview replaces (cancels) any preview still running.
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Ok(mut slot) = state.preview_cancel.lock() {
        if let Some(old) = slot.replace(flag.clone()) {
            old.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    blocking(move || pipeline::preview(&spec, &DatasetReader::open(&dir)?, sample_size, &flag)).await
}

/// Stops the running preview; it returns a "Cancelled" error.
#[tauri::command]
pub fn cancel_preview(state: State<'_, AppState>) -> AppResult<()> {
    if let Ok(slot) = state.preview_cancel.lock() {
        if let Some(flag) = slot.as_ref() {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn start_pipeline_run(
    spec: PipelineSpec,
    dataset_id: String,
    output_name: String,
    state: State<'_, AppState>,
) -> AppResult<JobId> {
    pipeline::steps_check(&spec)?;
    let ws = state.workspace()?;
    ws.dataset_dir(&dataset_id)?;
    let title = format!("Run \"{}\"", spec.name.trim());
    let req = pipeline::RunRequest { spec, input_dataset_id: dataset_id, output_name };
    state.jobs.spawn(JobKind::PipelineRun, title, move |ctx| pipeline::run(ctx, ws, req))
}

#[tauri::command]
pub fn list_runs(state: State<'_, AppState>) -> AppResult<Vec<RunSummary>> {
    state.workspace()?.catalog()?.list_runs()
}

#[tauri::command]
pub async fn get_run(run_id: String, state: State<'_, AppState>) -> AppResult<RunReport> {
    let ws = state.workspace()?;
    blocking(move || pipeline::read_report(&ws, &run_id)).await
}

#[tauri::command]
pub async fn browse_removed(
    run_id: String,
    start: u64,
    limit: u64,
    step_index: Option<u64>,
    rule: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<RemovedPage> {
    let ws = state.workspace()?;
    blocking(move || {
        let report = pipeline::read_report(&ws, &run_id)?;
        let reader = pipeline::RemovedReader::new(&ws.run_dir(&run_id)?.join("removed"), report.removed_parts);
        reader.browse(start, limit, step_index, rule.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn get_removed(run_id: String, index: u64, state: State<'_, AppState>) -> AppResult<RemovedDoc> {
    let ws = state.workspace()?;
    blocking(move || {
        let report = pipeline::read_report(&ws, &run_id)?;
        pipeline::RemovedReader::new(&ws.run_dir(&run_id)?.join("removed"), report.removed_parts).get(index)
    })
    .await
}

/// Deletes a run's record and its removed-documents log. The output dataset is kept.
#[tauri::command]
pub async fn delete_run(run_id: String, state: State<'_, AppState>) -> AppResult<()> {
    let ws = state.workspace()?;
    let dir = ws.run_dir(&run_id)?;
    {
        let catalog = ws.catalog()?;
        if catalog.list_runs()?.iter().any(|r| r.id == run_id && r.state == "running") {
            return Err(AppError::InvalidInput("This run is still in progress. Cancel it first.".into()));
        }
        catalog.delete_run(&run_id)?;
    }
    blocking(move || {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// Inspector
// ---------------------------------------------------------------------------------------------

/// Random documents removed by one rule of one step. The same `seed` returns the same sample.
#[tauri::command]
pub async fn sample_removed(
    run_id: String,
    step_index: u64,
    rule: String,
    count: u64,
    seed: u64,
    state: State<'_, AppState>,
) -> AppResult<Vec<RemovedDoc>> {
    let ws = state.workspace()?;
    blocking(move || {
        let report = pipeline::read_report(&ws, &run_id)?;
        pipeline::sample_removed(&ws.run_dir(&run_id)?, &report, step_index, &rule, count, seed)
    })
    .await
}

/// Kept documents that normalization changed, for comparing input and output.
#[tauri::command]
pub async fn browse_changed(
    run_id: String,
    step_index: Option<u64>,
    start: u64,
    limit: u64,
    state: State<'_, AppState>,
) -> AppResult<ChangedPage> {
    let ws = state.workspace()?;
    blocking(move || {
        let report = pipeline::read_report(&ws, &run_id)?;
        pipeline::browse_changed(&ws.run_dir(&run_id)?, &report, step_index, start, limit)
    })
    .await
}

// ---------------------------------------------------------------------------------------------
// Export, mixing, import preview
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub fn start_export(request: ExportRequest, app: AppHandle, state: State<'_, AppState>) -> AppResult<JobId> {
    export::validate(&request)?;
    let ws = state.workspace()?;
    ws.dataset_dir(&request.dataset_id)?;
    let version = app.package_info().version.to_string();
    let title = format!("Export \"{}\"", request.name.trim());
    state.jobs.spawn(JobKind::Export, title, move |ctx| export::run(ctx, ws, request, version))
}

#[tauri::command]
pub async fn list_exports(state: State<'_, AppState>) -> AppResult<Vec<ExportSummary>> {
    let ws = state.workspace()?;
    blocking(move || export::list(&ws)).await
}

/// Deletes an export in the workspace, or forgets one made elsewhere (files kept).
#[tauri::command]
pub async fn delete_export(path: String, state: State<'_, AppState>) -> AppResult<()> {
    let ws = state.workspace()?;
    blocking(move || export::delete(&ws, &path)).await
}

/// Opens an export folder in the system file manager.
#[tauri::command]
pub fn open_export_folder(path: String, app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let ws = state.workspace()?;
    let (dir, _) = export::resolve(&ws, &path)?;
    if !dir.is_dir() {
        return Err(AppError::InvalidInput(format!("{} does not exist", dir.display())));
    }
    app.opener()
        .open_path(dir.display().to_string(), None::<&str>)
        .map_err(|e| AppError::Internal(format!("Could not open the folder: {e}")))
}

#[tauri::command]
pub async fn pick_export_folder(app: AppHandle) -> AppResult<Option<String>> {
    let picked = blocking(move || Ok(app.dialog().file().set_title("Choose where to save the export").blocking_pick_folder()))
        .await?;
    picked
        .map(|p| p.into_path().map(|p| p.display().to_string()).map_err(|e| AppError::InvalidInput(e.to_string())))
        .transpose()
}

#[tauri::command]
pub async fn pick_tokenizer_file(app: AppHandle) -> AppResult<Option<String>> {
    let picked = blocking(move || {
        Ok(app
            .dialog()
            .file()
            .set_title("Choose a Hugging Face tokenizer.json")
            .add_filter("Tokenizer", &["json"])
            .blocking_pick_file())
    })
    .await?;
    picked
        .map(|p| p.into_path().map(|p| p.display().to_string()).map_err(|e| AppError::InvalidInput(e.to_string())))
        .transpose()
}

#[tauri::command]
pub fn start_mix(request: MixRequest, state: State<'_, AppState>) -> AppResult<JobId> {
    mix::validate(&request)?;
    let ws = state.workspace()?;
    for s in &request.sources {
        ws.dataset_dir(&s.dataset_id)?;
    }
    let title = format!("Mix \"{}\"", request.name.trim());
    state.jobs.spawn(JobKind::Mix, title, move |ctx| mix::run(ctx, ws, request))
}

/// Reads the first records of the first file, to show its fields before importing.
#[tauri::command]
pub async fn preview_import(
    paths: Vec<String>,
    format: ImportFormat,
    text_field: String,
    text_mode: TextMode,
) -> AppResult<ImportPreview> {
    blocking(move || import::preview(&paths, format, &text_field, text_mode)).await
}

// ---------------------------------------------------------------------------------------------
// Demo data and logs
// ---------------------------------------------------------------------------------------------

/// Generates the demo dataset (as an import job) and saves a "Demo cleanup" pipeline.
#[tauri::command]
pub fn create_demo_dataset(state: State<'_, AppState>) -> AppResult<JobId> {
    let ws = state.workspace()?;
    // The template recipe, so testers can run it straight away. Keep an existing one.
    let _ = pipeline::save_pipeline(&ws.pipelines_dir(), &aeon_shared::PipelineSpec::template("Demo cleanup"), None);
    state.jobs.spawn(JobKind::Import, format!("Import \"{}\"", demo::DATASET_NAME), move |ctx| {
        ctx.progress(0, None, Some("Generating demo documents…".into()));
        let file = std::env::temp_dir().join(format!("aeondatastudio-demo-{}.jsonl", ctx_id_suffix()));
        demo::write_jsonl(&file)?;
        let req = ImportRequest {
            name: demo::DATASET_NAME.into(),
            description: Some("Generated sample with planted problems; each document's meta.category says what it is.".into()),
            paths: vec![file.display().to_string()],
            format: ImportFormat::Jsonl,
            text_field: "text".into(),
            id_field: Some("id".into()),
            text_mode: TextMode::WholeFile,
        };
        let result = import::run(ctx, ws, req);
        let _ = std::fs::remove_file(&file);
        result
    })
}

fn ctx_id_suffix() -> String {
    crate::error::now_ms().to_string()
}

/// Opens the folder with AeonDataStudio.log in the system file manager.
#[tauri::command]
pub fn open_log_folder(app: AppHandle) -> AppResult<String> {
    use tauri::Manager;
    use tauri_plugin_opener::OpenerExt;
    let dir = app.path().app_log_dir()?;
    std::fs::create_dir_all(&dir)?;
    app.opener()
        .open_path(dir.display().to_string(), None::<&str>)
        .map_err(|e| AppError::Internal(format!("Could not open the folder: {e}")))?;
    Ok(dir.display().to_string())
}

// ---------------------------------------------------------------------------------------------
// Updates
// ---------------------------------------------------------------------------------------------

/// `Ok(None)` when up to date. Errors (including "not set up in this build") are shown as-is.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> AppResult<Option<aeon_shared::UpdateInfo>> {
    crate::updates::check(&app).await
}

/// Downloads, installs and restarts into the new version.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> AppResult<()> {
    crate::updates::install(&app).await
}
