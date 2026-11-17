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

**HTML and WARC** *(added after the alpha)*: `.html`/`.htm` files are one document each;
`.warc` / `.warc.gz` web archives (Common Crawl format) give one document per WARC `response`
record that is an HTTP 200 HTML page (requests, metadata, redirects and non-HTML responses are
skipped). Main text is extracted with html5ever: `script`, `style`, `noscript`, `nav`,
`header`, `footer`, `aside`, `form` and hidden elements are dropped; headings, paragraphs, list
items, table cells and `pre` blocks become paragraphs; `<article>`/`<main>` is used when it holds
at least half of the page text. Metadata: `title`, and for WARC `url`, `date`; the WARC record
id becomes the document id.

*Planned:* source-code files.

## 2. Normalize *(built in M2)*

Make text consistent without changing meaning. Normalizers never drop a document, except one
left with no visible text (rule `empty_after_step`).

| Step (`kind`) | What it does |
|---|---|
| `normalize_unicode` | NFC (default) or NFKC. Skipped quickly when text is already normalized. |
| `clean_characters` | Fixes mojibake (UTF-8 read as Windows-1252, e.g. `cafÃ©` → `café`; only when the whole text can be reversed and the result has fewer telltale sequences), removes control characters (keeps `\n`, `\t`, `\r`) and zero-width characters, replaces non-breaking spaces. |
| `normalize_whitespace` | Unifies line endings to `\n`, trims trailing spaces, collapses runs of spaces/tabs (indentation kept), limits consecutive blank lines, trims the document. |

| `mask_pii` *(after the alpha)* | Replaces email addresses with `<EMAIL>`, phone numbers (9–15 digits in a phone-like format; dates excluded) with `<PHONE>`, IPv4 and full IPv6 addresses with `<IP>`, and 13–19-digit numbers passing the Luhn check with `<CARD>`. Each kind can be turned off. |

PII masking is pattern-based; it does not detect names or postal addresses.

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
| `c4_filter` *(0.2)* | C4 rules (Raffel et al., 2020): removes lines without end punctuation, with fewer than 3 words, with a word over 1,000 characters, mentioning javascript, or about cookies/privacy/terms; then drops documents with fewer than 5 sentences (counted by end punctuation), lorem ipsum, or `{`. Rules: `c4_min_sentences`, `c4_lorem_ipsum`, `c4_curly_bracket`, `c4_no_lines_left` |
| `fineweb_filter` *(0.2)* | FineWeb rules (Penedo et al., 2024): ≥ 12% of lines end in punctuation, ≤ 67% of lines are 30 characters or shorter, ≤ 1% of characters in repeated lines, ≤ 0.3 newlines per word |
| `blocklist_filter` | Whole-word / whole-phrase matches of `terms`; more than `max_hits` (0) drops the document |

Measured throughput in a release build (synthetic ~700-character documents, all cores):
about 147k documents/s (105 MB/s) for the whole template; language detection is the slowest
step at about 127 MB/s, every other step runs at 450 MB/s or more. Benchmark:
`cargo test --release -p aeon-core bench_steps -- --ignored --nocapture`.

### Plugins and language models *(0.3)*

| Step (`kind`) | What it does |
|---|---|
| `plugin` | Runs a program from the workspace's `plugins/` folder on each document. It can keep, drop (rule `plugin`, with the plugin's reason) or rewrite the document. See [plugins.md](plugins.md). |
| `llm_score` | Sends each document (cut to `max_input_chars`) to an OpenAI-compatible `/chat/completions` endpoint with `prompt` (`{text}` is replaced). It reads the first number of the answer as the score and drops documents below `min_score` (rule `llm_score_below`). The default prompt is a 0–5 educational-value scale, as in FineWeb-Edu. |
| `llm_rewrite` | Replaces each document with the model's answer to `prompt`. Use it to rephrase (WRAP, Maini et al., 2024), summarize, translate or generate synthetic data. |

How the LLM steps behave:
- **Server.** `endpoint` is the server's base URL; the default is Ollama, at
  `http://localhost:11434/v1`. `model` is sent as is.
- **API key.** `api_key_env` names an environment variable that holds the key. The key itself
  is never written to the recipe.
- **Retries.** Requests time out after 3 minutes. Network errors, 429 and 5xx responses are
  retried 3 times with backoff.
- **Failures.** `on_error` decides what happens after that: `stop` (stop the run with the
  error, the default), `keep` (keep the document) or `drop` (drop it with rule `llm_error`).
- **Thinking.** Steps ask the server not to think (`chat_template_kwargs.enable_thinking =
  false`, `reasoning_effort = "none"`), since reasoning models otherwise spend the answer
  tokens on hidden reasoning. A server that rejects these fields gets the request again
  without them.
- **Order.** LLM steps must come after every dedup step. Dedup passes re-run the steps before
  them, so a model there would see every document several times, and duplicates would be paid
  for.
- **Previews.** Previews of pipelines with LLM steps use at most 50 documents.

Measured with a small local model (Gemma 4 E2B, Q4, llama.cpp, one consumer GPU, 8 parallel
slots): 50 documents scored in 14 s.

### Data recipes *(0.3)*

Presets that prepare documents and then use a language model. Each one first normalizes,
applies a length filter (50–5,000 words) and runs exact dedup:

| Preset | Model step |
|---|---|
| `recipe_edu` | `llm_score` with the educational-value prompt, keeps scores of 3 and up |
| `recipe_rephrase` | `llm_rewrite` into clean encyclopedia style, keeping the facts |
| `recipe_qa` | `llm_rewrite` into 3–5 question-answer pairs grounded in the text |
| `recipe_summary` | `llm_rewrite` into a 3–5 sentence summary |

*Planned:* perplexity filtering, and tagging documents with scores instead of dropping them,
so thresholds can be chosen from the score distribution.

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

## 6. Mix *(built in M4)*

Combine datasets into a new one with per-dataset **weights**: weight *w* includes each
document ⌊w⌋ times plus once more with probability *w* − ⌊w⌋ (0.5 = a random half, 2 = twice).
The choice is a seeded hash of (dataset, row), so the same request gives the same mix. Extra
copies get ids like `p17#2`; sources are prefixed with the dataset name.

**Shuffle** is a two-pass external shuffle: documents are scattered into random temporary
buckets of about 64 MB (at most 512), then each bucket is shuffled in memory and appended.
Memory stays bounded and the order is uniformly random. The mixed dataset's manifest records
the request and each source's documents in/out and character share.

## 7. Tokenize *(built in M4, as an export format)*

Tokenizers: built-in tiktoken encodings (`o200k_base`, `cl100k_base`, `p50k_base`, `r50k_base`)
or a Hugging Face `tokenizer.json` (loaded with the `tokenizers` crate, pure-Rust regex).
Text is encoded without special tokens (a literal `<|endoftext|>` in a document stays text);
optionally the end-of-text id is appended after each document. For Hugging Face tokenizers the
end-of-text token is the first of `<|endoftext|>`, `</s>`, `<|end_of_text|>`, `<eos>`,
`<|eot_id|>`, `[SEP]` that exists.

## 8. Export *(built in M4)*

| Format | Files | Use |
|---|---|---|
| JSONL (none / gzip / zstd) | `data-NNNNN.jsonl[.gz/.zst]` | General purpose; `{"id","text","source","meta"?}` per line |
| Parquet | `data-NNNNN.parquet` | Hugging Face `datasets`, pandas, polars |
| Token shards | `tokens-NNNNN.bin` + `tokens-NNNNN.idx` | Direct loading by a training loop |

Token shards: `.bin` holds token ids as little-endian `uint16` (vocabulary ≤ 65,536) or
`uint32`; `.idx` holds `docs + 1` little-endian `uint64` offsets in tokens, so document *i* is
`tokens[idx[i]:idx[i+1]]`. Shards are split by the requested size (MiB of output; for
JSONL/Parquet, of uncompressed text).

Every export folder has `manifest.json` (source dataset and the run that produced it, request,
tokenizer name/vocabulary/dtype/end-of-text id, document and token counts, each file's size and
SHA-256, app version) and `SHA256SUMS` in `sha256sum` format. Exports are written to
`exports/.exporting-<name>` and renamed when complete.

Measured on 26,505 documents (34 MB of text, debug build): JSONL+zstd 1.0 s, Parquet 1.0 s,
o200k token shards 2.5 s (10.2 M tokens).

**End to end in a release build** (300k documents, 172 M characters): the full 11-step template
including exact and near dedup (3 passes) took 13.4 s; exporting the 264,903 kept documents as
o200k token shards (35 M tokens) took 1.5 s.

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
