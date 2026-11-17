//! Everything the app can do, as plain blocking methods on [`AppState`].
//!
//! The desktop app's commands, the `aeon` CLI, the local HTTP API and the MCP server all call
//! these, so the four stay in step. Anything that may take long starts a background job and
//! returns its id; [`AppState::wait_job`] waits for it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aeon_shared::{
    AppInfo, ChangedPage, DatasetManifest, DatasetSummary, Document, DocumentPage, ExportRequest, ExportSummary,
    ImportFormat, ImportPreview, ImportRequest, JobId, JobKind, JobStatus, MixRequest, PipelinePreview, PipelineSpec,
    PipelineSummary, RemovedDoc, RemovedPage, RunReport, RunSummary, SqlResult, SqlTable, TextMode, WorkspaceInfo,
};

use crate::dataset::{self, DatasetReader};
use crate::error::{AppError, AppResult, now_ms};
use crate::state::AppState;
use crate::{demo, export, import, jobs, mix, pipeline};

impl AppState {
    /// App and workspace information. `updates_configured` is only known to the desktop app.
    pub fn app_info(&self) -> AppResult<AppInfo> {
        let workspace = self.current_workspace().map(|w| w.info()).transpose()?;
        Ok(AppInfo {
            updates_configured: false,
            sql_available: crate::sql::AVAILABLE,
            version: self.version.clone(),
            workspace,
            recent_workspaces: self.recent_workspaces(),
        })
    }

    /// Opens (or creates, in an empty folder) the workspace at `path`.
    pub fn open_workspace_path(&self, path: &str) -> AppResult<WorkspaceInfo> {
        // Rebuilding from components normalizes separators (`C:/a/b` → `C:\a\b` on Windows), so
        // the same folder is not listed twice in recent workspaces.
        let path: PathBuf = PathBuf::from(path.trim().trim_matches('"')).components().collect();
        if path.as_os_str().is_empty() {
            return Err(AppError::InvalidInput("No folder given".into()));
        }
        self.open_workspace(&path)?.info()
    }

    // -----------------------------------------------------------------------------------------
    // Datasets
    // -----------------------------------------------------------------------------------------

    pub fn list_datasets(&self) -> AppResult<Vec<DatasetSummary>> {
        self.workspace()?.catalog()?.list_datasets()
    }

    /// Finds a dataset by id, or by name when exactly one dataset has that name.
    pub fn resolve_dataset(&self, id_or_name: &str) -> AppResult<String> {
        let list = self.list_datasets()?;
        if list.iter().any(|d| d.id == id_or_name) {
            return Ok(id_or_name.to_string());
        }
        let named: Vec<_> = list.iter().filter(|d| d.name == id_or_name).collect();
        match named.as_slice() {
            [one] => Ok(one.id.clone()),
            [] => Err(AppError::DatasetNotFound(id_or_name.to_string())),
            _ => Err(AppError::InvalidInput(format!("Several datasets are named \"{id_or_name}\"; use the id"))),
        }
    }

    pub fn start_import(&self, request: ImportRequest) -> AppResult<JobId> {
        import::validate(&request)?;
        let ws = self.workspace()?;
        let title = format!("Import \"{}\"", request.name.trim());
        self.jobs.spawn(JobKind::Import, title, move |ctx| import::run(ctx, ws, request))
    }

    pub fn preview_import(
        &self,
        paths: &[String],
        format: ImportFormat,
        text_field: &str,
        text_mode: TextMode,
    ) -> AppResult<ImportPreview> {
        import::preview(paths, format, text_field, text_mode)
    }

    pub fn get_dataset(&self, dataset_id: &str) -> AppResult<DatasetManifest> {
        dataset::read_manifest(&self.workspace()?.dataset_dir(dataset_id)?)
    }

    pub fn browse_documents(
        &self,
        dataset_id: &str,
        start_row: u64,
        limit: u64,
        query: Option<&str>,
    ) -> AppResult<DocumentPage> {
        DatasetReader::open(&self.workspace()?.dataset_dir(dataset_id)?)?.browse(start_row, limit, query)
    }

    pub fn get_document(&self, dataset_id: &str, row: u64) -> AppResult<Document> {
        DatasetReader::open(&self.workspace()?.dataset_dir(dataset_id)?)?.document(row)
    }

    /// Removes a dataset from the catalog and deletes its files.
    pub fn delete_dataset(&self, dataset_id: &str) -> AppResult<()> {
        let ws = self.workspace()?;
        let dir = ws.dataset_dir(dataset_id)?;
        if !ws.catalog()?.delete_dataset(dataset_id)? && !dir.exists() {
            return Err(AppError::DatasetNotFound(dataset_id.to_string()));
        }
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    /// Generates the demo dataset (as an import job) and saves a "Demo cleanup" pipeline.
    pub fn create_demo_dataset(&self) -> AppResult<JobId> {
        let ws = self.workspace()?;
        // The template recipe, so testers can run it straight away. Keep an existing one.
        let _ = pipeline::save_pipeline(&ws.pipelines_dir(), &PipelineSpec::template("Demo cleanup"), None);
        self.jobs.spawn(JobKind::Import, format!("Import \"{}\"", demo::DATASET_NAME), move |ctx| {
            ctx.progress(0, None, Some("Generating demo documents…".into()));
            let file = std::env::temp_dir().join(format!("aeondatastudio-demo-{}.jsonl", now_ms()));
            demo::write_jsonl(&file)?;
            let req = ImportRequest {
                name: demo::DATASET_NAME.into(),
                description: Some(
                    "Generated sample with planted problems; each document's meta.category says what it is.".into(),
                ),
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

    // -----------------------------------------------------------------------------------------
    // Pipelines and runs
    // -----------------------------------------------------------------------------------------

    pub fn list_pipelines(&self) -> AppResult<Vec<PipelineSummary>> {
        pipeline::list_pipelines(&self.workspace()?.pipelines_dir())
    }

    pub fn get_pipeline(&self, file: &str) -> AppResult<PipelineSpec> {
        pipeline::read_pipeline(&self.workspace()?.pipelines_dir(), file)
    }

    /// Saves a pipeline. Without `file`, a new file is created from the name. Returns the file name.
    pub fn save_pipeline(&self, spec: &PipelineSpec, file: Option<&str>) -> AppResult<String> {
        pipeline::save_pipeline(&self.workspace()?.pipelines_dir(), spec, file)
    }

    pub fn delete_pipeline(&self, file: &str) -> AppResult<()> {
        pipeline::delete_pipeline(&self.workspace()?.pipelines_dir(), file)
    }

    /// Runs the pipeline on a sample of the dataset. A new preview cancels one still running.
    pub fn preview_pipeline(&self, spec: &PipelineSpec, dataset_id: &str, sample_size: u64) -> AppResult<PipelinePreview> {
        let ws = self.workspace()?;
        let dir = ws.dataset_dir(dataset_id)?;
        let flag = Arc::new(AtomicBool::new(false));
        if let Ok(mut slot) = self.preview_cancel.lock()
            && let Some(old) = slot.replace(flag.clone())
        {
            old.store(true, Ordering::Relaxed);
        }
        pipeline::preview(spec, &ws.step_env(), &DatasetReader::open(&dir)?, sample_size, &flag)
    }

    /// Stops the running preview; it returns a "Cancelled" error.
    pub fn cancel_preview(&self) {
        if let Ok(slot) = self.preview_cancel.lock()
            && let Some(flag) = slot.as_ref()
        {
            flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn start_pipeline_run(&self, spec: PipelineSpec, dataset_id: &str, output_name: &str) -> AppResult<JobId> {
        let ws = self.workspace()?;
        pipeline::steps_check(&spec, &ws.step_env())?;
        ws.dataset_dir(dataset_id)?;
        let title = format!("Run \"{}\"", spec.name.trim());
        let req = pipeline::RunRequest {
            spec,
            input_dataset_id: dataset_id.to_string(),
            output_name: output_name.to_string(),
        };
        self.jobs.spawn(JobKind::PipelineRun, title, move |ctx| pipeline::run(ctx, ws, req))
    }

    pub fn list_runs(&self) -> AppResult<Vec<RunSummary>> {
        self.workspace()?.catalog()?.list_runs()
    }

    pub fn get_run(&self, run_id: &str) -> AppResult<RunReport> {
        pipeline::read_report(&*self.workspace()?, run_id)
    }

    pub fn browse_removed(
        &self,
        run_id: &str,
        start: u64,
        limit: u64,
        step_index: Option<u64>,
        rule: Option<&str>,
    ) -> AppResult<RemovedPage> {
        let ws = self.workspace()?;
        let report = pipeline::read_report(&ws, run_id)?;
        pipeline::RemovedReader::new(&ws.run_dir(run_id)?.join("removed"), report.removed_parts)
            .browse(start, limit, step_index, rule)
    }

    pub fn get_removed(&self, run_id: &str, index: u64) -> AppResult<RemovedDoc> {
        let ws = self.workspace()?;
        let report = pipeline::read_report(&ws, run_id)?;
        pipeline::RemovedReader::new(&ws.run_dir(run_id)?.join("removed"), report.removed_parts).get(index)
    }

    /// Deletes a run's record and its removed-documents log. The output dataset is kept.
    pub fn delete_run(&self, run_id: &str) -> AppResult<()> {
        let ws = self.workspace()?;
        let dir = ws.run_dir(run_id)?;
        {
            let catalog = ws.catalog()?;
            if catalog.list_runs()?.iter().any(|r| r.id == run_id && r.state == "running") {
                return Err(AppError::InvalidInput("This run is still in progress. Cancel it first.".into()));
            }
            catalog.delete_run(run_id)?;
        }
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    /// Random documents removed by one rule of one step. The same `seed` returns the same sample.
    pub fn sample_removed(
        &self,
        run_id: &str,
        step_index: u64,
        rule: &str,
        count: u64,
        seed: u64,
    ) -> AppResult<Vec<RemovedDoc>> {
        let ws = self.workspace()?;
        let report = pipeline::read_report(&ws, run_id)?;
        pipeline::sample_removed(&ws.run_dir(run_id)?, &report, step_index, rule, count, seed)
    }

    /// Kept documents that normalization changed, for comparing input and output.
    pub fn browse_changed(&self, run_id: &str, step_index: Option<u64>, start: u64, limit: u64) -> AppResult<ChangedPage> {
        let ws = self.workspace()?;
        let report = pipeline::read_report(&ws, run_id)?;
        pipeline::browse_changed(&ws.run_dir(run_id)?, &report, step_index, start, limit)
    }

    // -----------------------------------------------------------------------------------------
    // Export and mixing
    // -----------------------------------------------------------------------------------------

    pub fn start_export(&self, request: ExportRequest) -> AppResult<JobId> {
        export::validate(&request)?;
        let ws = self.workspace()?;
        ws.dataset_dir(&request.dataset_id)?;
        let version = self.version.clone();
        let title = format!("Export \"{}\"", request.name.trim());
        self.jobs.spawn(JobKind::Export, title, move |ctx| export::run(ctx, ws, request, version))
    }

    pub fn list_exports(&self) -> AppResult<Vec<ExportSummary>> {
        export::list(&*self.workspace()?)
    }

    /// Deletes an export in the workspace, or forgets one made elsewhere (files kept).
    pub fn delete_export(&self, path: &str) -> AppResult<()> {
        export::delete(&*self.workspace()?, path)
    }

    /// The folder of an export, checked to exist.
    pub fn export_folder(&self, path: &str) -> AppResult<PathBuf> {
        let (dir, _) = export::resolve(&*self.workspace()?, path)?;
        if !dir.is_dir() {
            return Err(AppError::InvalidInput(format!("{} does not exist", dir.display())));
        }
        Ok(dir)
    }

    pub fn start_mix(&self, request: MixRequest) -> AppResult<JobId> {
        mix::validate(&request)?;
        let ws = self.workspace()?;
        for s in &request.sources {
            ws.dataset_dir(&s.dataset_id)?;
        }
        let title = format!("Mix \"{}\"", request.name.trim());
        self.jobs.spawn(JobKind::Mix, title, move |ctx| mix::run(ctx, ws, request))
    }

    // -----------------------------------------------------------------------------------------
    // SQL
    // -----------------------------------------------------------------------------------------

    pub fn sql_tables(&self) -> AppResult<Vec<SqlTable>> {
        crate::sql::tables(&*self.workspace()?)
    }

    /// Runs a SQL query over the workspace's datasets and run logs (at most 1,000 rows returned).
    /// [`AppState::cancel_sql`] interrupts it.
    pub fn run_sql(&self, sql: &str, limit: u64) -> AppResult<SqlResult> {
        let ws = self.workspace()?;
        let limit = limit.clamp(1, 1000) as usize;
        #[cfg(feature = "sql")]
        {
            let result = crate::sql::run(&ws, sql, limit, |handle| {
                if let Ok(mut s) = self.sql_cancel.lock() {
                    *s = Some(Box::new(move || handle.interrupt()));
                }
            });
            if let Ok(mut s) = self.sql_cancel.lock() {
                *s = None;
            }
            result.map_err(|e| match e {
                AppError::InvalidInput(m) if m.contains("nterrupt") || m.contains("INTERRUPT") => AppError::Cancelled,
                other => other,
            })
        }
        #[cfg(not(feature = "sql"))]
        {
            crate::sql::run(&ws, sql, limit, |_| {})
        }
    }

    /// Parses and binds a query without running it (for live checking in the editor).
    pub fn check_sql(&self, sql: &str) -> AppResult<aeon_shared::SqlCheck> {
        let tables = crate::sql::tables(&*self.workspace()?)?;
        self.sql_checker.check(&tables, sql)
    }

    pub fn cancel_sql(&self) {
        if let Ok(s) = self.sql_cancel.lock()
            && let Some(cancel) = s.as_ref()
        {
            cancel();
        }
    }

    // -----------------------------------------------------------------------------------------
    // Jobs
    // -----------------------------------------------------------------------------------------

    pub fn list_jobs(&self) -> AppResult<Vec<JobStatus>> {
        self.jobs.list()
    }

    pub fn get_job(&self, id: JobId) -> AppResult<JobStatus> {
        self.jobs.get(id)
    }

    pub fn cancel_job(&self, id: JobId) -> AppResult<()> {
        self.jobs.cancel(id)
    }

    pub fn clear_finished_jobs(&self) -> AppResult<()> {
        self.jobs.clear_finished()
    }

    /// Waits until the job has finished (or `timeout` passes) and returns its status.
    pub fn wait_job(&self, id: JobId, timeout: Option<Duration>) -> AppResult<JobStatus> {
        let deadline = timeout.map(|t| std::time::Instant::now() + t);
        loop {
            let s = self.jobs.get(id)?;
            if s.state.is_finished() || deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                return Ok(s);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Starts a diagnostic job that only reports progress, to check the job runner end to end.
    pub fn start_self_test_job(&self, steps: u64) -> AppResult<JobId> {
        if !(1..=10_000).contains(&steps) {
            return Err(AppError::InvalidInput("steps must be between 1 and 10000".into()));
        }
        self.jobs.spawn(JobKind::SelfTest, "Job runner self-test", jobs::self_test(steps, Duration::from_millis(50)))
    }
}

/// The desktop app's bundle identifier, which names its config folder.
pub const APP_IDENTIFIER: &str = "com.mega-pc.aeondatastudio";

/// The settings file the desktop app uses, so the CLI can find the last workspace.
pub fn default_settings_path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| Path::new(&h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))
    };
    base.map(|b| b.join(APP_IDENTIFIER).join("settings.json"))
}
