# Architecture

## Current state (0.3)

The Cargo workspace has five crates:

| Crate | Path | Contents |
|---|---|---|
| `aeondatastudio-ui` | `src/` | Leptos 0.8 (client-side rendering) compiled to WASM by Trunk |
| `aeon-core` | `core/` | The engine, without UI: everything under "Backend" below, plus `ops.rs` (every operation as a method on `AppState`), `api.rs` (HTTP router and server), `mcp.rs` (MCP over stdio), `plugins.rs`, `llm.rs` |
| `aeon-cli` | `cli/` | The `aeon` CLI as a library: CLI commands, `aeon serve`, `aeon mcp`. The binary is `src-tauri/src/bin/aeon.rs` |
| `aeondatastudio` | `src-tauri/` | The Tauri app: native dialogs, updates, themes, the API switch, and thin commands over `aeon-core` |
| `aeon-shared` | `shared/` | Types used by all of them: `AppInfo`, `WorkspaceInfo`, `DatasetSummary`, `JobStatus`, pipeline specs, and the `JOB_UPDATE_EVENT` name |

One router (`api::handle`) serves the HTTP API, the MCP tools and most CLI commands, so they
can't drift apart. The app's commands call the same `AppState` methods directly.

- **Plugins** are processes started per step (a small pool), fed JSON lines.
- **LLM steps** call `/chat/completions` with `ureq`.
- **Failures.** Both report a final failure through the pipeline's shared `Abort`. Steps stop
  calling out, and the run or preview returns that error after the current batch.

**Frontend** (`src/`)
- `api.rs`: the only place that calls `invoke`. Declared with `catch`, so a failed command
  becomes `Err(ApiError)`. One typed function per command, plus `on_job_update` for events.
- `app.rs`: `AppCtx` (signals for workspace, recent list, jobs, current page) provided as
  context, and the shell (sidebar + page switcher). No router yet; the page is a signal.
- `pages.rs`: Welcome (open/create workspace), Datasets, Jobs, Settings, and placeholders for
  Pipelines, Runs and Inspector.
- Job rows are keyed by id and update in place. Rebuilding them on every progress event
  replaced the Cancel button between mouse-down and mouse-up, so clicks were lost.

**Backend** (`core/src/` since 0.3; it was `src-tauri/src/`)
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

- `dataset/parts.rs`: writing and reading a sequence of Parquet part files; shared by
  datasets and by runs' removed-document logs.
- `pipeline/`: `steps.rs` has the `Step` trait (`apply(&mut String) -> Unchanged | Changed |
  Drop { rule, detail }`) and every normalizer and filter; `mod.rs` compiles a recipe into
  steps, processes one document, and aggregates per-step statistics; `run.rs` is the run job;
  `preview.rs` samples and previews; `removed.rs` writes, reads and samples removed documents;
  `kept.rs` the kept-document index for before/after; `dedup.rs` exact hashing, MinHash LSH and
  on-disk band files; `storage.rs` saves recipes as TOML. A compiled step is either a
  per-document step or a dedup step; `Pipeline::process(text, row, decisions, stop_at)` drops
  rows listed in a dedup step's resolved `Decisions`, and `stop_at` halts at a dedup step for
  fingerprint passes.
- Shared crate: `pipeline.rs` defines recipes (`PipelineSpec`, `StepConfig`), run reports,
  and `STEP_KINDS`, a description of every step's settings that the editor uses to build
  its forms. Adding a step type means adding a variant, its `STEP_KINDS` entry, and its
  `Step` impl; no UI code.
- Jobs can finish with a link (`JobLink::Dataset` / `Run` / `Export`) shown as an "Open" button.
- `export/`: `mod.rs` writes JSONL/Parquet/token shards into `exports/<name>/` with manifest and
  checksums; `tokenize.rs` wraps tiktoken and Hugging Face tokenizers.
- `mix.rs`: weighted, seeded mixing with an external bucket shuffle.
- `import::preview` samples the first file's fields for the import form.
- Frontend: `exports.rs` (Exports page, export form, mix form); `inspector.rs` (removed-by-reason cards, duplicate pairs, before/after) and
  `diff.rs` (Myers word-level diff, whitespace split per character).

**Commands:** `get_app_info`, `pick_folder`, `open_workspace`, `close_workspace`,
`forget_recent_workspace`, `list_datasets`, `pick_import_files`, `pick_import_folder`,
`start_import`, `get_dataset`, `browse_documents`, `get_document`, `delete_dataset`,
`list_pipelines`, `get_pipeline`, `save_pipeline`, `delete_pipeline`, `preview_pipeline`,
`start_pipeline_run`, `list_runs`, `get_run`, `browse_removed`, `get_removed`, `delete_run`,
`list_jobs`, `cancel_job`, `clear_finished_jobs`, `start_self_test_job` (diagnostic job that
only reports progress). Commands that read files run on a blocking thread pool.

**Frontend** additions: `datasets.rs` holds the dataset list, import form, detail page
(stats, histogram, import report, lineage) and document browser; `pipelines.rs` the recipe
list, step editor and preview/run panel; `runs.rs` the run list, funnel and removed-document
browser. The draft being edited lives in the app context, so it survives switching pages.

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

Everything below exists today except `exports/`, which is still empty.

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
│       ├── report.json      # per-step counts, rules, timing, removed/kept parts
│       ├── removed/         # dropped docs with step + rule + detail (+ duplicated row)
│       └── kept/            # per output row: input row + which steps changed it
└── exports/<name>/          # data-* or tokens-* files, manifest.json, SHA256SUMS
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
| 2026-11-17 | `aeon` is a second binary of the app package, not of `aeon-cli` | Built separately, it got a different set of dependency features, so the release workflow recompiled every crate, DuckDB included (30 minutes on Windows). Built in the same Cargo run as the app, it only costs a link, and the installers include it |
| 2026-11-17 | Release caches are saved only by build-only runs on `main` | GitHub lets tags read caches saved on the default branch, not caches saved on other tags or pull requests |
| 2026-11-17 | The local API uses `tiny_http` on 127.0.0.1 with a bearer token, compared in constant time; no CORS headers | Small and synchronous like the engine. Loopback-only plus a token stops other users and web pages; without CORS headers, browsers can't read responses |
| 2026-11-17 | MCP tools never delete | An assistant may misread a request; deleting stays a human action in the app, CLI or API |
| 2026-11-17 | Plugins speak JSON lines over stdio, one process pool per step | Any language works, and nothing has to be linked into the app. Long-lived processes avoid paying model start-up per document |
| 2026-11-17 | LLM steps must come after dedup steps | Dedup passes re-run the steps before them; a model there would be called (and paid) several times per document |
| 2026-11-17 | A failing plugin or model step sets a shared abort flag, not a panic | A panic on every worker thread flooded the log and turned into "the job crashed"; the run now stops with one clear error |
| 2026-11-17 | API keys for LLM steps are read from a named environment variable | Recipes are shared and committed; a key in them would leak |
| 2026-11-17 | The engine moved to `aeon-core`, and operations to `ops.rs` | The CLI, API and MCP server need it without Tauri; one implementation keeps all four frontends in step |
| 2026-11-15 | CSP `connect-src` includes `'self'`; Trunk's `wasm-opt` is off and the UI crate uses `opt-level = "z"` | The frontend fetches its own `.wasm`, which a CSP without `'self'` blocks (only visible in release builds; dev doesn't enforce the CSP). `wasm-opt` is downloaded at build time and failed offline |
| 2026-11-15 | Export shard sizes and reported sizes use MiB | One unit everywhere (the UI's byte formatting is binary) |
| 2026-11-15 | `tokenizers` 0.22 with `fancy-regex`, not the 1.0 release candidate or Oniguruma | Stable release; pure-Rust regex avoids building a C library on Windows |
| 2026-11-15 | `serde_json` with `preserve_order` | Metadata keeps the source's key order, and the import preview lists fields in file order |
| 2026-11-15 | Not adopting DuckDB for the alpha | The per-document steps are Rust either way and storage is already Parquet, which DuckDB reads natively; it fits later as an optional SQL explorer, without migrating data |
| 2026-11-12 | Dedup keeps the lowest input row and runs as extra passes with on-disk band files sorted one band at a time | Deterministic "keep first" semantics and bounded memory (16 bytes per document per band in RAM for one band), at the cost of one extra read of the input per dedup step |
| 2026-11-12 | Near-dup uses LSH band collisions without verifying the Jaccard estimate | Storing full signatures would cost ~1 KB per document; band collisions alone give a well-understood detection curve (shown in the UI) |
| 2026-11-12 | Runs record a kept index (input row + changed-steps bitmask per output row) | Enables before/after in the Inspector for 16 bytes per document, without storing text twice |
| 2026-11-12 | In recipes, a missing threshold means "rule off"; recommended values are only applied when a step is added | TOML has no null, so "off" must be representable by absence; otherwise a rule switched off would come back on reload |
| 2026-11-12 | Steps work on one document at a time; batches are processed in parallel with rayon | All M2 steps only need the document itself, which makes parallelism trivial; corpus-wide steps (dedup, M3) will be separate passes |
| 2026-11-12 | A run stores removed documents with full text | Needed to inspect why documents were dropped; compressed Parquet keeps the cost reasonable |
| 2026-11-12 | Step settings are described once in the shared crate (`STEP_KINDS`) and the editor builds forms from that | New step types need no UI work, and the backend and UI can't disagree about field names |
| 2026-11-12 | Import never fails on a bad record, only on a bad file | One broken line in a 300k-line file shouldn't cost the whole import; a wrong column name affects every record, so it should fail fast with the available names |
| 2026-11-12 | Search scans with a 2 s budget per request instead of building an index | Simple and good enough for single-machine datasets; an index can come later if needed |
| 2026-11-12 | `zstd` dependency pinned to the version `parquet` uses | Two versions would each link the zstd C library and conflict |
| 2026-11-12 | Release profile has no `panic = "abort"`, and lives in the root `Cargo.toml` | Job threads need unwinding so a panic marks the job failed instead of killing the app. Cargo ignores profiles in member crates, so the old one in `src-tauri/Cargo.toml` never applied |
| 2026-11-12 | New workspaces only in empty folders | Picking the wrong folder must not litter it with workspace files |
| 2026-11-12 | One `job://update` event carrying the full `JobStatus` | Simpler than separate progress/done events; the UI just upserts |
| 2026-11-12 | Data processing runs in the Rust backend, not in the WASM frontend | Native threads, direct file access, no memory limits of the webview |
| 2026-11-12 | Datasets are immutable; runs produce new datasets | Reproducibility and the ability to compare before/after |
