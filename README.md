# AeonDataStudio

A personal desktop data studio for preparing text data for pretraining AI models. Import raw
data, clean it with normalizers, quality filters and deduplication, see exactly what was removed
or changed and why, and export training-ready JSONL, Parquet or tokenized shards. Everything
runs locally.

**Status: 0.3 alpha.** The whole workflow works end to end; expect rough edges. See the
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
- **Extend** with plugins (any program that reads JSON lines) and language-model steps
  through any OpenAI-compatible server, including ready-made data recipes (educational-quality
  scoring, rephrasing, synthetic Q&A).
- **Automate** with the `aeon` CLI (installed with the app), a local HTTP API, and an MCP
  server for AI assistants.

Start with the **[user guide](docs/user-guide.md)**.

## Install

Download an installer from the [Releases page](https://github.com/deadby20s/AeonDataStudio/releases).
The installers are not code-signed yet: on Windows, SmartScreen may warn (More info → Run
anyway); on macOS, right-click the app and choose Open the first time.

Or build one yourself (installers end up in `target/release/bundle/`):

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
cargo test -p aeon-core -p aeon-cli -p aeondatastudio -p aeon-shared -p aeondatastudio-ui
```

Built with [Tauri 2](https://tauri.app) (Rust backend) and [Leptos](https://leptos.dev)
(WebAssembly frontend).

## Documentation

See [docs/](docs/README.md):

- [User guide](docs/user-guide.md): how to use the app
- [Overview](docs/overview.md): goals and core concepts
- [Architecture](docs/architecture.md): how the app is structured, and the decision log
- [Data pipeline](docs/pipeline.md): every stage, step and file format in detail
- [Plugins](docs/plugins.md): writing and installing plugins
- [CLI, API and MCP](docs/api.md): using the studio from scripts and AI assistants
- [Development](docs/development.md): setup and conventions
- [Roadmap](docs/roadmap.md)

## License

[MIT](LICENSE)
