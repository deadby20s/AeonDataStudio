# AeonDataStudio Documentation

AeonDataStudio is a personal desktop data studio for preparing text datasets for
pretraining AI models. It runs locally as a Tauri 2 app, with a Rust backend and a
Leptos (WebAssembly) frontend.

> **Status:** early. Milestone M0 is done: app shell, workspaces with a SQLite catalog, and a
> background job runner. No data processing exists yet. These docs describe the intended
> design so implementation has a clear target; anything not yet built is marked *planned*.

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
