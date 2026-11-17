//! Tauri command handlers: thin wrappers over [`aeon_core::state::AppState`] (see
//! `core/src/ops.rs`), plus what only the desktop app has: native dialogs, opening folders,
//! updates, themes and the local API switch.

use std::sync::Arc;

use aeon_core::state::AppState;
use aeon_core::{AppError, AppResult};
use aeon_shared::{
    ApiStatus, AppInfo, ChangedPage, DatasetManifest, DatasetSummary, Document, DocumentPage, ExportRequest,
    ExportSummary, ImportFormat, ImportPreview, ImportRequest, JobId, JobStatus, MixRequest, PipelinePreview,
    PipelineSpec, PipelineSummary, PluginInfo, RemovedDoc, RemovedPage, RunReport, RunSummary, TextMode,
    ThemeSettings, WorkspaceInfo,
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::{Api, Studio};

type St<'a> = State<'a, Studio>;

/// Runs blocking engine work off the async runtime's worker threads.
async fn blocking<T: Send + 'static>(
    state: &St<'_>,
    f: impl FnOnce(&AppState) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let s = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || f(&s)).await.map_err(|e| AppError::Internal(e.to_string()))?
}

fn path_of(p: tauri_plugin_dialog::FilePath) -> AppResult<String> {
    p.into_path().map(|p| p.display().to_string()).map_err(|e| AppError::InvalidInput(e.to_string()))
}

async fn dialog<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| AppError::Internal(e.to_string()))
}

#[tauri::command]
pub fn get_app_info(state: St<'_>) -> AppResult<AppInfo> {
    let mut info = state.0.app_info()?;
    info.updates_configured = crate::updates::configured();
    Ok(info)
}

/// Shows a native folder picker. Returns `None` if the user cancels.
#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> AppResult<Option<String>> {
    dialog(move || app.dialog().file().set_title("Choose a workspace folder").blocking_pick_folder())
        .await?
        .map(path_of)
        .transpose()
}

#[tauri::command]
pub async fn open_workspace(path: String, state: St<'_>) -> AppResult<WorkspaceInfo> {
    blocking(&state, move |s| s.open_workspace_path(&path)).await
}

#[tauri::command]
pub fn close_workspace(state: St<'_>) -> AppResult<()> {
    state.0.close_workspace()
}

#[tauri::command]
pub fn forget_recent_workspace(path: String, state: St<'_>) -> AppResult<Vec<String>> {
    state.0.update_settings(|s| s.forget(&path))?;
    Ok(state.0.recent_workspaces())
}

#[tauri::command]
pub fn list_datasets(state: St<'_>) -> AppResult<Vec<DatasetSummary>> {
    state.0.list_datasets()
}

/// Shows a native multi-file picker for data files. Returns an empty list if cancelled.
#[tauri::command]
pub async fn pick_import_files(app: AppHandle) -> AppResult<Vec<String>> {
    dialog(move || {
        app.dialog()
            .file()
            .set_title("Choose files to import")
            .add_filter(
                "Data files",
                &["jsonl", "ndjson", "json", "txt", "md", "csv", "tsv", "parquet", "html", "htm", "warc", "gz", "zst"],
            )
            .add_filter("All files", &["*"])
            .blocking_pick_files()
    })
    .await?
    .unwrap_or_default()
    .into_iter()
    .map(path_of)
    .collect()
}

/// Shows a native folder picker for an import source. Returns `None` if cancelled.
#[tauri::command]
pub async fn pick_import_folder(app: AppHandle) -> AppResult<Option<String>> {
    dialog(move || app.dialog().file().set_title("Choose a folder to import").blocking_pick_folder())
        .await?
        .map(path_of)
        .transpose()
}

#[tauri::command]
pub fn start_import(request: ImportRequest, state: St<'_>) -> AppResult<JobId> {
    state.0.start_import(request)
}

#[tauri::command]
pub async fn get_dataset(dataset_id: String, state: St<'_>) -> AppResult<DatasetManifest> {
    blocking(&state, move |s| s.get_dataset(&dataset_id)).await
}

#[tauri::command]
pub async fn browse_documents(
    dataset_id: String,
    start_row: u64,
    limit: u64,
    query: Option<String>,
    state: St<'_>,
) -> AppResult<DocumentPage> {
    blocking(&state, move |s| s.browse_documents(&dataset_id, start_row, limit, query.as_deref())).await
}

#[tauri::command]
pub async fn get_document(dataset_id: String, row: u64, state: St<'_>) -> AppResult<Document> {
    blocking(&state, move |s| s.get_document(&dataset_id, row)).await
}

/// Removes a dataset from the catalog and deletes its files.
#[tauri::command]
pub async fn delete_dataset(dataset_id: String, state: St<'_>) -> AppResult<()> {
    blocking(&state, move |s| s.delete_dataset(&dataset_id)).await
}

#[tauri::command]
pub fn list_jobs(state: St<'_>) -> AppResult<Vec<JobStatus>> {
    state.0.list_jobs()
}

#[tauri::command]
pub fn cancel_job(job_id: JobId, state: St<'_>) -> AppResult<()> {
    state.0.cancel_job(job_id)
}

#[tauri::command]
pub fn clear_finished_jobs(state: St<'_>) -> AppResult<()> {
    state.0.clear_finished_jobs()
}

/// Starts a diagnostic job that only reports progress, to check the job runner end to end.
#[tauri::command]
pub fn start_self_test_job(steps: u64, state: St<'_>) -> AppResult<JobId> {
    state.0.start_self_test_job(steps)
}

// ---------------------------------------------------------------------------------------------
// Pipelines and runs
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn list_pipelines(state: St<'_>) -> AppResult<Vec<PipelineSummary>> {
    blocking(&state, |s| s.list_pipelines()).await
}

#[tauri::command]
pub async fn get_pipeline(file: String, state: St<'_>) -> AppResult<PipelineSpec> {
    blocking(&state, move |s| s.get_pipeline(&file)).await
}

/// Saves a pipeline. Without `file`, a new file is created from the name. Returns the file name.
#[tauri::command]
pub async fn save_pipeline(spec: PipelineSpec, file: Option<String>, state: St<'_>) -> AppResult<String> {
    blocking(&state, move |s| s.save_pipeline(&spec, file.as_deref())).await
}

#[tauri::command]
pub async fn delete_pipeline(file: String, state: St<'_>) -> AppResult<()> {
    blocking(&state, move |s| s.delete_pipeline(&file)).await
}

/// Runs the pipeline on a sample of the dataset and returns statistics and examples.
#[tauri::command]
pub async fn preview_pipeline(
    spec: PipelineSpec,
    dataset_id: String,
    sample_size: u64,
    state: St<'_>,
) -> AppResult<PipelinePreview> {
    blocking(&state, move |s| s.preview_pipeline(&spec, &dataset_id, sample_size)).await
}

/// Stops the running preview; it returns a "Cancelled" error.
#[tauri::command]
pub fn cancel_preview(state: St<'_>) {
    state.0.cancel_preview();
}

#[tauri::command]
pub async fn start_pipeline_run(
    spec: PipelineSpec,
    dataset_id: String,
    output_name: String,
    state: St<'_>,
) -> AppResult<JobId> {
    // Off the main thread: checking a recipe starts its plugins once.
    blocking(&state, move |s| s.start_pipeline_run(spec, &dataset_id, &output_name)).await
}

#[tauri::command]
pub fn list_runs(state: St<'_>) -> AppResult<Vec<RunSummary>> {
    state.0.list_runs()
}

#[tauri::command]
pub async fn get_run(run_id: String, state: St<'_>) -> AppResult<RunReport> {
    blocking(&state, move |s| s.get_run(&run_id)).await
}

#[tauri::command]
pub async fn browse_removed(
    run_id: String,
    start: u64,
    limit: u64,
    step_index: Option<u64>,
    rule: Option<String>,
    state: St<'_>,
) -> AppResult<RemovedPage> {
    blocking(&state, move |s| s.browse_removed(&run_id, start, limit, step_index, rule.as_deref())).await
}

#[tauri::command]
pub async fn get_removed(run_id: String, index: u64, state: St<'_>) -> AppResult<RemovedDoc> {
    blocking(&state, move |s| s.get_removed(&run_id, index)).await
}

/// Deletes a run's record and its removed-documents log. The output dataset is kept.
#[tauri::command]
pub async fn delete_run(run_id: String, state: St<'_>) -> AppResult<()> {
    blocking(&state, move |s| s.delete_run(&run_id)).await
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
    state: St<'_>,
) -> AppResult<Vec<RemovedDoc>> {
    blocking(&state, move |s| s.sample_removed(&run_id, step_index, &rule, count, seed)).await
}

/// Kept documents that normalization changed, for comparing input and output.
#[tauri::command]
pub async fn browse_changed(
    run_id: String,
    step_index: Option<u64>,
    start: u64,
    limit: u64,
    state: St<'_>,
) -> AppResult<ChangedPage> {
    blocking(&state, move |s| s.browse_changed(&run_id, step_index, start, limit)).await
}

// ---------------------------------------------------------------------------------------------
// Export, mixing, import preview
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub fn start_export(request: ExportRequest, state: St<'_>) -> AppResult<JobId> {
    state.0.start_export(request)
}

#[tauri::command]
pub async fn list_exports(state: St<'_>) -> AppResult<Vec<ExportSummary>> {
    blocking(&state, |s| s.list_exports()).await
}

/// Deletes an export in the workspace, or forgets one made elsewhere (files kept).
#[tauri::command]
pub async fn delete_export(path: String, state: St<'_>) -> AppResult<()> {
    blocking(&state, move |s| s.delete_export(&path)).await
}

fn open_path(app: &AppHandle, dir: &std::path::Path) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(dir.display().to_string(), None::<&str>)
        .map_err(|e| AppError::Internal(format!("Could not open the folder: {e}")))
}

/// Opens an export folder in the system file manager.
#[tauri::command]
pub fn open_export_folder(path: String, app: AppHandle, state: St<'_>) -> AppResult<()> {
    open_path(&app, &state.0.export_folder(&path)?)
}

#[tauri::command]
pub async fn pick_export_folder(app: AppHandle) -> AppResult<Option<String>> {
    dialog(move || app.dialog().file().set_title("Choose where to save the export").blocking_pick_folder())
        .await?
        .map(path_of)
        .transpose()
}

#[tauri::command]
pub async fn pick_tokenizer_file(app: AppHandle) -> AppResult<Option<String>> {
    dialog(move || {
        app.dialog()
            .file()
            .set_title("Choose a Hugging Face tokenizer.json")
            .add_filter("Tokenizer", &["json"])
            .blocking_pick_file()
    })
    .await?
    .map(path_of)
    .transpose()
}

#[tauri::command]
pub fn start_mix(request: MixRequest, state: St<'_>) -> AppResult<JobId> {
    state.0.start_mix(request)
}

/// Reads the first records of the first file, to show its fields before importing.
#[tauri::command]
pub async fn preview_import(
    paths: Vec<String>,
    format: ImportFormat,
    text_field: String,
    text_mode: TextMode,
    state: St<'_>,
) -> AppResult<ImportPreview> {
    blocking(&state, move |s| s.preview_import(&paths, format, &text_field, text_mode)).await
}

// ---------------------------------------------------------------------------------------------
// Demo data and logs
// ---------------------------------------------------------------------------------------------

/// Generates the demo dataset (as an import job) and saves a "Demo cleanup" pipeline.
#[tauri::command]
pub fn create_demo_dataset(state: St<'_>) -> AppResult<JobId> {
    state.0.create_demo_dataset()
}

/// Opens the folder with AeonDataStudio.log in the system file manager.
#[tauri::command]
pub fn open_log_folder(app: AppHandle) -> AppResult<String> {
    use tauri::Manager;
    let dir = app.path().app_log_dir().map_err(|e| AppError::Internal(e.to_string()))?;
    std::fs::create_dir_all(&dir)?;
    open_path(&app, &dir)?;
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

// ---------------------------------------------------------------------------------------------
// SQL explorer
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn sql_tables(state: St<'_>) -> AppResult<Vec<aeon_shared::SqlTable>> {
    blocking(&state, |s| s.sql_tables()).await
}

/// Runs a SQL query over the workspace's datasets and run logs (at most 1,000 rows returned).
#[tauri::command]
pub async fn run_sql(sql: String, limit: u64, state: St<'_>) -> AppResult<aeon_shared::SqlResult> {
    blocking(&state, move |s| s.run_sql(&sql, limit)).await
}

/// Parses and binds a query without running it, for live checking while typing.
#[tauri::command]
pub async fn check_sql(sql: String, state: St<'_>) -> AppResult<aeon_shared::SqlCheck> {
    blocking(&state, move |s| s.check_sql(&sql)).await
}

#[tauri::command]
pub fn cancel_sql(state: St<'_>) {
    state.0.cancel_sql();
}

// ---------------------------------------------------------------------------------------------
// Plugins
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn list_plugins(state: St<'_>) -> AppResult<Vec<PluginInfo>> {
    blocking(&state, |s| aeon_core::plugins::list_plugins(&s.workspace()?.plugins_dir())).await
}

#[tauri::command]
pub fn open_plugins_folder(app: AppHandle, state: St<'_>) -> AppResult<()> {
    let dir = state.0.workspace()?.plugins_dir();
    std::fs::create_dir_all(&dir)?;
    open_path(&app, &dir)
}

// ---------------------------------------------------------------------------------------------
// Themes
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub fn get_theme_settings(state: St<'_>) -> ThemeSettings {
    state.0.settings().theme
}

#[tauri::command]
pub fn set_theme_settings(theme: ThemeSettings, state: St<'_>) -> AppResult<()> {
    for t in &theme.custom {
        if t.name.trim().is_empty() {
            return Err(AppError::InvalidInput("A theme needs a name".into()));
        }
        // Only custom properties, so a theme can't inject other CSS.
        if let Some((k, _)) = t.vars.iter().find(|(k, v)| {
            !k.starts_with("--")
                || !k[2..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                || v.contains([';', '{', '}', '<', '>'])
        }) {
            return Err(AppError::InvalidInput(format!("Theme \"{}\": invalid variable {k:?}", t.name)));
        }
    }
    state.0.update_settings(|s| s.theme = theme)
}

// ---------------------------------------------------------------------------------------------
// Local API
// ---------------------------------------------------------------------------------------------

/// Starts the API with the saved port and token (creating the token on first use).
pub fn start_api_server(state: &Arc<AppState>, api: &Api) {
    let mut settings = state.settings().api;
    if settings.token.is_empty() {
        settings.token = aeon_core::api::new_token();
        let token = settings.token.clone();
        if let Err(e) = state.update_settings(|s| s.api.token = token) {
            log::warn!("could not save the API token: {e}");
        }
    }
    let result = aeon_core::api::ApiServer::start(Arc::clone(state), settings.port, settings.token);
    let (server, error) = match result {
        Ok(s) => (Some(s), None),
        Err(e) => {
            log::warn!("local API: {e}");
            (None, Some(e.to_string()))
        }
    };
    if let Ok(mut slot) = api.server.lock() {
        *slot = server;
    }
    if let Ok(mut slot) = api.error.lock() {
        *slot = error;
    }
}

fn stop_api_server(api: &Api) {
    let server = api.server.lock().ok().and_then(|mut s| s.take());
    if let Some(server) = server {
        server.stop();
    }
}

#[tauri::command]
pub fn api_status(api: State<'_, Api>) -> ApiStatus {
    let server = api.server.lock().ok();
    let running = server.as_ref().and_then(|s| s.as_ref()).map(|s| (s.url.clone(), s.token.clone()));
    ApiStatus {
        running: running.is_some(),
        url: running.as_ref().map(|(u, _)| u.clone()),
        token: running.map(|(_, t)| t),
        error: api.error.lock().ok().and_then(|e| e.clone()),
    }
}

/// Turns the local API on or off (remembered for the next start). `port` changes the port.
#[tauri::command]
pub async fn set_api_enabled(enabled: bool, port: Option<u16>, state: St<'_>, api: State<'_, Api>) -> AppResult<ApiStatus> {
    state.0.update_settings(|s| {
        s.api.enabled = enabled;
        if let Some(p) = port.filter(|p| *p >= 1024) {
            s.api.port = p;
        }
    })?;
    stop_api_server(&api);
    if enabled {
        start_api_server(&state.0, &api);
    } else if let Ok(mut e) = api.error.lock() {
        *e = None;
    }
    Ok(api_status(api))
}

/// Replaces the API token (old clients stop working) and restarts the API if it runs.
#[tauri::command]
pub async fn new_api_token(state: St<'_>, api: State<'_, Api>) -> AppResult<ApiStatus> {
    let token = aeon_core::api::new_token();
    state.0.update_settings(|s| s.api.token = token)?;
    let running = api.server.lock().map(|s| s.is_some()).unwrap_or(false);
    if running {
        stop_api_server(&api);
        start_api_server(&state.0, &api);
    }
    Ok(api_status(api))
}
