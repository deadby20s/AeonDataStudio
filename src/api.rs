//! Typed wrappers around Tauri commands and events. Components call these instead of
//! `invoke` with raw strings, so command names and argument shapes live in one place.

use aeon_shared::{
    AppInfo, DatasetManifest, DatasetSummary, Document, DocumentPage, ImportRequest, JOB_UPDATE_EVENT, JobId,
    ChangedPage, JobStatus, PipelinePreview, PipelineSpec, PipelineSummary, RemovedDoc, RemovedPage, RunReport, RunSummary,
    WorkspaceInfo,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    // `catch` turns a rejected promise (a command returning `Err`) into `Err(JsValue)`
    // instead of a panic.
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], catch)]
    async fn invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "event"], catch)]
    async fn listen(event: &str, handler: &Closure<dyn FnMut(JsValue)>) -> Result<JsValue, JsValue>;
}

/// Error from a backend command, as a user-readable message.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError(pub String);

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<JsValue> for ApiError {
    fn from(v: JsValue) -> Self {
        let msg = v
            .as_string()
            .or_else(|| js_sys::Reflect::get(&v, &"message".into()).ok().and_then(|m| m.as_string()))
            .unwrap_or_else(|| format!("{v:?}"));
        ApiError(msg)
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Command arguments use camelCase keys: Tauri maps `job_id` in Rust to `jobId` in JS.
async fn call<A: Serialize, R: DeserializeOwned>(cmd: &str, args: &A) -> ApiResult<R> {
    let serializer = serde_wasm_bindgen::Serializer::json_compatible();
    let args = args.serialize(&serializer).map_err(|e| ApiError(e.to_string()))?;
    let value = invoke(cmd, args).await?;
    serde_wasm_bindgen::from_value(value).map_err(|e| ApiError(format!("bad response from {cmd}: {e}")))
}

#[derive(Serialize)]
struct NoArgs {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PathArg<'a> {
    path: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JobIdArg {
    job_id: JobId,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StepsArg {
    steps: u64,
}

pub async fn get_app_info() -> ApiResult<AppInfo> {
    call("get_app_info", &NoArgs {}).await
}

pub async fn pick_folder() -> ApiResult<Option<String>> {
    call("pick_folder", &NoArgs {}).await
}

pub async fn open_workspace(path: &str) -> ApiResult<WorkspaceInfo> {
    call("open_workspace", &PathArg { path }).await
}

pub async fn close_workspace() -> ApiResult<()> {
    call("close_workspace", &NoArgs {}).await
}

pub async fn forget_recent_workspace(path: &str) -> ApiResult<Vec<String>> {
    call("forget_recent_workspace", &PathArg { path }).await
}

pub async fn list_datasets() -> ApiResult<Vec<DatasetSummary>> {
    call("list_datasets", &NoArgs {}).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DatasetArg<'a> {
    dataset_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowseArgs<'a> {
    dataset_id: &'a str,
    start_row: u64,
    limit: u64,
    query: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentArgs<'a> {
    dataset_id: &'a str,
    row: u64,
}

#[derive(Serialize)]
struct ImportArgs<'a> {
    request: &'a ImportRequest,
}

pub async fn pick_import_files() -> ApiResult<Vec<String>> {
    call("pick_import_files", &NoArgs {}).await
}

pub async fn pick_import_folder() -> ApiResult<Option<String>> {
    call("pick_import_folder", &NoArgs {}).await
}

pub async fn start_import(request: &ImportRequest) -> ApiResult<JobId> {
    call("start_import", &ImportArgs { request }).await
}

pub async fn get_dataset(dataset_id: &str) -> ApiResult<DatasetManifest> {
    call("get_dataset", &DatasetArg { dataset_id }).await
}

pub async fn browse_documents(
    dataset_id: &str,
    start_row: u64,
    limit: u64,
    query: Option<&str>,
) -> ApiResult<DocumentPage> {
    call("browse_documents", &BrowseArgs { dataset_id, start_row, limit, query }).await
}

pub async fn get_document(dataset_id: &str, row: u64) -> ApiResult<Document> {
    call("get_document", &DocumentArgs { dataset_id, row }).await
}

pub async fn delete_dataset(dataset_id: &str) -> ApiResult<()> {
    call("delete_dataset", &DatasetArg { dataset_id }).await
}

#[derive(Serialize)]
struct FileArg<'a> {
    file: &'a str,
}

#[derive(Serialize)]
struct SavePipelineArgs<'a> {
    spec: &'a PipelineSpec,
    file: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewArgs<'a> {
    spec: &'a PipelineSpec,
    dataset_id: &'a str,
    sample_size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunArgs<'a> {
    spec: &'a PipelineSpec,
    dataset_id: &'a str,
    output_name: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunIdArg<'a> {
    run_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowseRemovedArgs<'a> {
    run_id: &'a str,
    start: u64,
    limit: u64,
    step_index: Option<u64>,
    rule: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemovedArgs<'a> {
    run_id: &'a str,
    index: u64,
}

pub async fn list_pipelines() -> ApiResult<Vec<PipelineSummary>> {
    call("list_pipelines", &NoArgs {}).await
}

pub async fn get_pipeline(file: &str) -> ApiResult<PipelineSpec> {
    call("get_pipeline", &FileArg { file }).await
}

pub async fn save_pipeline(spec: &PipelineSpec, file: Option<&str>) -> ApiResult<String> {
    call("save_pipeline", &SavePipelineArgs { spec, file }).await
}

pub async fn delete_pipeline(file: &str) -> ApiResult<()> {
    call("delete_pipeline", &FileArg { file }).await
}

pub async fn preview_pipeline(spec: &PipelineSpec, dataset_id: &str, sample_size: u64) -> ApiResult<PipelinePreview> {
    call("preview_pipeline", &PreviewArgs { spec, dataset_id, sample_size }).await
}

pub async fn start_pipeline_run(spec: &PipelineSpec, dataset_id: &str, output_name: &str) -> ApiResult<JobId> {
    call("start_pipeline_run", &RunArgs { spec, dataset_id, output_name }).await
}

pub async fn list_runs() -> ApiResult<Vec<RunSummary>> {
    call("list_runs", &NoArgs {}).await
}

pub async fn get_run(run_id: &str) -> ApiResult<RunReport> {
    call("get_run", &RunIdArg { run_id }).await
}

pub async fn browse_removed(
    run_id: &str,
    start: u64,
    limit: u64,
    step_index: Option<u64>,
    rule: Option<&str>,
) -> ApiResult<RemovedPage> {
    call("browse_removed", &BrowseRemovedArgs { run_id, start, limit, step_index, rule }).await
}

pub async fn get_removed(run_id: &str, index: u64) -> ApiResult<RemovedDoc> {
    call("get_removed", &RemovedArgs { run_id, index }).await
}

pub async fn delete_run(run_id: &str) -> ApiResult<()> {
    call("delete_run", &RunIdArg { run_id }).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SampleRemovedArgs<'a> {
    run_id: &'a str,
    step_index: u64,
    rule: &'a str,
    count: u64,
    seed: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowseChangedArgs<'a> {
    run_id: &'a str,
    step_index: Option<u64>,
    start: u64,
    limit: u64,
}

pub async fn sample_removed(run_id: &str, step_index: u64, rule: &str, count: u64, seed: u64) -> ApiResult<Vec<RemovedDoc>> {
    call("sample_removed", &SampleRemovedArgs { run_id, step_index, rule, count, seed }).await
}

pub async fn browse_changed(run_id: &str, step_index: Option<u64>, start: u64, limit: u64) -> ApiResult<ChangedPage> {
    call("browse_changed", &BrowseChangedArgs { run_id, step_index, start, limit }).await
}

pub async fn list_jobs() -> ApiResult<Vec<JobStatus>> {
    call("list_jobs", &NoArgs {}).await
}

pub async fn cancel_job(job_id: JobId) -> ApiResult<()> {
    call("cancel_job", &JobIdArg { job_id }).await
}

pub async fn clear_finished_jobs() -> ApiResult<()> {
    call("clear_finished_jobs", &NoArgs {}).await
}

pub async fn start_self_test_job(steps: u64) -> ApiResult<JobId> {
    call("start_self_test_job", &StepsArg { steps }).await
}

/// Calls `on_update` for every job status change. The listener lives for the rest of the app.
pub async fn on_job_update(mut on_update: impl FnMut(JobStatus) + 'static) -> ApiResult<()> {
    let handler = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let payload = js_sys::Reflect::get(&event, &"payload".into()).unwrap_or(JsValue::NULL);
        match serde_wasm_bindgen::from_value::<JobStatus>(payload) {
            Ok(status) => on_update(status),
            Err(e) => leptos::logging::error!("bad job update payload: {e}"),
        }
    });
    listen(JOB_UPDATE_EVENT, &handler).await?;
    handler.forget();
    Ok(())
}
