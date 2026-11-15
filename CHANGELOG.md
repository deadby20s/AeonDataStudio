# Changelog

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
