# Roadmap

Milestones are ordered; each one should leave the app usable.

## M0: Foundation
- [x] Replace starter UI and placeholder metadata (title, description, authors, README).
- [x] App shell: sidebar navigation (Datasets, Pipelines, Runs, Inspector, Jobs, Settings).
- [x] Workspace selection and on-disk layout; SQLite catalog; recent workspaces remembered.
- [x] `shared` crate for types used by both frontend and backend.
- [x] Error-safe `invoke` wrapper and a typed `api` module.
- [x] Background job runner with progress events and cancellation.
- [x] Initialize git; commit `Cargo.lock`.

## M1: Import and browse
- [x] Import JSONL, JSON, TXT/MD, CSV/TSV, Parquet (including `.gz` / `.zst`) into Parquet datasets.
- [x] Dataset list with row counts, size, and source info; delete datasets.
- [x] Document browser with paging and text search.
- [x] Basic stats: length histogram, document and character counts.
- [ ] Follow-ups: preview the first records and fields before importing (today a wrong
      field name is only reported after the job starts); highlight search matches.

## M2: Cleaning pipeline
- [x] Step trait, registry, and executor with batching and `rayon` parallelism.
- [x] Normalization steps (Unicode, whitespace, encoding fixes).
- [x] Heuristic filters (length, language, character ratios, word quality, repetition, blocklists).
- [x] Pipeline editor UI; save/load TOML recipes; preview on a sample.
- [x] Runs record removed documents with step and reason; Runs page with funnel and removed-document browser.
- [ ] Follow-ups: time a full run in a release build; cancel button for a running preview.

## M3: Deduplication and inspection
- [ ] Exact dedup.
- [ ] MinHash + LSH near-duplicate dedup with an on-disk index.
- [ ] Inspector: per-step funnel, removed samples by reason, before/after view.

## M4: Export
- [ ] JSONL and Parquet export with manifests and checksums.
- [ ] Tokenization (Hugging Face tokenizer / tiktoken) and binary token shards.
- [ ] Dataset mixing with weights and seeded shuffle.

## Later
- PII masking.
- Benchmark decontamination.
- Model-based quality and perplexity filters.
- HTML/web archive (WARC) ingestion.
- Code-specific filters (license headers, generated files, minified code).
