# AeonDataStudio Documentation

AeonDataStudio is a personal desktop data studio for preparing text datasets for
pretraining AI models. It runs locally as a Tauri 2 app, with a Rust backend and a
Leptos (WebAssembly) frontend.

> **Status:** alpha (0.3 in progress). The full workflow works: import (with field preview),
> explore, clean with pipelines (normalizers, Gopher/C4/FineWeb filters, exact and
> near-duplicate removal, plugins, language-model steps), inspect, mix, and export
> JSONL/Parquet/token shards with checksums. The same engine runs from the `aeon` CLI, a local
> HTTP API and an MCP server. Anything not yet built is marked *planned*.

## Contents

| Document | What it covers |
|---|---|
| [User guide](user-guide.md) | How to use the app, step by step |
| [Overview](overview.md) | Goals, non-goals, and the core concepts (datasets, pipelines, runs) |
| [Architecture](architecture.md) | How the frontend, backend and storage fit together, and where code should live |
| [Data pipeline](pipeline.md) | The pretraining data stages: ingest, normalize, filter, deduplicate, inspect, tokenize, export |
| [Plugins](plugins.md) | Writing and installing plugins for the Plugin step |
| [CLI, API and MCP](api.md) | The `aeon` command-line tool, the local HTTP API and the MCP server |
| [Development](development.md) | Prerequisites, running, building, and project conventions |
| [Roadmap](roadmap.md) | Milestones from starter app to a usable studio |

## Keeping these docs current

- When a *planned* feature lands, update its section and remove the marker.
- Record design decisions that are not obvious from the code in [architecture.md](architecture.md#decision-log).
