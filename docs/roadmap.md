# Roadmap

Milestones are ordered; each one should leave the app usable.

## M0: Foundation
- [x] Replace starter UI and placeholder metadata (title, description, authors, README).
- [x] App shell: sidebar navigation (Datasets, Pipelines, Runs, Inspector, Jobs, Settings).
- [x] Workspace selection and on-disk layout; SQLite catalog; recent workspaces remembered.
- [x] `shared` crate for types used by both frontend and backend.
- [x] Error-safe `invoke` wrapper and a typed `api` module.
- [x] Background job runner with progress events and cancellation.
- [ ] Initialize git; commit `Cargo.lock`. (`.gitignore` no longer excludes `Cargo.lock`; `git init` still to do.)

## M1: Import and browse
- [ ] Import JSONL, TXT, CSV, Parquet (including `.gz` / `.zst`) into Parquet datasets.
- [ ] Dataset list with row counts, size, and source info.
- [ ] Document browser with paging and text search.
- [ ] Basic stats: length histogram, document and character counts.

## M2: Cleaning pipeline
- [ ] Step trait, registry, and executor with batching and `rayon` parallelism.
- [ ] Normalization steps (Unicode, whitespace, encoding fixes).
- [ ] Heuristic filters (length, language, character ratios, repetition, blocklists).
- [ ] Pipeline editor UI; save/load TOML recipes.
- [ ] Runs record removed documents with step and reason.

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
