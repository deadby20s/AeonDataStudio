# CLI, local API and MCP server

There are three ways to use AeonDataStudio without clicking through the app. All three use the
same engine (`aeon-core`) and work on the same workspace folders as the app:

- **`aeon`**, a command-line tool, for scripts and servers
- **The local HTTP API**, JSON over HTTP on `127.0.0.1`, for other programs
- **The MCP server**, so AI assistants (Claude Desktop, Claude Code, other MCP clients) can
  explore datasets, try pipelines and start runs

## The `aeon` CLI

`aeon` comes with the app:

| System | Location after installing |
|---|---|
| Windows | Next to `AeonDataStudio.exe` in the install folder |
| macOS | `/Applications/AeonDataStudio.app/Contents/MacOS/aeon` |
| Linux (`.deb` / `.rpm`) | `/usr/bin/aeon` |

Release pages also have it on its own (`aeon-windows-x64.exe`, `aeon-linux-x64`, …). To build
it from source, run `cargo build --release --bin aeon` (the binary ends up in
`target/release/`).

```bash
aeon init ./my-workspace                  # create a workspace in an empty folder
aeon -w ./my-workspace demo               # generate the demo dataset
aeon -w ./my-workspace import "Web crawl" crawl/*.jsonl.zst
aeon -w ./my-workspace datasets
aeon -w ./my-workspace docs "Web crawl" --limit 3 --query "recipe"
aeon -w ./my-workspace preview preset:fineweb "Web crawl" --sample 2000
aeon -w ./my-workspace run my-pipeline.toml "Web crawl" -o "Web crawl clean"
aeon -w ./my-workspace report <run-id>
aeon -w ./my-workspace export "Web crawl clean" --format tokens --tokenizer o200k_base
aeon -w ./my-workspace sql "select count(*) from ds_web_crawl_clean"
```

- **Workspace.** It comes from `--workspace`, then `$AEON_WORKSPACE`, then the workspace you
  last opened in the app.
- **Pipelines.** A pipeline argument can be a saved pipeline (`web-cleanup`), a `.toml` file,
  or `preset:<id>` (`aeon presets` lists them).
- **Datasets.** Give them by id or by name.
- **JSON output.** Add `--json` for raw JSON instead of tables.
- **Progress.** Jobs show progress on stderr and exit non-zero if they fail.

## Local HTTP API

Start it with `aeon serve` (port 7878, prints a token), or in the app under **Settings →
Local API → Turn on**. When the app serves it, jobs started through the API appear on the
Jobs page, and the setting is remembered.

- It listens on **127.0.0.1 only**.
- Every request needs the token, as `Authorization: Bearer <token>` (or `X-Api-Key`).
- The app keeps its token across restarts. **New token** replaces it.
- Bodies and responses are JSON. Errors are `{"error": "..."}` with a 4xx/5xx status.
- Dataset path segments can be ids or (URL-encoded) names.

```bash
TOKEN=...   # from aeon serve or Settings
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7878/api/v1/datasets
curl -H "Authorization: Bearer $TOKEN" -X POST http://127.0.0.1:7878/api/v1/runs \
     -d '{"pipeline": "web-cleanup", "dataset": "Web crawl"}'
curl -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:7878/api/v1/jobs/7?wait=600"
```

### Endpoints (prefix `/api/v1`)

| Method | Path | Body / query | Returns |
|---|---|---|---|
| GET | `/info` | | App version, open workspace |
| POST | `/workspace` | `{"path"}` | Opens a workspace |
| GET | `/datasets` | | Dataset list |
| GET | `/datasets/{id}` | | Manifest |
| GET | `/datasets/{id}/documents` | `start`, `limit` (≤ 500), `query` | A page of previews |
| GET | `/datasets/{id}/documents/{row}` | | One document |
| DELETE | `/datasets/{id}` | | Deletes the dataset |
| POST | `/imports` | `ImportRequest` | `{"job_id"}` |
| POST | `/imports/preview` | `{"paths", "format", "text_field", "text_mode"}` | Fields and first records |
| POST | `/demo` | | `{"job_id"}` |
| GET | `/steps` | | Step types, their settings and defaults |
| GET | `/presets` | | Ready-made pipelines and data recipes, as specs |
| GET | `/plugins` | | Installed plugins |
| GET | `/pipelines` | | Saved pipelines |
| GET | `/pipelines/{file}` | | A pipeline spec |
| POST | `/pipelines` | `{"spec", "file"?}` | `{"file"}` (new, or overwrites `file`) |
| DELETE | `/pipelines/{file}` | | |
| POST | `/preview` | `{"pipeline" or "spec", "dataset", "sample_size"}` | Preview statistics and examples |
| POST | `/runs` | `{"pipeline" or "spec", "dataset", "output_name"?}` | `{"job_id"}` |
| GET | `/runs` | | Run list |
| GET | `/runs/{id}` | | Run report |
| DELETE | `/runs/{id}` | | Deletes the run log (output dataset is kept) |
| GET | `/runs/{id}/removed` | `start`, `limit`, `step`, `rule` | Removed documents |
| GET | `/runs/{id}/removed/{index}` | | One removed document |
| GET | `/runs/{id}/sample` | `step`, `rule`, `count`, `seed` | Random removals by one rule |
| GET | `/runs/{id}/changed` | `step`, `start`, `limit` | Documents normalization changed |
| POST | `/exports` | `ExportRequest` | `{"job_id"}` |
| GET | `/exports` | | Export list |
| POST | `/mixes` | `MixRequest` | `{"job_id"}` |
| GET | `/sql/tables` | | Tables for SQL |
| POST | `/sql` | `{"sql", "limit"}` | Columns and rows |
| POST | `/sql/check` | `{"sql"}` | Parses and binds without running: `{"error", "line", "column"}` |
| GET | `/jobs` | | All jobs |
| GET | `/jobs/{id}` | `wait` (seconds, ≤ 600) | Job status; with `wait`, waits until it ends |
| POST | `/jobs/{id}/cancel` | | |

Request types are the ones in `shared/src`. Enum values are spelled as in Rust, for example:

```json
{"dataset_id": "Web crawl clean", "name": "crawl-tokens", "format": "Tokens",
 "compression": "None", "shard_mb": 256, "include_meta": false,
 "tokenizer": {"Tiktoken": "o200k_base"}, "append_eos": true, "output_dir": null}
```

`ImportRequest` example:

```json
{"name": "Web crawl", "description": null, "paths": ["D:/crawl"], "format": "Auto",
 "text_field": "text", "id_field": null, "text_mode": "WholeFile"}
```

## MCP server

`aeon mcp` speaks the Model Context Protocol over stdio. Add it to your MCP client. For
Claude Desktop, in `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "aeondatastudio": {
      "command": "C:\\Tools\\aeon.exe",
      "args": ["mcp", "--workspace", "D:\\data\\my-workspace"]
    }
  }
}
```

For Claude Code:

```bash
claude mcp add aeondatastudio -- aeon mcp --workspace /path/to/workspace
```

Tools:
- `studio_info`, `open_workspace`
- `list_datasets`, `get_dataset`, `browse_documents`, `get_document`
- `import_files`, `create_demo_dataset`
- `list_step_kinds`, `list_presets`, `list_plugins`
- `list_pipelines`, `get_pipeline`, `save_pipeline`, `preview_pipeline`, `run_pipeline`
- `list_runs`, `get_run`, `sample_removed`
- `export_dataset`, `list_exports`
- `run_sql`, `sql_tables`
- `get_job`, `cancel_job`

Long jobs wait for completion by default (`wait: false` returns the job id right away).

**MCP tools never delete anything.** Deleting datasets, runs or pipelines stays in the app,
the CLI and the HTTP API.
