# AeonDataStudio Documentation

AeonDataStudio is a personal desktop data studio for preparing text datasets for
pretraining AI models. It runs locally as a Tauri 2 app, with a Rust backend and a
Leptos (WebAssembly) frontend.

> **Status:** early. Milestones M0–M3 are done: workspaces, a background job runner,
> importing data into Parquet datasets with stats and a document browser, cleaning pipelines
> (normalizers, Gopher-style filters, exact and MinHash near-duplicate removal, preview, runs
> with removed-document logs), and an Inspector for tuning. Export (M4) is next. Anything not
> yet built is marked *planned*.

## Contents

| Document | What it covers |
|---|---|
| [Overview](overview.md) | Goals, non-goals, and the core concepts (datasets, pipelines, runs) |
| [Architecture](architecture.md) | How the frontend, backend and storage fit together, and where code should live |
| [Data pipeline](pipeline.md) | The pretraining data stages: ingest, normalize, filter, deduplicate, inspect, tokenize, export |
| [Development](development.md) | Prerequisites, running, building, and project conventions |
| [Roadmap](roadmap.md) | Milestones from starter app to a usable studio |

## Keeping these docs current

- When a *planned* feature lands, update its section and remove the marker.
- Record design decisions that are not obvious from the code in [architecture.md](architecture.md#decision-log).
