# AeonDataStudio

A personal desktop data studio for preparing text data for pretraining AI models: import raw
data, clean and filter it, remove duplicates, inspect the results, and export training-ready
shards. Everything runs locally.

Built with [Tauri 2](https://tauri.app) (Rust backend) and [Leptos](https://leptos.dev)
(WebAssembly frontend).

> Early stage: you can import data (JSONL, JSON, text, CSV/TSV, Parquet, optionally `.gz`/`.zst`), see stats, and browse and search documents. Cleaning pipelines come next. See the [roadmap](docs/roadmap.md).

## Quick start

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk tauri-cli
cargo tauri dev
```

## Documentation

See [docs/](docs/README.md):

- [Overview](docs/overview.md): goals and core concepts
- [Architecture](docs/architecture.md): how the app is structured
- [Data pipeline](docs/pipeline.md): the pretraining data stages
- [Development](docs/development.md): setup and conventions
- [Roadmap](docs/roadmap.md)

## Recommended IDE setup

[VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer).
