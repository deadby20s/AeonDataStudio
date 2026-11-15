# AeonDataStudio

A personal desktop data studio for preparing text data for pretraining AI models. Import raw
data, clean it with normalizers, quality filters and deduplication, see exactly what was removed
or changed and why, and export training-ready JSONL, Parquet or tokenized shards. Everything
runs locally.

**Status: 0.1 alpha.** The whole workflow works end to end; expect rough edges. See the
[changelog](CHANGELOG.md) and [roadmap](docs/roadmap.md).

## What it does

- **Import** JSONL, JSON, text/Markdown, CSV/TSV and Parquet (optionally `.gz`/`.zst`), with a
  preview that finds the text field for you.
- **Explore** datasets: stats, length histogram, document browser and search.
- **Clean** with pipelines: Unicode/character/whitespace normalization; length, language,
  character-ratio, Gopher quality and repetition filters; blocklists; exact and MinHash
  near-duplicate removal. Preview on a sample, then run on the full dataset.
- **Inspect** every run: removed samples per rule, duplicate pairs side by side, before/after diffs.
- **Mix** datasets with weights and a seeded shuffle.
- **Export** JSONL/Parquet or token shards (tiktoken or any Hugging Face tokenizer), with a
  manifest and SHA-256 checksums.

Start with the **[user guide](docs/user-guide.md)**.

## Install

Build an installer (Windows: `.msi` and `.exe` in `target/release/bundle/`):

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk tauri-cli
cargo tauri build
```

## Develop

```bash
cargo tauri dev
```

```bash
cargo test -p aeondatastudio -p aeon-shared -p aeondatastudio-ui
```

Built with [Tauri 2](https://tauri.app) (Rust backend) and [Leptos](https://leptos.dev)
(WebAssembly frontend).

## Documentation

See [docs/](docs/README.md):

- [User guide](docs/user-guide.md): how to use the app
- [Overview](docs/overview.md): goals and core concepts
- [Architecture](docs/architecture.md): how the app is structured, and the decision log
- [Data pipeline](docs/pipeline.md): every stage, step and file format in detail
- [Development](docs/development.md): setup and conventions
- [Roadmap](docs/roadmap.md)
