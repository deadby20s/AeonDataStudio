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

## 1. Ingest

Read sources into a raw dataset with a common schema.

| Field | Type | Notes |
|---|---|---|
| `id` | string | Stable: hash of source path + offset, or an id field from the source |
| `text` | string | The content |
| `source` | string | Which source it came from |
| `meta` | map | Any extra fields kept from the input |

Supported inputs: `.txt` (one doc per file or per line), `.jsonl` / `.json` (choose the text
field), `.csv`, `.parquet`, `.md`, `.html`, and source code files. Compressed inputs
(`.gz`, `.zst`) are read transparently.

## 2. Normalize

Make text consistent without changing meaning.

- Fix encoding problems and invalid UTF-8.
- Unicode normalization (NFC by default, NFKC optional).
- Normalize line endings and whitespace; collapse runs of blank lines.
- Strip HTML/boilerplate (navigation, cookie banners) from web text.
- Optional PII masking: emails, phone numbers, IP addresses replaced with placeholders.

## 3. Filter

Drop low-quality documents. Each filter records *which rule* removed a document.

**Heuristic filters** (cheap, run first):
- Length: min/max characters or words.
- Language ID with a confidence threshold; keep a chosen set of languages.
- Character ratios: alphabetic, digit, symbol, uppercase, non-ASCII.
- Repetition: share of duplicate lines, paragraphs and n-grams inside one document.
- Line-level rules: lines ending in punctuation, bullet-only documents, "lorem ipsum",
  "javascript is disabled" and similar boilerplate.
- Word blocklists (configurable lists).

**Model-based filters** (optional, slower):
- Perplexity under a small reference language model.
- A quality classifier score.

Filters can either **drop** a document or **tag** it with a score field, so the threshold can be
chosen later by looking at the score distribution in the Inspector.

## 4. Deduplicate

Duplicates waste compute and increase memorization.

- **Exact:** hash of normalized text; keep the first occurrence.
- **Near-duplicate:** MinHash signatures over word n-grams with LSH banding; documents above a
  Jaccard similarity threshold (e.g. 0.8) are grouped and one per group is kept.
- **Sub-document (optional):** remove repeated paragraphs or long repeated substrings across the
  corpus.

Deduplication needs a view of the whole dataset, so it runs as its own pass and keeps its
index on disk to fit large datasets.

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

## Pipeline recipe format

Pipelines are saved as TOML so they can be read, diffed and kept in version control.

```toml
name = "web-clean"
version = 1

[[steps]]
kind = "normalize_unicode"
form = "NFC"

[[steps]]
kind = "length_filter"
min_chars = 200
max_chars = 100000

[[steps]]
kind = "language_filter"
keep = ["en"]
min_confidence = 0.65

[[steps]]
kind = "repetition_filter"
max_duplicate_line_fraction = 0.3

[[steps]]
kind = "minhash_dedup"
ngram = 5
num_perm = 128
threshold = 0.8
```

## Inspection

The Inspector is how you decide thresholds. For any dataset or run it shows:

- Document browser with search and paging.
- Per-step counts: in, kept, dropped, and the share of tokens removed.
- Removed documents grouped by step and reason, with random samples from each.
- Distributions (length, language, quality scores) as histograms.
- Before/after view of a single document through normalization steps.
