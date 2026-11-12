# Architecture

## Current state (after M1)

The Cargo workspace has three crates:

| Crate | Path | Contents |
|---|---|---|
| `aeondatastudio-ui` | `src/` | Leptos 0.8 (client-side rendering) compiled to WASM by Trunk |
| `aeondatastudio` | `src-tauri/` | The Tauri app |
| `aeon-shared` | `shared/` | Types used by both: `AppInfo`, `WorkspaceInfo`, `DatasetSummary`, `JobStatus`, and the `JOB_UPDATE_EVENT` name |

**Frontend** (`src/`)
- `api.rs`: the only place that calls `invoke`. Declared with `catch`, so a failed command
  becomes `Err(ApiError)`. One typed function per command, plus `on_job_update` for events.
- `app.rs`: `AppCtx` (signals for workspace, recent list, jobs, current page) provided as
  context, and the shell (sidebar + page switcher). No router yet; the page is a signal.
- `pages.rs`: Welcome (open/create workspace), Datasets, Jobs, Settings, and placeholders for
  Pipelines, Runs and Inspector.
- Job rows are keyed by id and update in place. Rebuilding them on every progress event
  replaced the Cancel button between mouse-down and mouse-up, so clicks were lost.

**Backend** (`src-tauri/src/`)
- `commands.rs`: thin handlers, all returning `AppResult<T>`.
- `error.rs`: `AppError`, which serializes to its message string for the frontend.
- `state.rs`: `AppState` (open workspace, job manager) and `AppSettings` (recent workspaces,
  stored as `settings.json` in the OS app config dir). The last workspace reopens on startup.
- `workspace.rs`: opens a folder containing `studio.toml`, or initializes an **empty** folder.
  Non-empty folders without `studio.toml` are refused so a mis-click never scatters files
  into, say, Documents. Workspaces from a newer format version are refused.
- `catalog.rs`: SQLite catalog with `datasets` and `runs` tables; migrations tracked with
  `PRAGMA user_version` (append new migrations, never edit old ones).
- `jobs.rs`: `JobManager` runs each job on its own thread, emits `job://update` with the
  full `JobStatus` (throttled to 10/s per job), supports cancellation, and turns errors and
  panics into a `Failed` state. Keeps the last 50 finished jobs.

- `dataset/`: dataset storage. `writer.rs` streams documents into Parquet part files
  (new part every 250k rows or 512 MB of text, 16k-row row groups) and collects stats;
  `reader.rs` pages, searches and fetches single documents across parts;
  `mod.rs` has ids, manifests, and `TempDataset`, which writes into `datasets/.importing-<id>/`
  and renames into place only on success (leftovers are removed when a workspace opens).
- `import/`: `sources.rs` turns files into records (one reader per format, plus `.gz`/`.zst`
  decompression and progress counting); `mod.rs` is the import job.
- Dataset ids are restricted to `[a-z0-9-]` and checked before any path is built from them.

**Commands:** `get_app_info`, `pick_folder`, `open_workspace`, `close_workspace`,
`forget_recent_workspace`, `list_datasets`, `pick_import_files`, `pick_import_folder`,
`start_import`, `get_dataset`, `browse_documents`, `get_document`, `delete_dataset`,
`list_jobs`, `cancel_job`, `clear_finished_jobs`, `start_self_test_job` (diagnostic job that
only reports progress). Commands that read files run on a blocking thread pool.

**Frontend** additions: `datasets.rs` holds the dataset list, import form, detail page
(stats, histogram, import report) and document browser.

Measured on a 300k-document, 40 MB gzipped JSONL file (debug build): import 6.5 s, opening the
dataset 0.1 s, a page of documents 0.05 s, a full-dataset search with no hits 1.8 s.

## Target design

```
┌─────────────────────────── Tauri window ───────────────────────────┐
│  Frontend (Leptos / WASM)                                           │
│    pages: Datasets · Pipeline editor · Run monitor · Inspector      │
│    api/: typed wrappers around invoke + event listeners             │
└───────────────▲──────────────────────────────────┬─────────────────┘
                │ events (progress, logs)           │ commands (invoke)
┌───────────────┴──────────────────────────────────▼─────────────────┐
│  Backend (Rust, src-tauri)                                          │
│    commands/   thin Tauri command handlers                          │
│    jobs/       background job runner, cancellation, progress        │
│    pipeline/   step trait, step registry, executor                  │
│    io/         readers & writers (JSONL, CSV, Parquet, text, HTML)  │
│    store/      workspace layout, dataset catalog, metadata          │
└───────────────────────────────┬────────────────────────────────────┘
                                │
                        Workspace on disk
```

### Responsibilities

**Frontend.** Displays state and collects input. It never processes data itself; it asks the
backend for pages of documents, statistics and job status. Keep heavy logic out of WASM.

**Commands.** Short handlers that validate input, call into the core modules, and return
quickly. Anything that can take more than ~100 ms starts a job instead and returns a job id.

**Jobs.** Long work (import, pipeline runs, export) runs on background threads. Each job:
- reports its status through the `job://update` Tauri event (a separate log event may follow);
- can be cancelled via a shared cancellation flag checked between batches;
- writes its output to a temporary location and moves it into place only on success, so a
  crash or cancel never leaves a half-written dataset.

**Pipeline.** Each step implements a common trait, roughly:

```rust
trait Step: Send + Sync {
    fn name(&self) -> &str;
    /// Process one batch. Returns kept documents; dropped ones are reported with a reason.
    fn process(&self, batch: Vec<Doc>, ctx: &mut StepContext) -> Result<Vec<Doc>>;
}
```

Steps are created from a registry by name plus parameters, so pipelines can be stored as
plain data. Steps that need a global view (deduplication, corpus statistics) run as a
separate pass instead of per batch.

**IO and store.** Readers stream documents in batches; writers append batches. The store owns
the on-disk workspace layout and the catalog of datasets and runs.

### Shared types

Request/response types used by both crates live in the `aeon-shared` crate (`shared/`), so
they cannot drift apart. It depends only on `serde` and must keep compiling for both WASM and
native targets.

## Workspace layout on disk

`studio.toml`, `catalog.sqlite`, the four folders and `datasets/<id>/` (manifest + Parquet
parts) exist today; `runs/`, `pipelines/` and `exports/` are still empty.

The user picks a workspace folder. Inside it:

```
workspace/
├── studio.toml              # workspace settings
├── catalog.sqlite           # datasets, runs, lineage
├── pipelines/
│   └── web-clean.toml       # saved recipes
├── datasets/
│   └── <dataset-id>/
│       ├── manifest.json    # parts, stats, import report (hashes planned)
│       └── part-00000.parquet ...   # columns: id, text, source, meta
├── runs/
│   └── <run-id>/
│       ├── pipeline.toml    # frozen copy of the recipe used
│       ├── stats.json       # per-step counts, timing
│       └── removed/         # dropped docs with step + reason, for inspection
└── exports/
```

Parquet is the planned internal format: columnar, compressed, fast to page through for the
inspector, and readable by Python tooling (pandas, polars, Hugging Face `datasets`).

## Candidate crates

Crates marked *(in use)* are already dependencies; the rest guide future choices.

| Need | Candidates |
|---|---|
| Columnar storage | `arrow`, `parquet` *(in use)*; `polars` if analysis gets heavy |
| Catalog | `rusqlite` (bundled SQLite) *(in use)* |
| Parallelism | `rayon` |
| Hashing / dedup | `xxhash-rust`, `blake3`, a MinHash/LSH implementation |
| Language ID | `whatlang` or `lingua` |
| Unicode normalization | `unicode-normalization` |
| HTML to text | `html2text` or `scraper` |
| Tokenization | `tokenizers` (Hugging Face) or `tiktoken-rs` |
| File pickers | `tauri-plugin-dialog` *(in use)* |
| Config | `toml`, `serde` *(in use)* |

## Security notes

- `tauri.conf.json` currently sets `csp: null`. Set a strict CSP before loading any
  user-provided HTML into the UI (for example in the inspector).
- File access goes through backend commands that check paths are inside the workspace or a
  folder the user explicitly picked.
- Capabilities in `src-tauri/capabilities/` should grant only what the UI uses.

## Decision log

Record non-obvious decisions here, newest first.

| Date | Decision | Why |
|---|---|---|
| 2026-11-12 | Import never fails on a bad record, only on a bad file | One broken line in a 300k-line file shouldn't cost the whole import; a wrong column name affects every record, so it should fail fast with the available names |
| 2026-11-12 | Search scans with a 2 s budget per request instead of building an index | Simple and good enough for single-machine datasets; an index can come later if needed |
| 2026-11-12 | `zstd` dependency pinned to the version `parquet` uses | Two versions would each link the zstd C library and conflict |
| 2026-11-12 | Release profile has no `panic = "abort"`, and lives in the root `Cargo.toml` | Job threads need unwinding so a panic marks the job failed instead of killing the app. Cargo ignores profiles in member crates, so the old one in `src-tauri/Cargo.toml` never applied |
| 2026-11-12 | New workspaces only in empty folders | Picking the wrong folder must not litter it with workspace files |
| 2026-11-12 | One `job://update` event carrying the full `JobStatus` | Simpler than separate progress/done events; the UI just upserts |
| 2026-11-12 | Data processing runs in the Rust backend, not in the WASM frontend | Native threads, direct file access, no memory limits of the webview |
| 2026-11-12 | Datasets are immutable; runs produce new datasets | Reproducibility and the ability to compare before/after |
