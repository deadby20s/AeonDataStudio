# Changelog

## 0.2.0 alpha — 2026-11-17

Installers for all three systems, Common Crawl import, published filter presets, and SQL.

### Added
- Pipeline presets when creating a pipeline: Gopher-style, FineWeb-style, C4-style, Minimal.
- C4 filter (C4's line and document rules) and FineWeb filter (FineWeb's quality rules).
- Better HTML extraction: skips blocks named like menus, sidebars, cookie banners and "related"
  lists, and blocks that are mostly links.
- Installers for Windows, macOS (Apple silicon and Intel) and Linux, built by GitHub Actions;
  CI runs tests and lints on every change.
- Signed in-app updates (Settings → Updates), once the release signing key is set up.
- Import HTML pages and WARC web archives (Common Crawl) with main-text extraction.
- Mask personal information: emails, phone numbers, IP addresses, card numbers.
- SQL page: DuckDB queries over datasets and removed-document logs.
- Export to any folder.
- Cancel a running preview.
- Demo dataset with planted problems, and a log file (Settings → Open log folder).
- Issue templates for bug reports and feature requests.

### Measured
- 2 million documents through the full template with exact and near dedup on a GitHub
  runner: 6.9 minutes, 148 MB peak memory; dedup found the planted 5% exact and 5% near copies.

## 0.1.0 alpha — 2026-11-15

First usable version: the full path from raw files to training-ready shards.

### Data
- Workspaces: a folder holding datasets, pipelines, runs and exports, with a SQLite catalog.
- Import JSONL/NDJSON, JSON arrays, plain text/Markdown (per file, line or paragraph), CSV/TSV
  and Parquet, with `.gz`/`.zst` decompression and recursive folders. Bad records are skipped
  and reported; a preview shows a file's fields and picks the text field.
- Datasets stored as ZSTD Parquet with stats (counts, length range, length histogram) and a
  document browser with paging and time-bounded search.

### Cleaning
- Pipelines of steps saved as TOML: Unicode normalization, character cleanup (mojibake repair,
  invisible characters), whitespace normalization; length, language (whatlang), character
  ratio, Gopher word-quality and repetition filters, blocklist; exact and MinHash LSH
  near-duplicate removal (keeps the first copy, on-disk index).
- Preview on a sample spread across the dataset; runs as background jobs that create a new
  dataset and log every removed document with its step, rule and reason.
- Runs page with per-step funnel; Inspector with removed samples by reason, duplicate pairs and
  before/after diffs.

### Output
- Mix datasets with per-dataset weights and a seeded external shuffle.
- Export JSONL (optionally gzip/zstd), Parquet, or tokenized `uint16`/`uint32` shards with
  document offsets, using tiktoken encodings or a Hugging Face `tokenizer.json`. Every export
  has a manifest with lineage and SHA-256 checksums.

### App
- Background jobs with progress, cancellation and links to results; nothing half-written is
  left behind on cancel, failure or crash.
- Light and dark themes; app icon; content security policy.
