# Data Pipeline

This is the processing model for turning raw text into pretraining data. All stages are
*planned* unless noted otherwise.

Stages run in this order by default, but a pipeline is just a list of steps, so any subset
and order is allowed.

```
Ingest → Normalize → Filter → Deduplicate → (Decontaminate) → Mix → Tokenize → Export
                       ↘          ↘                ↘
                        removed-doc logs with reasons, used by the Inspector
```

## 1. Ingest *(built in M1)*

Read sources into a dataset with a common schema, stored as Parquet (ZSTD-compressed):

| Column | Type | Notes |
|---|---|---|
| `id` | string | From the chosen ID field, otherwise `<source>:<record number>` (stable across re-imports) |
| `text` | string | The content |
| `source` | string | File it came from, relative to the picked folder (e.g. `corpus/sub/a.jsonl`) |
| `meta` | string | JSON object with every other field/column of the record (`{}` when none) |

**Inputs** (format detected from the extension, or chosen explicitly):

| Format | Extensions | Text taken from |
|---|---|---|
| JSONL | `.jsonl`, `.ndjson` | The text field of each line's object |
| JSON array | `.json` | The text field of each object in the array (whole file is loaded into memory) |
| Plain text | `.txt`, `.text`, `.md`, `.markdown` | Whole file, each line, or each blank-line-separated paragraph |
| CSV / TSV | `.csv`, `.tsv` (header row required) | The text column |
| Parquet | `.parquet` | The text column (any type castable to string) |

`.gz` and `.zst` suffixes are decompressed on the fly (not for Parquet). Folders are walked
recursively in name order; hidden files and files of unknown type are skipped and counted.
Invalid UTF-8 is replaced with U+FFFD; proper encoding repair belongs to Normalize.

**What is skipped**, counted per file in the import report: records without the text field
(or where it is not a string), records whose text is empty or whitespace, and lines/rows that
cannot be parsed (the first 10 are kept as examples). A file-level problem such as a missing
column fails the whole import with a message naming the available fields.

**Stats** computed during import and stored in the manifest: document and character counts,
text bytes, shortest/longest document, and a length histogram with power-of-two bins
(0-64, 64-128, ... >=128K characters).

*Planned:* HTML extraction, source-code files, WARC.

## 2. Normalize *(built in M2)*

Make text consistent without changing meaning. Normalizers never drop a document, except one
left with no visible text (rule `empty_after_step`).

| Step (`kind`) | What it does |
|---|---|
| `normalize_unicode` | NFC (default) or NFKC. Skipped quickly when text is already normalized. |
| `clean_characters` | Fixes mojibake (UTF-8 read as Windows-1252, e.g. `cafÃ©` → `café`; only when the whole text can be reversed and the result has fewer telltale sequences), removes control characters (keeps `\n`, `\t`, `\r`) and zero-width characters, replaces non-breaking spaces. |
| `normalize_whitespace` | Unifies line endings to `\n`, trims trailing spaces, collapses runs of spaces/tabs (indentation kept), limits consecutive blank lines, trims the document. |

*Planned:* HTML/boilerplate stripping, PII masking.

## 3. Filter *(heuristic filters built in M2)*

Drop low-quality documents. A document stops at the first rule it fails; the step, rule
name and a detail such as `12 words < 50` are recorded. Thresholds left empty turn that rule
off. The recommended values come from DeepMind's Gopher paper (Rae et al., 2021).

| Step (`kind`) | Rules |
|---|---|
| `length_filter` | `min_chars`, `max_chars`, `min_words` (50), `max_words` (100,000) |
| `language_filter` | Detected language (whatlang, first 2,000 chars) must be in `keep` (ISO 639-3, e.g. `eng`) with at least `min_confidence` (0.5). Rules: `language`, `language_confidence`, `language_unknown`. |
| `character_ratio_filter` | Over non-whitespace characters: `min_alpha_ratio` (0.5), `max_digit_ratio` (0.3), `max_symbol_ratio` (0.3), `max_uppercase_ratio` over letters (0.5), `max_replacement_ratio` for U+FFFD (0.01) |
| `word_quality_filter` | Mean word length 3–10, ≥ 80% of words contain a letter, ≤ 0.1 `#`/`…` per word, ≤ 90% bullet lines, ≤ 30% lines ending in `…` |
| `repetition_filter` | Duplicate lines ≤ 0.3, duplicate paragraphs ≤ 0.3, top 2/3/4-gram character share ≤ 0.20/0.18/0.16, characters in duplicated 5/10-grams ≤ 0.15/0.10 |
| `blocklist_filter` | Whole-word / whole-phrase matches of `terms`; more than `max_hits` (0) drops the document |

Measured throughput in a release build (synthetic ~700-character documents, all cores):
about 147k documents/s (105 MB/s) for the whole template; language detection is the slowest
step at about 127 MB/s, every other step runs at 450 MB/s or more. Benchmark:
`cargo test --release -p aeondatastudio bench_steps -- --ignored --nocapture`.

*Planned:* model-based filters (perplexity under a small reference model, a quality
classifier), and tagging documents with scores instead of dropping them so thresholds can be
chosen from the score distribution.

## 4. Deduplicate *(built in M3)*

Duplicates waste compute and increase memorization. Dedup steps go in a pipeline like any
other step; the **first occurrence (lowest input row) is kept** and later copies are removed
with rule `exact_duplicate` or `near_duplicate`, recording which row they duplicate.

| Step (`kind`) | How it works |
|---|---|
| `exact_dedup` | xxh3 hash of the text (with `normalize`, lowercased and whitespace-collapsed first). |
| `near_dedup` | MinHash over word n-grams (`ngram_size` 5; words lowercased, punctuation removed), split into `bands` × `rows_per_band` (14 × 8). Two documents are duplicates if all hashes of any band match. A pair with n-gram Jaccard similarity *s* is caught with probability 1 − (1 − s⁸)¹⁴: about 56% at 0.7, 92% at 0.8, over 99% at 0.9. |

Dedup only considers documents that reach the step, so put it after the filters (it then
also compares cleaned text). Duplicates are transitive in effect: if B copies A and C copies
B, both B and C are removed.

**How a run does it.** Each dedup step needs the whole dataset, so a run makes one extra pass
per dedup step. The pass processes documents up to that step, writes one `(band hash, row)`
pair per band to `runs/<id>/dedup-<k>/band-NN.bin` (16 bytes per document per band), then sorts
each band file in turn to find the rows sharing a hash with an earlier row. Memory use is one
band at a time; the band files are deleted afterwards. A run with exact + near dedup makes 3
passes in total, shown in its progress.

Measured on 30,000 documents with planted copies: 995 of 1,000 near copies (1–2 words changed or
a footer added) and every exact copy were found. Fingerprinting speed in a release build:
about 2.1 GB/s for exact, 150 MB/s for MinHash.

**In previews**, dedup steps only compare documents within the sample, so a full run usually
finds more.

*Planned:* sub-document dedup (repeated paragraphs or long substrings across the corpus).

## 5. Decontaminate (optional)

Remove documents that overlap with evaluation benchmarks, by n-gram overlap against
benchmark files the user provides. Keeps evaluation scores honest.

## 6. Mix

Combine several clean datasets into one training mix with per-source weights (upsample or
downsample), and shuffle with a fixed seed.

## 7. Tokenize

Optional. Tokenize with a chosen tokenizer (a Hugging Face `tokenizer.json` or a tiktoken
encoding), append an end-of-document token, and report token counts per source.

## 8. Export

| Format | Use |
|---|---|
| JSONL (optionally `.zst`) | General purpose, easy to read |
| Parquet | Hugging Face `datasets`, pandas, polars |
| Binary token shards (`uint16`/`uint32` + index) | Direct loading by a training loop |

Every export includes a `manifest.json`: source datasets, pipeline recipe, tool version,
tokenizer, document and token counts, and file checksums.

## Pipeline recipe format *(built in M2)*

Pipelines are saved as TOML in the workspace's `pipelines/` folder, so they can be read,
diffed and kept in version control. Each run also stores the exact recipe it used in
`runs/<id>/pipeline.toml`.

```toml
name = "Web cleanup"
description = "Normalization plus Gopher-style quality filters for web text."

[[steps]]
enabled = true
kind = "normalize_unicode"
form = "nfc"

[[steps]]
enabled = true
kind = "language_filter"
keep = ["eng"]
min_confidence = 0.5

[[steps]]
enabled = false          # kept in the recipe, skipped when running
kind = "length_filter"
min_words = 50           # max_words is absent, so that rule is off
```

A missing threshold means the rule is **off** (TOML has no null). The recommended values are
only filled in when a step is added in the editor.

## Running a pipeline *(built in M2)*

- **Preview** runs the recipe (including unsaved edits) on a sample of 1,000–20,000 documents
  taken from 20 evenly spaced places in the dataset, and shows per-step counts plus up to 3
  examples per removal rule and before/after snippets for normalizers.
- **Run** processes the whole dataset as a background job (batches of 4,096 documents,
  processed in parallel) and creates a new dataset; the input is never modified. The run
  records per-step statistics and every removed document with its step, rule, detail and
  text (as it was when removed) in `runs/<id>/removed/*.parquet`.
- The Runs page shows the per-step funnel; clicking a rule lists the documents it removed.
- *(M3)* The run also records, for every kept document, its input row and which steps changed
  it (`runs/<id>/kept/`), so the Inspector can show before/after.

## Inspection

*Built in M1:* each dataset's page shows its stats, length histogram, import report, and a
document browser (25 per page, click to read the full text and metadata). Search is a
case-insensitive substring match that scans forward from the current position; each request
stops after 2 seconds and reports how far it got, so searching a large dataset never freezes
the app.

*Built in M3:* the **Inspector** page is how you decide thresholds. For a completed run it shows:

- The per-step funnel.
- **Removed by reason:** a card per (step, rule) with its count, its share of the step's input and
  of all removals, and 3 random examples (“Other examples” draws again; samples are
  reproducible). Only the small columns are scanned to find candidates, so this stays fast on
  large removal logs.
- **Duplicates:** pairs of removed copy and kept original, with a word-level diff highlighting
  what differs, to judge whether the near-duplicate threshold is right.
- **Before / after:** kept documents changed by normalization (optionally by one step), with a
  diff of input versus output that marks individual characters of whitespace changes.
  Runs made before M3 have no kept index; run them again to use this view.

*Planned:* distributions of quality scores, and inspecting a dataset without a run.
