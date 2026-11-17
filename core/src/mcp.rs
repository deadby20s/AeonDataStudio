//! A Model Context Protocol server over stdio, so AI assistants (Claude Desktop, Claude Code,
//! other MCP clients) can inspect datasets, try pipelines and start runs.
//!
//! Messages are JSON-RPC 2.0, one per line. Every tool is a call into the local API router
//! ([`crate::api::handle`]). Tools never delete anything: deleting stays in the app.

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::{Value, json};

use crate::api::{self, ApiError, Request};
use crate::state::AppState;

const PROTOCOL_VERSION: &str = "2025-06-18";

const INSTRUCTIONS: &str = "AeonDataStudio prepares text data for pretraining language models. \
A workspace holds datasets (imported documents), pipelines (cleaning recipes: normalize, filter, deduplicate, \
plugins and language-model steps), runs (a pipeline applied to a dataset, producing a new dataset and a log of \
removed documents) and exports (JSONL, Parquet or token shards). Typical flow: list_datasets → browse_documents \
to look at the data → list_step_kinds / list_presets → preview_pipeline on a sample → run_pipeline → get_run to \
see what each step removed → sample_removed to check the removals are right → export_dataset. Datasets can be \
named by id or by name. Long jobs return a job id; get_job with wait_seconds waits for them.";

struct Tool {
    name: &'static str,
    description: &'static str,
    schema: fn() -> Value,
    call: fn(&AppState, &Value) -> Result<Value, ApiError>,
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

fn s(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}

fn n(desc: &str) -> Value {
    json!({ "type": "integer", "description": desc })
}

fn b(desc: &str) -> Value {
    json!({ "type": "boolean", "description": desc })
}

fn get(state: &AppState, path: &str, query: &[(&str, Option<String>)]) -> Result<Value, ApiError> {
    let query = query.iter().filter_map(|(k, v)| v.clone().map(|v| (k.to_string(), v))).collect();
    api::handle(state, &Request { method: "GET", path, query, body: Value::Null })
}

fn post(state: &AppState, path: &str, body: Value) -> Result<Value, ApiError> {
    api::handle(state, &Request { method: "POST", path, query: Default::default(), body })
}

fn arg_str(args: &Value, key: &str) -> Result<String, ApiError> {
    args[key]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| ApiError { status: 400, message: format!("Missing argument `{key}`") })
}

fn opt(args: &Value, key: &str) -> Option<String> {
    match &args[key] {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        v => Some(v.to_string()),
    }
}

fn path_part(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Starts a job through `start`, then waits for it unless `wait` is false.
fn start_and_wait(state: &AppState, args: &Value, started: Result<Value, ApiError>) -> Result<Value, ApiError> {
    let started = started?;
    let id = started["job_id"].as_u64().ok_or_else(|| ApiError { status: 500, message: "no job id".into() })?;
    if args["wait"].as_bool() == Some(false) {
        return Ok(started);
    }
    let secs = args["wait_seconds"].as_u64().unwrap_or(600).min(3600);
    let status = state.wait_job(id, Some(Duration::from_secs(secs)))?;
    Ok(serde_json::to_value(status).unwrap_or(Value::Null))
}

/// `jsonl` → `Jsonl`, as the import format is spelled in requests.
pub fn format_name(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// The dataset argument as the router expects it (the router resolves names, but a name is a
/// path segment there, so it is percent-encoded).
fn dataset_path(args: &Value) -> Result<String, ApiError> {
    Ok(path_part(&arg_str(args, "dataset")?))
}

static TOOLS: &[Tool] = &[
    Tool {
        name: "studio_info",
        description: "App version and the open workspace (folder, counts).",
        schema: || obj(json!({}), &[]),
        call: |st, _| get(st, "/info", &[]),
    },
    Tool {
        name: "open_workspace",
        description: "Open a workspace folder (an empty folder becomes a new workspace).",
        schema: || obj(json!({ "path": s("Folder path") }), &["path"]),
        call: |st, a| post(st, "/workspace", json!({ "path": arg_str(a, "path")? })),
    },
    Tool {
        name: "list_datasets",
        description: "Datasets in the workspace with document and character counts.",
        schema: || obj(json!({}), &[]),
        call: |st, _| get(st, "/datasets", &[]),
    },
    Tool {
        name: "get_dataset",
        description: "A dataset's manifest: source files, fields, statistics, lineage.",
        schema: || obj(json!({ "dataset": s("Dataset id or name") }), &["dataset"]),
        call: |st, a| get(st, &format!("/datasets/{}", dataset_path(a)?), &[]),
    },
    Tool {
        name: "browse_documents",
        description: "A page of documents (text previews) from a dataset, optionally only those containing `query`.",
        schema: || obj(
            json!({ "dataset": s("Dataset id or name"), "start": n("First row (0-based)"), "limit": n("Rows, at most 100"), "query": s("Only documents containing this text") }),
            &["dataset"],
        ),
        call: |st, a| {
            let limit = a["limit"].as_u64().unwrap_or(10).min(100).to_string();
            get(st, &format!("/datasets/{}/documents", dataset_path(a)?), &[("start", opt(a, "start")), ("limit", Some(limit)), ("query", opt(a, "query"))])
        },
    },
    Tool {
        name: "get_document",
        description: "One document's full text and metadata.",
        schema: || obj(json!({ "dataset": s("Dataset id or name"), "row": n("Row (0-based)") }), &["dataset", "row"]),
        call: |st, a| get(st, &format!("/datasets/{}/documents/{}", dataset_path(a)?, a["row"].as_u64().unwrap_or(0)), &[]),
    },
    Tool {
        name: "import_files",
        description: "Import files or folders (JSONL, JSON, CSV/TSV, Parquet, text, Markdown, HTML, WARC; .gz/.zst) as a new dataset. Waits for the import unless wait=false.",
        schema: || obj(
            json!({
                "name": s("Dataset name"),
                "paths": { "type": "array", "items": { "type": "string" }, "description": "Files or folders" },
                "format": s("auto (default), jsonl, json, csv, tsv, parquet, text, html, warc"),
                "text_field": s("Field holding the text (default: text)"),
                "wait": b("Wait for the import to finish (default true)"),
            }),
            &["name", "paths"],
        ),
        call: |st, a| {
            let body = json!({
                "name": arg_str(a, "name")?,
                "description": null,
                "paths": a["paths"].clone(),
                "format": format_name(a["format"].as_str().unwrap_or("auto")),
                "text_field": a["text_field"].as_str().unwrap_or("text"),
                "id_field": null,
                "text_mode": "WholeFile",
            });
            start_and_wait(st, a, post(st, "/imports", body))
        },
    },
    Tool {
        name: "create_demo_dataset",
        description: "Generate a demo dataset with planted problems (spam, duplicates, other languages, broken encoding) and a matching pipeline. Waits for it.",
        schema: || obj(json!({}), &[]),
        call: |st, a| start_and_wait(st, a, post(st, "/demo", Value::Null)),
    },
    Tool {
        name: "list_step_kinds",
        description: "Every pipeline step type with its settings, defaults and descriptions. Read this before writing a pipeline spec.",
        schema: || obj(json!({}), &[]),
        call: |_, _| Ok(api::step_kinds_json()),
    },
    Tool {
        name: "list_presets",
        description: "Ready-made pipelines (Gopher, FineWeb, C4, minimal, and LLM data recipes) as full specs.",
        schema: || obj(json!({}), &[]),
        call: |st, _| get(st, "/presets", &[]),
    },
    Tool {
        name: "list_plugins",
        description: "Plugins installed in the workspace's plugins folder, with their options.",
        schema: || obj(json!({}), &[]),
        call: |st, _| get(st, "/plugins", &[]),
    },
    Tool {
        name: "list_pipelines",
        description: "Saved pipelines (file names and step counts).",
        schema: || obj(json!({}), &[]),
        call: |st, _| get(st, "/pipelines", &[]),
    },
    Tool {
        name: "get_pipeline",
        description: "A saved pipeline's full spec.",
        schema: || obj(json!({ "file": s("File name, e.g. web-cleanup.toml") }), &["file"]),
        call: |st, a| get(st, &format!("/pipelines/{}", path_part(&arg_str(a, "file")?)), &[]),
    },
    Tool {
        name: "save_pipeline",
        description: "Save a pipeline spec ({name, description, steps: [{kind, enabled, ...settings}]}). Without `file` a new file is created; with it, that file is overwritten. Returns the file name.",
        schema: || obj(json!({ "spec": { "type": "object", "description": "Pipeline spec" }, "file": s("Existing file to overwrite") }), &["spec"]),
        call: |st, a| post(st, "/pipelines", json!({ "spec": a["spec"].clone(), "file": a["file"].clone() })),
    },
    Tool {
        name: "preview_pipeline",
        description: "Try a pipeline on a sample of a dataset without saving anything: per-step counts of dropped and changed documents, with examples. Give `pipeline` (saved file name) or `spec`.",
        schema: || obj(
            json!({ "dataset": s("Dataset id or name"), "pipeline": s("Saved pipeline file name"), "spec": { "type": "object" }, "sample_size": n("Documents to sample (default 1000)") }),
            &["dataset"],
        ),
        call: |st, a| post(st, "/preview", json!({ "dataset": arg_str(a, "dataset")?, "pipeline": a["pipeline"].clone(), "spec": a["spec"].clone(), "sample_size": a["sample_size"].as_u64().unwrap_or(1000) })),
    },
    Tool {
        name: "run_pipeline",
        description: "Run a pipeline on a whole dataset, producing a new dataset and a run report. Waits for it unless wait=false.",
        schema: || obj(
            json!({ "dataset": s("Dataset id or name"), "pipeline": s("Saved pipeline file name"), "spec": { "type": "object" }, "output_name": s("Name of the new dataset"), "wait": b("Wait (default true)") }),
            &["dataset"],
        ),
        call: |st, a| {
            let body = json!({ "dataset": arg_str(a, "dataset")?, "pipeline": a["pipeline"].clone(), "spec": a["spec"].clone(), "output_name": a["output_name"].as_str().unwrap_or("") });
            start_and_wait(st, a, post(st, "/runs", body))
        },
    },
    Tool {
        name: "list_runs",
        description: "Pipeline runs with their input, output and document counts.",
        schema: || obj(json!({}), &[]),
        call: |st, _| get(st, "/runs", &[]),
    },
    Tool {
        name: "get_run",
        description: "A run's report: per step, documents in, dropped (by rule) and changed.",
        schema: || obj(json!({ "run_id": s("Run id") }), &["run_id"]),
        call: |st, a| get(st, &format!("/runs/{}", path_part(&arg_str(a, "run_id")?)), &[]),
    },
    Tool {
        name: "sample_removed",
        description: "Random documents one rule of one step removed in a run, to check the rule is right.",
        schema: || obj(
            json!({ "run_id": s("Run id"), "step_index": n("Step index (from get_run)"), "rule": s("Rule name (from get_run)"), "count": n("How many (default 5)") }),
            &["run_id", "step_index", "rule"],
        ),
        call: |st, a| get(
            st,
            &format!("/runs/{}/sample", path_part(&arg_str(a, "run_id")?)),
            &[("step", opt(a, "step_index")), ("rule", opt(a, "rule")), ("count", Some(a["count"].as_u64().unwrap_or(5).to_string()))],
        ),
    },
    Tool {
        name: "export_dataset",
        description: "Export a dataset for training: format jsonl (gzip/zstd), parquet, or tokens (uint16/uint32 shards with a tokenizer). Takes an ExportRequest; see docs/api.md. Waits unless wait=false.",
        schema: || obj(json!({ "request": { "type": "object", "description": "ExportRequest; dataset_id may be a name" }, "wait": b("Wait (default true)") }), &["request"]),
        call: |st, a| start_and_wait(st, a, post(st, "/exports", a["request"].clone())),
    },
    Tool {
        name: "list_exports",
        description: "Exports with format, size and document counts.",
        schema: || obj(json!({}), &[]),
        call: |st, _| get(st, "/exports", &[]),
    },
    Tool {
        name: "run_sql",
        description: "Run a read-only DuckDB SQL query over the workspace: each dataset is a view (by name), plus removed_<run> views. Call sql_tables first to see names. At most 1000 rows.",
        schema: || obj(json!({ "sql": s("SQL query"), "limit": n("Max rows (default 200)") }), &["sql"]),
        call: |st, a| post(st, "/sql", json!({ "sql": arg_str(a, "sql")?, "limit": a["limit"].as_u64().unwrap_or(200) })),
    },
    Tool {
        name: "sql_tables",
        description: "Tables and columns available to run_sql.",
        schema: || obj(json!({}), &[]),
        call: |st, _| get(st, "/sql/tables", &[]),
    },
    Tool {
        name: "get_job",
        description: "Status and progress of a background job. With wait_seconds, waits up to that long for it to finish.",
        schema: || obj(json!({ "job_id": n("Job id"), "wait_seconds": n("Wait up to this long") }), &["job_id"]),
        call: |st, a| get(st, &format!("/jobs/{}", a["job_id"].as_u64().unwrap_or(0)), &[("wait", opt(a, "wait_seconds"))]),
    },
    Tool {
        name: "cancel_job",
        description: "Ask a running job to stop.",
        schema: || obj(json!({ "job_id": n("Job id") }), &["job_id"]),
        call: |st, a| post(st, &format!("/jobs/{}/cancel", a["job_id"].as_u64().unwrap_or(0)), Value::Null),
    },
];

fn tools_list() -> Value {
    Value::Array(
        TOOLS
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": (t.schema)() }))
            .collect(),
    )
}

/// Handles one JSON-RPC message. Returns the response, or `None` for notifications.
pub fn handle_message(state: &AppState, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg["method"].as_str().unwrap_or("");
    let params = &msg["params"];
    let result: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "aeondatastudio", "title": "AeonDataStudio", "version": state.version },
            "instructions": INSTRUCTIONS,
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools_list() })),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let empty = json!({});
            let args = if params["arguments"].is_object() { &params["arguments"] } else { &empty };
            match TOOLS.iter().find(|t| t.name == name) {
                None => Err((-32602, format!("Unknown tool {name}"))),
                Some(tool) => {
                    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (tool.call)(state, args)))
                        .unwrap_or_else(|_| Err(ApiError { status: 500, message: "Internal error".into() }));
                    Ok(match out {
                        Ok(v) => json!({
                            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }],
                            "isError": false,
                        }),
                        Err(e) => json!({ "content": [{ "type": "text", "text": e.message }], "isError": true }),
                    })
                }
            }
        }
        m if m.starts_with("notifications/") => return None,
        m => Err((-32601, format!("Method not found: {m}"))),
    };
    let id = id?; // Requests without an id are notifications.
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

/// Serves MCP on stdin/stdout until stdin closes.
pub fn serve_stdio(state: &AppState) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle_message(state, &msg),
            Err(e) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("Parse error: {e}") } })),
        };
        if let Some(r) = response {
            writeln!(stdout, "{r}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobManager, NoEvents};
    use std::sync::Arc;

    fn call(state: &AppState, id: u64, method: &str, params: Value) -> Value {
        handle_message(state, &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).unwrap()
    }

    fn tool(state: &AppState, name: &str, args: Value) -> (bool, String) {
        let r = call(state, 9, "tools/call", json!({ "name": name, "arguments": args }));
        let res = &r["result"];
        (res["isError"].as_bool().unwrap(), res["content"][0]["text"].as_str().unwrap().to_string())
    }

    #[test]
    fn speaks_mcp_and_runs_the_demo_flow() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("s.json"), JobManager::new(Arc::new(NoEvents)), "1.2.3");
        let init = call(&state, 1, "initialize", json!({ "protocolVersion": "2025-03-26", "capabilities": {} }));
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["version"], "1.2.3");
        assert!(handle_message(&state, &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none());
        let tools = call(&state, 2, "tools/list", json!({}));
        let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"run_pipeline") && !names.iter().any(|n| n.starts_with("delete")));
        assert_eq!(call(&state, 3, "nope", json!({}))["error"]["code"], -32601);

        let (err, text) = tool(&state, "list_datasets", json!({}));
        assert!(err && text.contains("No workspace"), "{text}");
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        assert!(!tool(&state, "open_workspace", json!({ "path": ws.display().to_string() })).0);
        let (err, text) = tool(&state, "create_demo_dataset", json!({}));
        assert!(!err && text.contains("\"Completed\""), "{text}");
        let name = crate::demo::DATASET_NAME;
        let (err, text) = tool(&state, "browse_documents", json!({ "dataset": name, "limit": 2 }));
        assert!(!err, "{text}");
        let (err, text) = tool(&state, "run_pipeline", json!({ "dataset": name, "pipeline": "demo-cleanup" }));
        assert!(!err && text.contains("\"Completed\""), "{text}");
        let (err, text) = tool(&state, "list_runs", json!({}));
        assert!(!err && text.contains("Demo cleanup"), "{text}");
    }
}
