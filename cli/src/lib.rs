//! `aeon`: AeonDataStudio without the window.
//!
//! The binary is `src-tauri/src/bin/aeon.rs`, a second binary of the desktop app package: the
//! app's release build compiles both in one Cargo run (same dependency features, so nothing is
//! built twice), and the installers include it. Run it with `cargo run --bin aeon -- ...`.
//! Works on the same workspace folders as the desktop app. Every command goes through the
//! local API router, so the CLI, the HTTP API, the MCP server and the app behave the same.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use aeon_core::api::{self, Request};
use aeon_core::jobs::{JobEvents, JobManager};
use aeon_core::state::{AppSettings, AppState};
use aeon_shared::{JobState, JobStatus};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(name = "aeon", version, about = "AeonDataStudio on the command line", long_about = None)]
struct Cli {
    /// Workspace folder. Defaults to $AEON_WORKSPACE, then the workspace last opened in the app.
    #[arg(short, long, global = true, env = "AEON_WORKSPACE")]
    workspace: Option<PathBuf>,

    /// Print raw JSON instead of tables.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show the workspace and app version.
    Info,
    /// Create a new workspace in an empty folder (or check an existing one).
    Init { folder: PathBuf },
    /// List datasets.
    Datasets,
    /// Show a dataset's manifest (statistics, sources, lineage).
    Show { dataset: String },
    /// Print documents from a dataset.
    Docs {
        dataset: String,
        #[arg(long, default_value_t = 0)]
        start: u64,
        #[arg(long, default_value_t = 5)]
        limit: u64,
        /// Only documents containing this text.
        #[arg(long)]
        query: Option<String>,
    },
    /// Import files or folders as a new dataset.
    Import {
        /// Dataset name.
        name: String,
        /// Files or folders.
        #[arg(required = true)]
        paths: Vec<String>,
        /// auto, jsonl, json, csv, tsv, parquet, text, html or warc.
        #[arg(long, default_value = "auto")]
        format: String,
        /// Field holding the text (JSON, CSV, Parquet).
        #[arg(long, default_value = "text")]
        text_field: String,
        /// Field holding the document id.
        #[arg(long)]
        id_field: Option<String>,
    },
    /// Generate the demo dataset and the "Demo cleanup" pipeline.
    Demo,
    /// List saved pipelines.
    Pipelines,
    /// List ready-made pipelines and data recipes.
    Presets,
    /// List pipeline step types and their settings.
    Steps,
    /// List plugins installed in the workspace's plugins folder.
    Plugins,
    /// Try a pipeline on a sample of a dataset (nothing is saved).
    Preview {
        /// Saved pipeline name (e.g. web-cleanup), a .toml file, or preset:<id>.
        pipeline: String,
        dataset: String,
        #[arg(long, default_value_t = 1000)]
        sample: u64,
    },
    /// Run a pipeline on a dataset, producing a new dataset.
    Run {
        /// Saved pipeline name (e.g. web-cleanup), a .toml file, or preset:<id>.
        pipeline: String,
        dataset: String,
        /// Name of the new dataset.
        #[arg(long, short)]
        output: Option<String>,
    },
    /// List pipeline runs.
    Runs,
    /// Show a run's report (what each step removed).
    Report { run: String },
    /// Export a dataset for training.
    Export {
        dataset: String,
        /// Export folder name.
        #[arg(long)]
        name: Option<String>,
        /// jsonl, parquet or tokens.
        #[arg(long, default_value = "jsonl")]
        format: String,
        /// jsonl: zstd, gzip or none.
        #[arg(long, default_value = "zstd")]
        compression: String,
        /// tokens: a tiktoken encoding (cl100k_base, o200k_base, …) or a tokenizer.json path.
        #[arg(long, default_value = "cl100k_base")]
        tokenizer: String,
        #[arg(long, default_value_t = 256)]
        shard_mb: u64,
        /// Leave out document metadata (jsonl, parquet).
        #[arg(long)]
        no_meta: bool,
        /// Folder to write into instead of the workspace's exports folder.
        #[arg(long)]
        out: Option<String>,
    },
    /// Run a SQL query over the workspace (DuckDB).
    Sql {
        query: String,
        #[arg(long, default_value_t = 50)]
        limit: u64,
    },
    /// Serve the local HTTP API (127.0.0.1 only, token required).
    Serve {
        #[arg(long, default_value_t = api::DEFAULT_PORT)]
        port: u16,
        /// Token clients must send. Defaults to $AEON_API_TOKEN, or a new random one.
        #[arg(long, env = "AEON_API_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
    /// Serve the MCP server on stdin/stdout (for Claude Desktop, Claude Code and other MCP clients).
    Mcp,
}

/// Prints job progress on one terminal line.
struct Progress;

impl JobEvents for Progress {
    fn job_updated(&self, s: &JobStatus) {
        if s.state.is_finished() {
            return;
        }
        let pct = s.progress.total.filter(|&t| t > 0).map(|t| format!(" {:>3}%", s.progress.done * 100 / t));
        let line = format!("{}{} {}", s.title, pct.unwrap_or_default(), s.message.as_deref().unwrap_or(""));
        let line: String = line.chars().take(100).collect();
        eprint!("\r{line:<100}");
        let _ = std::io::stderr().flush();
    }
}

fn config_dir() -> PathBuf {
    aeon_core::ops::default_settings_path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
}

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("error: {msg}");
    ExitCode::FAILURE
}

/// Shown when `aeon` is double-clicked: a console window that closed straight away after the
/// help text looked like a broken installer.
const DOUBLE_CLICK_MESSAGE: &str = "This is aeon, the AeonDataStudio command-line tool. It runs in a terminal; it is not the app
and not the app installer.

  - To install the app, download AeonDataStudio_<version>_x64-setup.exe from
    https://github.com/deadby20s/AeonDataStudio/releases
  - If the app is installed, start AeonDataStudio from the Start menu.
  - To use this tool, open PowerShell or Command Prompt and run:  aeon --help
";

/// True when this process is alone in its console window, which on Windows means it was
/// started by double-clicking (a terminal would share the console with its shell).
#[cfg(windows)]
fn started_by_double_click() -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
    }
    let mut ids = [0u32; 4];
    // SAFETY: the buffer is valid for `ids.len()` entries; the call only writes into it.
    let attached = unsafe { GetConsoleProcessList(ids.as_mut_ptr(), ids.len() as u32) };
    attached == 1
}

#[cfg(not(windows))]
fn started_by_double_click() -> bool {
    false
}

/// Parses the command line and runs it.
pub fn run() -> ExitCode {
    if std::env::args_os().len() == 1 && started_by_double_click() {
        println!("AeonDataStudio command-line tool {}
", env!("CARGO_PKG_VERSION"));
        print!("{DOUBLE_CLICK_MESSAGE}
Press Enter to close.");
        let _ = std::io::stdout().flush();
        let _ = std::io::stdin().read_line(&mut String::new());
        return ExitCode::SUCCESS;
    }
    let cli = Cli::parse();
    let quiet_progress = matches!(cli.command, Cmd::Mcp | Cmd::Serve { .. });
    let events: Arc<dyn JobEvents> =
        if quiet_progress { Arc::new(aeon_core::jobs::NoEvents) } else { Arc::new(Progress) };
    // The CLI keeps its own recent list, so it never reorders the app's.
    let state = Arc::new(AppState::new(
        config_dir().join("cli-settings.json"),
        JobManager::new(events),
        env!("CARGO_PKG_VERSION"),
    ));

    if let Cmd::Init { folder } = &cli.command {
        if !folder.exists()
            && let Err(e) = std::fs::create_dir_all(folder)
        {
            return fail(e);
        }
        return match state.open_workspace_path(&folder.display().to_string()) {
            Ok(info) => {
                println!("Workspace ready: {}", info.path);
                ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        };
    }

    // Find the workspace: flag / env, else the app's most recent one.
    let workspace = cli.workspace.clone().or_else(|| {
        let settings = AppSettings::load(&aeon_core::ops::default_settings_path()?);
        settings.recent_workspaces.first().map(PathBuf::from)
    });
    let needs_workspace = !matches!(cli.command, Cmd::Steps | Cmd::Presets | Cmd::Mcp);
    match &workspace {
        Some(w) => {
            if let Err(e) = state.open_workspace_path(&w.display().to_string())
                && needs_workspace
            {
                return fail(format!("could not open workspace {}: {e}", w.display()));
            }
        }
        None if needs_workspace => {
            return fail("no workspace: pass --workspace <folder>, set AEON_WORKSPACE, or open one in the app first");
        }
        None => {}
    }

    match execute(&cli, &state) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}

fn call(state: &AppState, method: &str, path: &str, query: &[(&str, Option<String>)], body: Value) -> Result<Value, String> {
    let query: BTreeMap<String, String> =
        query.iter().filter_map(|(k, v)| v.clone().map(|v| (k.to_string(), v))).collect();
    api::handle(state, &Request { method, path, query, body }).map_err(|e| e.message)
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Waits for a job, then reports how it ended.
fn finish(state: &AppState, started: Value) -> Result<Value, String> {
    let id = started["job_id"].as_u64().ok_or("no job id")?;
    let status = state.wait_job(id, None).map_err(|e| e.to_string())?;
    eprint!("\r{:<100}\r", "");
    let msg = status.message.clone().unwrap_or_default();
    match status.state {
        JobState::Completed => {
            eprintln!("✓ {}: {msg}", status.title);
            Ok(serde_json::to_value(&status).unwrap_or(Value::Null))
        }
        JobState::Cancelled => Err(format!("{} was cancelled", status.title)),
        _ => Err(format!("{} failed: {msg}", status.title)),
    }
}

/// A pipeline argument: `preset:<id>`, a path to a .toml file, or a saved pipeline name.
fn pipeline_body(state: &AppState, arg: &str) -> Result<Value, String> {
    if let Some(id) = arg.strip_prefix("preset:") {
        let p = aeon_shared::PRESETS.iter().find(|p| p.id == id).ok_or_else(|| {
            let ids: Vec<_> = aeon_shared::PRESETS.iter().map(|p| p.id).collect();
            format!("no preset {id:?}; presets: {}", ids.join(", "))
        })?;
        return Ok(json!({ "spec": (p.build)() }));
    }
    let path = Path::new(arg);
    if path.extension().is_some_and(|e| e == "toml") && path.is_file() {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let spec: aeon_shared::PipelineSpec = toml::from_str(&text).map_err(|e| format!("{arg}: {e}"))?;
        return Ok(json!({ "spec": spec }));
    }
    let _ = state;
    Ok(json!({ "pipeline": arg }))
}

fn table(rows: Vec<Vec<String>>) {
    if rows.is_empty() {
        return;
    }
    let cols = rows[0].len();
    let widths: Vec<usize> =
        (0..cols).map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0).min(60)).collect();
    for r in rows {
        let line: Vec<String> = r
            .iter()
            .zip(&widths)
            .map(|(cell, w)| {
                let cell: String = cell.chars().take(*w).collect();
                format!("{cell:<w$}", w = *w)
            })
            .collect();
        println!("{}", line.join("  ").trim_end());
    }
}

fn num(v: &Value) -> String {
    match v.as_u64() {
        Some(n) => {
            let s = n.to_string();
            let mut out = String::new();
            for (i, c) in s.chars().enumerate() {
                if i > 0 && (s.len() - i) % 3 == 0 {
                    out.push(',');
                }
                out.push(c);
            }
            out
        }
        None => v.as_str().unwrap_or("").to_string(),
    }
}

fn execute(cli: &Cli, state: &Arc<AppState>) -> Result<(), String> {
    let print = |v: &Value| println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    match &cli.command {
        Cmd::Init { .. } => unreachable!("handled before"),
        Cmd::Info => print(&call(state, "GET", "/info", &[], Value::Null)?),
        Cmd::Datasets => {
            let v = call(state, "GET", "/datasets", &[], Value::Null)?;
            if cli.json {
                print(&v);
            } else {
                let mut rows = vec![vec!["ID".into(), "NAME".into(), "DOCS".into(), "CHARS".into()]];
                for d in v.as_array().into_iter().flatten() {
                    rows.push(vec![num(&d["id"]), num(&d["name"]), num(&d["doc_count"]), num(&d["char_count"])]);
                }
                table(rows);
            }
        }
        Cmd::Show { dataset } => print(&call(state, "GET", &format!("/datasets/{}", enc(dataset)), &[], Value::Null)?),
        Cmd::Docs { dataset, start, limit, query } => {
            let v = call(
                state,
                "GET",
                &format!("/datasets/{}/documents", enc(dataset)),
                &[("start", Some(start.to_string())), ("limit", Some(limit.to_string())), ("query", query.clone())],
                Value::Null,
            )?;
            if cli.json {
                print(&v);
            } else {
                for d in v["docs"].as_array().into_iter().flatten() {
                    println!("── row {} · {} ──", d["row"], d["id"].as_str().unwrap_or(""));
                    println!("{}\n", d["preview"].as_str().unwrap_or(""));
                }
            }
        }
        Cmd::Import { name, paths, format, text_field, id_field } => {
            let abs: Vec<String> = paths
                .iter()
                .map(|p| std::path::absolute(p).map_or_else(|_| p.clone(), |a| a.display().to_string()))
                .collect();
            let body = json!({
                "name": name, "description": null, "paths": abs,
                "format": aeon_core::mcp::format_name(format), "text_field": text_field,
                "id_field": id_field, "text_mode": "WholeFile",
            });
            let started = call(state, "POST", "/imports", &[], body)?;
            let done = finish(state, started)?;
            if cli.json {
                print(&done);
            }
        }
        Cmd::Demo => {
            let started = call(state, "POST", "/demo", &[], Value::Null)?;
            finish(state, started)?;
        }
        Cmd::Pipelines => {
            let v = call(state, "GET", "/pipelines", &[], Value::Null)?;
            if cli.json {
                print(&v);
            } else {
                let mut rows = vec![vec!["FILE".into(), "NAME".into(), "STEPS".into()]];
                for p in v.as_array().into_iter().flatten() {
                    rows.push(vec![num(&p["file"]), num(&p["name"]), num(&p["step_count"])]);
                }
                table(rows);
            }
        }
        Cmd::Presets => {
            let v = call(state, "GET", "/presets", &[], Value::Null)?;
            if cli.json {
                print(&v);
            } else {
                for p in v.as_array().into_iter().flatten() {
                    println!("preset:{:<18} {}", p["id"].as_str().unwrap_or(""), p["label"].as_str().unwrap_or(""));
                }
            }
        }
        Cmd::Steps => {
            let v = api::step_kinds_json();
            if cli.json {
                print(&v);
            } else {
                for k in v.as_array().into_iter().flatten() {
                    println!("{:<24} [{}] {}", k["kind"].as_str().unwrap_or(""), k["category"].as_str().unwrap_or(""), k["label"].as_str().unwrap_or(""));
                }
            }
        }
        Cmd::Plugins => {
            let v = call(state, "GET", "/plugins", &[], Value::Null)?;
            if cli.json {
                print(&v);
            } else if v.as_array().is_none_or(|a| a.is_empty()) {
                println!("No plugins. Put plugin folders (each with a plugin.toml) in the workspace's plugins folder.");
            } else {
                for p in v.as_array().into_iter().flatten() {
                    let status = p["error"].as_str().map(|e| format!("  ⚠ {e}")).unwrap_or_default();
                    println!("{:<24} {}{status}", p["id"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or(""));
                }
            }
        }
        Cmd::Preview { pipeline, dataset, sample } => {
            let mut body = pipeline_body(state, pipeline)?;
            body["dataset"] = json!(dataset);
            body["sample_size"] = json!(sample);
            let v = call(state, "POST", "/preview", &[], body)?;
            if cli.json {
                print(&v);
            } else {
                println!("Sample: {} documents → {} kept", num(&v["sample_docs"]), num(&v["docs_out"]));
                print_steps(&v["steps"]);
            }
        }
        Cmd::Run { pipeline, dataset, output } => {
            let mut body = pipeline_body(state, pipeline)?;
            body["dataset"] = json!(dataset);
            body["output_name"] = json!(output.clone().unwrap_or_default());
            let started = call(state, "POST", "/runs", &[], body)?;
            let done = finish(state, started)?;
            if cli.json {
                print(&done);
            } else if let Some(run) = call(state, "GET", "/runs", &[], Value::Null)?.as_array().and_then(|r| r.first()) {
                println!("Run {}: {} → {} documents", run["id"].as_str().unwrap_or(""), num(&run["docs_in"]), num(&run["docs_out"]));
                if let Ok(report) = call(state, "GET", &format!("/runs/{}", enc(run["id"].as_str().unwrap_or(""))), &[], Value::Null) {
                    print_steps(&report["steps"]);
                }
            }
        }
        Cmd::Runs => {
            let v = call(state, "GET", "/runs", &[], Value::Null)?;
            if cli.json {
                print(&v);
            } else {
                let mut rows = vec![vec!["ID".into(), "PIPELINE".into(), "STATE".into(), "IN".into(), "OUT".into(), "OUTPUT".into()]];
                for r in v.as_array().into_iter().flatten() {
                    rows.push(vec![num(&r["id"]), num(&r["pipeline_name"]), num(&r["state"]), num(&r["docs_in"]), num(&r["docs_out"]), num(&r["output_name"])]);
                }
                table(rows);
            }
        }
        Cmd::Report { run } => {
            let v = call(state, "GET", &format!("/runs/{}", enc(run)), &[], Value::Null)?;
            if cli.json {
                print(&v);
            } else {
                println!("{}: {} → {} documents", v["pipeline"]["name"].as_str().unwrap_or(""), num(&v["docs_in"]), num(&v["docs_out"]));
                print_steps(&v["steps"]);
            }
        }
        Cmd::Export { dataset, name, format, compression, tokenizer, shard_mb, no_meta, out } => {
            let fmt = match format.as_str() {
                "jsonl" => "Jsonl",
                "parquet" => "Parquet",
                "tokens" => "Tokens",
                f => return Err(format!("unknown format {f:?} (jsonl, parquet, tokens)")),
            };
            let comp = match compression.as_str() {
                "zstd" => "Zstd",
                "gzip" => "Gzip",
                "none" => "None",
                c => return Err(format!("unknown compression {c:?} (zstd, gzip, none)")),
            };
            let tok = if fmt == "Tokens" {
                if tokenizer.ends_with(".json") {
                    let p = std::path::absolute(tokenizer).map_or_else(|_| tokenizer.clone(), |a| a.display().to_string());
                    json!({ "HuggingFace": p })
                } else {
                    json!({ "Tiktoken": tokenizer })
                }
            } else {
                Value::Null
            };
            let body = json!({
                "dataset_id": dataset,
                "name": name.clone().unwrap_or_else(|| format!("{dataset} {format}")),
                "format": fmt, "compression": comp, "shard_mb": shard_mb,
                "include_meta": !no_meta, "tokenizer": tok, "append_eos": true,
                "output_dir": out.as_ref().map(|o| std::path::absolute(o).map_or_else(|_| o.clone(), |a| a.display().to_string())),
            });
            let started = call(state, "POST", "/exports", &[], body)?;
            let done = finish(state, started)?;
            if cli.json {
                print(&done);
            }
        }
        Cmd::Sql { query, limit } => {
            let v = call(state, "POST", "/sql", &[], json!({ "sql": query, "limit": limit }))?;
            if cli.json {
                print(&v);
            } else {
                let mut rows = vec![v["columns"].as_array().into_iter().flatten().map(|c| c.as_str().unwrap_or("").to_string()).collect::<Vec<_>>()];
                for r in v["rows"].as_array().into_iter().flatten() {
                    rows.push(r.as_array().into_iter().flatten().map(|c| match c {
                        Value::String(s) => s.replace('\n', " "),
                        Value::Null => "NULL".into(),
                        other => other.to_string(),
                    }).collect());
                }
                table(rows);
            }
        }
        Cmd::Serve { port, token } => {
            let token = token.clone().unwrap_or_else(api::new_token);
            let server = api::ApiServer::start(state.clone(), *port, token).map_err(|e| e.to_string())?;
            println!("AeonDataStudio API on {}{}", server.url, api::PREFIX);
            println!("Token: {}", server.token);
            println!("Try:   curl -H \"Authorization: Bearer {}\" {}{}/datasets", server.token, server.url, api::PREFIX);
            println!("Press Ctrl+C to stop.");
            server.join();
        }
        Cmd::Mcp => aeon_core::mcp::serve_stdio(state).map_err(|e| e.to_string())?,
    }
    Ok(())
}

fn print_steps(steps: &Value) {
    let mut rows = vec![vec!["#".into(), "STEP".into(), "IN".into(), "DROPPED".into(), "CHANGED".into(), "TOP RULES".into()]];
    for s in steps.as_array().into_iter().flatten() {
        let rules: Vec<String> = s["drops_by_rule"]
            .as_array()
            .into_iter()
            .flatten()
            .take(3)
            .map(|r| format!("{} {}", r["rule"].as_str().unwrap_or(""), num(&r["count"])))
            .collect();
        rows.push(vec![
            (s["index"].as_u64().unwrap_or(0) + 1).to_string(),
            num(&s["label"]),
            num(&s["docs_in"]),
            num(&s["dropped"]),
            num(&s["modified"]),
            rules.join(", "),
        ]);
    }
    table(rows);
}
