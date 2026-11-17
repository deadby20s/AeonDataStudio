//! The local HTTP API: JSON over HTTP on 127.0.0.1, guarded by a bearer token.
//!
//! [`handle`] routes one request to [`AppState`]; [`ApiServer`] serves it over HTTP, and the
//! MCP server ([`crate::mcp`]) calls [`handle`] directly, so all three stay in step.
//! Endpoints are listed in docs/api.md.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use aeon_shared::{
    ExportRequest, FieldKind, ImportFormat, ImportRequest, MixRequest, PRESETS, PipelineSpec, STEP_KINDS, StepCategory,
    TextMode,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::AppError;
use crate::state::AppState;

pub const PREFIX: &str = "/api/v1";
pub const DEFAULT_PORT: u16 = 7878;
const MAX_BODY: usize = 16 * 1024 * 1024;
const MAX_WAIT_SECS: u64 = 600;

/// An API error: HTTP status and message.
#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }
}

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        let status = match &e {
            AppError::NoWorkspace => 409,
            AppError::InvalidInput(_) | AppError::Json(_) | AppError::TomlRead(_) => 400,
            AppError::DatasetNotFound(_) | AppError::JobNotFound(_) => 404,
            AppError::Cancelled => 409,
            AppError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => 404,
            _ => 500,
        };
        Self::new(status, e.to_string())
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        Self::new(400, format!("Invalid request body: {e}"))
    }
}

pub type ApiResult = Result<Value, ApiError>;

/// One API request, independent of HTTP.
pub struct Request<'a> {
    pub method: &'a str,
    /// Path after [`PREFIX`], e.g. `/datasets/abc/documents`.
    pub path: &'a str,
    pub query: BTreeMap<String, String>,
    pub body: Value,
}

impl Request<'_> {
    fn q_u64(&self, key: &str, default: u64) -> Result<u64, ApiError> {
        match self.query.get(key) {
            None => Ok(default),
            Some(v) => v.parse().map_err(|_| ApiError::new(400, format!("Query parameter {key} must be a number"))),
        }
    }

    fn q_opt_u64(&self, key: &str) -> Result<Option<u64>, ApiError> {
        self.query.get(key).map(|_| self.q_u64(key, 0)).transpose()
    }

    fn body<T: DeserializeOwned>(&self) -> Result<T, ApiError> {
        Ok(serde_json::from_value(self.body.clone())?)
    }
}

fn to_json<T: serde::Serialize>(v: T) -> ApiResult {
    serde_json::to_value(v).map_err(|e| ApiError::new(500, e.to_string()))
}

fn job(id: u64) -> ApiResult {
    Ok(json!({ "job_id": id }))
}

/// A pipeline given inline (`spec`) or by saved file name (`pipeline`).
#[derive(Deserialize)]
struct PipelineRef {
    #[serde(default)]
    spec: Option<PipelineSpec>,
    #[serde(default)]
    pipeline: Option<String>,
}

impl PipelineRef {
    fn resolve(self, state: &AppState) -> Result<PipelineSpec, ApiError> {
        match (self.spec, self.pipeline) {
            (Some(spec), _) => Ok(spec),
            (None, Some(file)) => {
                let file = if file.ends_with(".toml") { file } else { format!("{file}.toml") };
                Ok(state.get_pipeline(&file)?)
            }
            (None, None) => Err(ApiError::new(400, "Give either `spec` (a pipeline) or `pipeline` (a saved file name)")),
        }
    }
}

#[derive(Deserialize)]
struct PreviewBody {
    #[serde(flatten)]
    pipeline: PipelineRef,
    dataset: String,
    #[serde(default = "default_sample")]
    sample_size: u64,
}

fn default_sample() -> u64 {
    1000
}

#[derive(Deserialize)]
struct RunBody {
    #[serde(flatten)]
    pipeline: PipelineRef,
    dataset: String,
    #[serde(default)]
    output_name: String,
}

#[derive(Deserialize)]
struct ImportPreviewBody {
    paths: Vec<String>,
    #[serde(default = "auto")]
    format: ImportFormat,
    #[serde(default = "text")]
    text_field: String,
    #[serde(default = "whole")]
    text_mode: TextMode,
}

fn auto() -> ImportFormat {
    ImportFormat::Auto
}
fn text() -> String {
    "text".into()
}
fn whole() -> TextMode {
    TextMode::WholeFile
}

#[derive(Deserialize)]
struct SaveBody {
    spec: PipelineSpec,
    #[serde(default)]
    file: Option<String>,
}

#[derive(Deserialize)]
struct SqlBody {
    sql: String,
    #[serde(default = "sql_limit")]
    limit: u64,
}

fn sql_limit() -> u64 {
    200
}

#[derive(Deserialize)]
struct WorkspaceBody {
    path: String,
}

/// Step kinds and their settings, for building or checking pipelines.
pub fn step_kinds_json() -> Value {
    let kinds: Vec<Value> = STEP_KINDS
        .iter()
        .map(|k| {
            let fields: Vec<Value> = k
                .fields
                .iter()
                .map(|f| {
                    let (kind, choices) = match f.kind {
                        FieldKind::Bool => ("bool", None),
                        FieldKind::Count => ("integer", None),
                        FieldKind::OptCount => ("optional integer", None),
                        FieldKind::OptRatio => ("optional ratio 0-1", None),
                        FieldKind::Ratio => ("ratio 0-1", None),
                        FieldKind::OptNumber => ("optional number", None),
                        FieldKind::Number => ("number", None),
                        FieldKind::Choice(c) => ("choice", Some(c.iter().map(|(v, _)| *v).collect::<Vec<_>>())),
                        FieldKind::List => ("list of strings", None),
                        FieldKind::Text => ("string", None),
                        FieldKind::LongText => ("text", None),
                    };
                    json!({ "key": f.key, "label": f.label, "type": kind, "choices": choices, "help": f.help })
                })
                .collect();
            json!({
                "kind": k.kind,
                "label": k.label,
                "category": match k.category {
                    StepCategory::Normalize => "normalize",
                    StepCategory::Filter => "filter",
                    StepCategory::Dedup => "dedup",
                    StepCategory::External => "external",
                },
                "description": k.description,
                "fields": fields,
                "default": (k.default)(),
            })
        })
        .collect();
    Value::Array(kinds)
}

fn presets_json() -> Value {
    Value::Array(
        PRESETS
            .iter()
            .map(|p| json!({ "id": p.id, "label": p.label, "description": p.description, "uses_llm": p.uses_llm, "spec": (p.build)() }))
            .collect(),
    )
}

/// Routes one request. Paths are relative to [`PREFIX`].
pub fn handle(state: &AppState, req: &Request<'_>) -> ApiResult {
    let decoded: Vec<String> =
        req.path.trim_matches('/').split('/').filter(|s| !s.is_empty()).map(percent_decode_path).collect();
    let segs: Vec<&str> = decoded.iter().map(String::as_str).collect();
    let ds = |id: &str| state.resolve_dataset(id).map_err(ApiError::from);
    match (req.method, segs.as_slice()) {
        ("GET", []) | ("GET", ["info"]) => to_json(state.app_info()?),
        ("POST", ["workspace"]) => {
            let b: WorkspaceBody = req.body()?;
            to_json(state.open_workspace_path(&b.path)?)
        }

        ("GET", ["datasets"]) => to_json(state.list_datasets()?),
        ("GET", ["datasets", id]) => to_json(state.get_dataset(&ds(id)?)?),
        ("DELETE", ["datasets", id]) => {
            state.delete_dataset(&ds(id)?)?;
            Ok(json!({ "deleted": true }))
        }
        ("GET", ["datasets", id, "documents"]) => to_json(state.browse_documents(
            &ds(id)?,
            req.q_u64("start", 0)?,
            req.q_u64("limit", 20)?.min(500),
            req.query.get("query").map(String::as_str).filter(|q| !q.is_empty()),
        )?),
        ("GET", ["datasets", id, "documents", row]) => {
            let row: u64 = row.parse().map_err(|_| ApiError::new(400, "Row must be a number"))?;
            to_json(state.get_document(&ds(id)?, row)?)
        }
        ("POST", ["imports"]) => job(state.start_import(req.body::<ImportRequest>()?)?),
        ("POST", ["imports", "preview"]) => {
            let b: ImportPreviewBody = req.body()?;
            to_json(state.preview_import(&b.paths, b.format, &b.text_field, b.text_mode)?)
        }
        ("POST", ["demo"]) => job(state.create_demo_dataset()?),

        ("GET", ["steps"]) => Ok(step_kinds_json()),
        ("GET", ["presets"]) => Ok(presets_json()),
        ("GET", ["plugins"]) => to_json(crate::plugins::list_plugins(&state.workspace()?.plugins_dir())?),

        ("GET", ["pipelines"]) => to_json(state.list_pipelines()?),
        ("GET", ["pipelines", file]) => to_json(state.get_pipeline(file)?),
        ("POST", ["pipelines"]) => {
            let b: SaveBody = req.body()?;
            Ok(json!({ "file": state.save_pipeline(&b.spec, b.file.as_deref())? }))
        }
        ("DELETE", ["pipelines", file]) => {
            state.delete_pipeline(file)?;
            Ok(json!({ "deleted": true }))
        }
        ("POST", ["preview"]) => {
            let b: PreviewBody = req.body()?;
            let spec = b.pipeline.resolve(state)?;
            to_json(state.preview_pipeline(&spec, &ds(&b.dataset)?, b.sample_size)?)
        }
        ("POST", ["runs"]) => {
            let b: RunBody = req.body()?;
            let spec = b.pipeline.resolve(state)?;
            job(state.start_pipeline_run(spec, &ds(&b.dataset)?, &b.output_name)?)
        }
        ("GET", ["runs"]) => to_json(state.list_runs()?),
        ("GET", ["runs", id]) => to_json(state.get_run(id)?),
        ("DELETE", ["runs", id]) => {
            state.delete_run(id)?;
            Ok(json!({ "deleted": true }))
        }
        ("GET", ["runs", id, "removed"]) => to_json(state.browse_removed(
            id,
            req.q_u64("start", 0)?,
            req.q_u64("limit", 20)?.min(500),
            req.q_opt_u64("step")?,
            req.query.get("rule").map(String::as_str),
        )?),
        ("GET", ["runs", id, "removed", index]) => {
            let index: u64 = index.parse().map_err(|_| ApiError::new(400, "Index must be a number"))?;
            to_json(state.get_removed(id, index)?)
        }
        ("GET", ["runs", id, "sample"]) => {
            let rule = req.query.get("rule").ok_or_else(|| ApiError::new(400, "Give rule="))?;
            let step = req.q_opt_u64("step")?.ok_or_else(|| ApiError::new(400, "Give step="))?;
            to_json(state.sample_removed(id, step, rule, req.q_u64("count", 10)?.min(200), req.q_u64("seed", 1)?)?)
        }
        ("GET", ["runs", id, "changed"]) => to_json(state.browse_changed(
            id,
            req.q_opt_u64("step")?,
            req.q_u64("start", 0)?,
            req.q_u64("limit", 20)?.min(500),
        )?),

        ("POST", ["exports"]) => {
            let mut r: ExportRequest = req.body()?;
            r.dataset_id = ds(&r.dataset_id)?;
            job(state.start_export(r)?)
        }
        ("GET", ["exports"]) => to_json(state.list_exports()?),
        ("POST", ["mixes"]) => {
            let mut r: MixRequest = req.body()?;
            for s in &mut r.sources {
                s.dataset_id = ds(&s.dataset_id)?;
            }
            job(state.start_mix(r)?)
        }

        ("GET", ["sql", "tables"]) => to_json(state.sql_tables()?),
        ("POST", ["sql"]) => {
            let b: SqlBody = req.body()?;
            to_json(state.run_sql(&b.sql, b.limit)?)
        }

        ("GET", ["jobs"]) => to_json(state.list_jobs()?),
        ("GET", ["jobs", id]) => {
            let id: u64 = id.parse().map_err(|_| ApiError::new(400, "Job id must be a number"))?;
            match req.q_u64("wait", 0)? {
                0 => to_json(state.get_job(id)?),
                secs => to_json(state.wait_job(id, Some(Duration::from_secs(secs.min(MAX_WAIT_SECS))))?),
            }
        }
        ("POST", ["jobs", id, "cancel"]) => {
            let id: u64 = id.parse().map_err(|_| ApiError::new(400, "Job id must be a number"))?;
            state.cancel_job(id)?;
            Ok(json!({ "cancelling": true }))
        }

        (_, _) => Err(ApiError::new(404, format!("No endpoint {} {PREFIX}{}", req.method, req.path))),
    }
}

/// A random token for the API (48 hex characters).
pub fn new_token() -> String {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).expect("the operating system provides random numbers");
    hex::encode(bytes)
}

fn parse_query(q: &str) -> BTreeMap<String, String> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

/// Like [`percent_decode`], but `+` stays a plus (it only means a space in queries).
fn percent_decode_path(s: &str) -> String {
    percent_decode(&s.replace('+', "%2B"))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                Some(b) => {
                    out.push(b);
                    i += 2;
                }
                None => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Constant-time comparison, so the token can't be guessed byte by byte from timings.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A running HTTP server for the API.
pub struct ApiServer {
    server: Arc<tiny_http::Server>,
    threads: Vec<std::thread::JoinHandle<()>>,
    pub url: String,
    pub token: String,
}

impl ApiServer {
    /// Serves the API on 127.0.0.1:`port` (0 picks a free port).
    pub fn start(state: Arc<AppState>, port: u16, token: String) -> Result<Self, AppError> {
        let server = tiny_http::Server::http(("127.0.0.1", port))
            .map_err(|e| AppError::InvalidInput(format!("Could not listen on 127.0.0.1:{port}: {e}")))?;
        let port = server.server_addr().to_ip().map_or(port, |a| a.port());
        let server = Arc::new(server);
        let threads = (0..4)
            .map(|i| {
                let server = server.clone();
                let state = state.clone();
                let token = token.clone();
                std::thread::Builder::new()
                    .name(format!("api-{i}"))
                    .spawn(move || {
                        for request in server.incoming_requests() {
                            serve_one(&state, &token, request);
                        }
                    })
                    .expect("spawn API thread")
            })
            .collect();
        log::info!("local API listening on http://127.0.0.1:{port}");
        Ok(Self { server, threads, url: format!("http://127.0.0.1:{port}"), token })
    }

    /// Stops accepting requests and waits for the workers.
    pub fn stop(self) {
        self.server.unblock();
        for _ in 1..self.threads.len() {
            self.server.unblock();
        }
        for t in self.threads {
            let _ = t.join();
        }
        log::info!("local API stopped");
    }

    /// Blocks until the server stops (for `aeon serve`).
    pub fn join(self) {
        for t in self.threads {
            let _ = t.join();
        }
    }
}

fn respond(request: tiny_http::Request, status: u16, body: &Value) {
    let header = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("valid header");
    let resp = tiny_http::Response::from_string(body.to_string()).with_status_code(status).with_header(header);
    let _ = request.respond(resp);
}

fn serve_one(state: &AppState, token: &str, mut request: tiny_http::Request) {
    let header = |name: &str| {
        request
            .headers()
            .iter()
            .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str().to_string())
    };
    let given = header("Authorization")
        .and_then(|v| v.strip_prefix("Bearer ").map(str::to_string))
        .or_else(|| header("X-Api-Key"));
    if !given.is_some_and(|g| same(g.trim(), token)) {
        return respond(request, 401, &json!({ "error": "Missing or wrong API token (send Authorization: Bearer <token>)" }));
    }

    let url = request.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let Some(path) = path.strip_prefix(PREFIX) else {
        return respond(request, 404, &json!({ "error": format!("API paths start with {PREFIX}") }));
    };
    let path = path.to_string();
    let method = request.method().as_str().to_uppercase();

    let mut raw = Vec::new();
    if let Err(e) = request.as_reader().take(MAX_BODY as u64 + 1).read_to_end(&mut raw) {
        return respond(request, 400, &json!({ "error": format!("Could not read the body: {e}") }));
    }
    if raw.len() > MAX_BODY {
        return respond(request, 413, &json!({ "error": "Request body too large" }));
    }
    let body = if raw.iter().all(u8::is_ascii_whitespace) {
        Value::Null
    } else {
        match serde_json::from_slice(&raw) {
            Ok(v) => v,
            Err(e) => return respond(request, 400, &json!({ "error": format!("Body is not valid JSON: {e}") })),
        }
    };

    let req = Request { method: &method, path: &path, query: parse_query(query), body };
    let started = std::time::Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle(state, &req)));
    let (status, body) = match result {
        Ok(Ok(v)) => (200, v),
        Ok(Err(e)) => (e.status, json!({ "error": e.message })),
        Err(_) => (500, json!({ "error": "Internal error" })),
    };
    log::info!("API {method} {path} → {status} in {} ms", started.elapsed().as_millis());
    respond(request, status, &body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobManager, NoEvents};

    fn state_with_workspace() -> (tempfile::TempDir, Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new(dir.path().join("settings.json"), JobManager::new(Arc::new(NoEvents)), "test"));
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        state.open_workspace(&ws).unwrap();
        (dir, state)
    }

    fn call(server: &ApiServer, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let url = format!("{}{PREFIX}{path}", server.url);
        let auth = format!("Bearer {token}");
        let mut resp = match (method, body) {
            ("GET", _) => agent.get(&url).header("Authorization", &auth).call().unwrap(),
            (_, Some(b)) => agent.post(&url).header("Authorization", &auth).send_json(&b).unwrap(),
            (_, None) => agent.post(&url).header("Authorization", &auth).send_empty().unwrap(),
        };
        let status = resp.status().as_u16();
        (status, resp.body_mut().read_json().unwrap_or(Value::Null))
    }

    #[test]
    fn decodes_queries() {
        let q = parse_query("query=caf%C3%A9+au+lait&limit=5&x");
        assert_eq!(q["query"], "café au lait");
        assert_eq!(q["limit"], "5");
        assert_eq!(q["x"], "");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn serves_the_demo_flow_over_http_with_a_token() {
        let (_dir, state) = state_with_workspace();
        let server = ApiServer::start(state.clone(), 0, new_token()).unwrap();
        let token = server.token.clone();

        assert_eq!(call(&server, "GET", "/datasets", "wrong", None).0, 401);
        let (status, info) = call(&server, "GET", "/info", &token, None);
        assert_eq!(status, 200, "{info}");
        assert_eq!(info["version"], "test");

        let (_, j) = call(&server, "POST", "/demo", &token, None);
        let id = j["job_id"].as_u64().unwrap();
        let (_, done) = call(&server, "GET", &format!("/jobs/{id}?wait=60"), &token, None);
        assert_eq!(done["state"], "Completed", "{done}");

        let (_, list) = call(&server, "GET", "/datasets", &token, None);
        let name = list[0]["name"].as_str().unwrap().to_string();
        let enc = name.replace(' ', "%20").replace(':', "%3A");
        let (status, page) = call(&server, "GET", &format!("/datasets/{enc}/documents?limit=2"), &token, None);
        assert_eq!(status, 200, "{page}");
        assert_eq!(page["docs"].as_array().unwrap().len(), 2);

        let (status, preview) = call(
            &server,
            "POST",
            "/preview",
            &token,
            Some(json!({ "pipeline": "demo-cleanup", "dataset": name, "sample_size": 300 })),
        );
        assert_eq!(status, 200, "{preview}");
        assert_eq!(preview["sample_docs"], 300);

        let (_, run) = call(&server, "POST", "/runs", &token, Some(json!({ "pipeline": "demo-cleanup", "dataset": name })));
        let id = run["job_id"].as_u64().unwrap();
        let (_, done) = call(&server, "GET", &format!("/jobs/{id}?wait=120"), &token, None);
        assert_eq!(done["state"], "Completed", "{done}");
        let (_, runs) = call(&server, "GET", "/runs", &token, None);
        assert_eq!(runs.as_array().unwrap().len(), 1);

        let (status, err) = call(&server, "GET", "/datasets/nope", &token, None);
        assert_eq!(status, 404, "{err}");
        assert_eq!(call(&server, "GET", "/nothing", &token, None).0, 404);
        let (_, steps) = call(&server, "GET", "/steps", &token, None);
        assert!(steps.as_array().unwrap().iter().any(|s| s["kind"] == "llm_score"));
        server.stop();
    }
}
