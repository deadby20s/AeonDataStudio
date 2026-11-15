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
- [x] Exact dedup.
- [x] MinHash + LSH near-duplicate dedup with an on-disk index.
- [x] Inspector: per-step funnel, removed samples by reason, duplicate pairs, before/after view.
- [ ] Follow-ups: sub-document (paragraph) dedup; optional Jaccard verification of LSH
      candidates; inspecting a dataset without a run.

## M4: Export
- [x] JSONL and Parquet export with manifests and checksums.
- [x] Tokenization (Hugging Face tokenizer / tiktoken) and binary token shards.
- [x] Dataset mixing with weights and seeded shuffle.
- [x] Import preview: fields, suggested text/id field, first documents.

## 0.1 alpha
- [x] App icon, content security policy, release build and installers.
- [x] User guide and changelog.

## After the alpha
- SQL explorer with DuckDB over datasets, runs and removed logs (Parquet is read natively, so
  no migration): ad-hoc queries, filtering by metadata, distributions of any field.
- Export to a folder outside the workspace; per-source token counts.
- Search index for large datasets.
- Cancel button for previews.

## Later
- PII masking.
- Benchmark decontamination.
- Model-based quality and perplexity filters.
- HTML/web archive (WARC) ingestion.
- Code-specific filters (license headers, generated files, minified code).
