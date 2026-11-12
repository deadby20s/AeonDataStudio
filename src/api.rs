//! Typed wrappers around Tauri commands and events. Components call these instead of
//! `invoke` with raw strings, so command names and argument shapes live in one place.

use aeon_shared::{
    AppInfo, DatasetManifest, DatasetSummary, Document, DocumentPage, ImportRequest, JOB_UPDATE_EVENT, JobId,
    JobStatus, WorkspaceInfo,
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
