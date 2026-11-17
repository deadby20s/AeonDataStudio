# User Guide (0.1 alpha)

This guide walks through preparing a pretraining dataset with AeonDataStudio, from raw files
to training-ready shards. Everything happens on your machine; nothing is uploaded.

## 1. Create a workspace

A workspace is a folder that holds everything: datasets, pipelines, runs and exports.

1. Start the app and choose **Open or create workspace…**.
2. Pick an **empty folder** to create a new workspace, or a folder that already contains a
   `studio.toml` to open an existing one. (Non-empty folders that aren't workspaces are refused,
   so the app never scatters files into, say, your Documents folder.)

The app reopens the last workspace on start. **Settings → Close workspace** switches.

**Just trying the app?** In an empty workspace, **Datasets → Create demo dataset** (also in
Settings) adds 3,000 generated web pages with planted problems (duplicates, spam, menus, other
languages, broken encoding, …) and a matching **Demo cleanup** pipeline. Each document's
`meta.category` says what it contains, so you can check what each step caught.

## 2. Import data

**Datasets → Import…**

1. Add sources with **Add files…**, **Add folder…** (searched recursively), or paste a path and
   press Enter. Supported: JSONL/NDJSON, JSON arrays, plain text and Markdown, CSV/TSV, Parquet,
   **HTML pages** (one document per file) and **WARC web archives** such as Common Crawl's
   `.warc.gz` files (one document per HTML page); `.gz` and `.zst` are decompressed
   automatically. For HTML and WARC the main text is extracted (menus, headers, footers,
   scripts and the like are dropped) and the page title, URL and crawl date become metadata.
2. The **preview** samples the first file and lists its fields with examples and average length.
   The app picks the field with the longest text if the text field you typed doesn't exist;
   use **Use as text** / **Use as ID** to choose differently. The first few documents are shown
   exactly as they will be imported.
3. For plain text, choose whether a document is a whole file, a line, or a paragraph.
4. **Start import.** Progress shows in place. Broken lines and empty records are skipped and
   counted (they never stop an import); the dataset page lists them with examples.

## 3. Look at the data

Click a dataset to see document and character counts, length range, size on disk, a histogram
of document lengths, where it came from, and a **document browser**: page through documents,
click one to read it in full with its metadata, or **search** (case-insensitive; each request
searches for up to 2 seconds and tells you how far it got).

## 4. Clean it with a pipeline

**Pipelines → New pipeline** starts from a template with every step and recommended settings.

- **Normalize** steps change text: Unicode normalization, character cleanup (mojibake repair,
  invisible characters), whitespace.
- **Filter** steps remove documents: length, language, character ratios, Gopher word-quality
  and repetition rules, a blocklist.
- **Dedup** steps remove repeats: exact duplicates and near duplicates (MinHash). The first copy
  is kept.

Click a step to change its settings; leave a threshold empty to turn that rule off. Untick a
step to skip it, use ↑ ↓ to reorder. **Save** writes the recipe as TOML to `pipelines/`.

**Try it:** pick a dataset and **Preview on sample** (1,000–20,000 documents from across the
dataset). You'll see how many documents each step removes and why, examples of removed
documents, and before/after snippets for normalize steps. Adjust and preview again; this loop
is the point of the app. (Dedup in previews only compares documents within the sample.)

When it looks right, **Run on full dataset**. The run creates a **new dataset**; the original is
never changed.

## 5. Check what happened

- **Runs** lists every run. A run's page shows the per-step funnel; click a rule to list the
  documents it removed, and click one to read it.
- **Inspector** (or **Inspect** on a run) is for tuning:
  - **Removed by reason:** random examples for every rule. If good documents show up, loosen
    that threshold.
  - **Duplicates:** each removed copy next to the kept original, differences highlighted.
  - **Before / after:** input versus output for documents a normalize step changed.

**Open recipe in editor** on a run copies its exact settings into a new pipeline.

## 6. Mix datasets (optional)

**Datasets → Mix…**: tick datasets and set weights (1 = every document once, 0.5 = a random
half, 2 = twice). With **Shuffle**, the sources are interleaved. The same seed always gives the
same mix. The mixed dataset's page shows each source's share.

## 7. Export for training

Open a dataset and choose **Export…**:

| Format | Files | Notes |
|---|---|---|
| JSONL | `data-NNNNN.jsonl[.zst/.gz]` | `{"id", "text", "source", "meta"}` per line |
| Parquet | `data-NNNNN.parquet` | columns `id`, `text`, `source`, `meta` |
| Tokenized | `tokens-NNNNN.bin` + `.idx` | token ids (uint16 or uint32, little-endian) and uint64 document offsets |

Tokenizers: built-in tiktoken encodings (`o200k_base`, `cl100k_base`, `p50k_base`,
`r50k_base`/GPT-2) or any Hugging Face `tokenizer.json`. **Append end-of-text token** puts the
tokenizer's end-of-text id after every document.

Exports go to `exports/<name>/` in the workspace, or to a folder you choose with **Save to →
Choose folder…**, with `manifest.json` (source dataset and how it was made, settings,
tokenizer, counts, file list) and `SHA256SUMS` (`sha256sum -c SHA256SUMS` verifies the files).
**Exports** lists them all, with **Open folder** and **Delete** (for exports outside the
workspace, **Remove from list**: the app never deletes files outside the workspace).

Reading a token shard in Python:

```python
import numpy as np
tokens = np.fromfile("tokens-00000.bin", dtype=np.uint32)   # dtype from manifest.json
offsets = np.fromfile("tokens-00000.idx", dtype=np.uint64)
doc_3 = tokens[offsets[3]:offsets[4]]
```

## 8. Query with SQL

**SQL** runs DuckDB queries over the workspace: every dataset is a table `ds_<name>` (columns
`id`, `text`, `source`, `meta`; `meta` is JSON, so `meta->>'url'` works) and every run's removed
documents a table `removed_<run>`. Click a table to start a query, or pick an example (documents
per source, length distribution, filtering by metadata, text search). **Ctrl+Enter** runs, up to
1,000 rows are shown, and long queries can be cancelled. DuckDB uses half of your CPU cores.

## Updates

**Settings → Updates** checks for a new version and installs it (the app restarts). When one is
available, the sidebar says so. Builds made before updates were set up say so instead; download
new versions from the Releases page.

## Logs

**Settings → Open log folder** opens the folder with `AeonDataStudio.log`, which records each
job's start, end and errors. Please attach it to bug reports.

## Jobs

Imports, runs, mixes and exports run in the background; the **Jobs** page lists them with
progress and **Cancel**. Cancelled or failed jobs leave no partial files. Finished jobs have an
**Open** button for what they produced.

## Where things are on disk

```
workspace/
├── studio.toml        workspace settings
├── catalog.sqlite     list of datasets and runs
├── datasets/<id>/     manifest.json + Parquet parts
├── pipelines/         saved recipes (.toml)
├── runs/<id>/         report, recipe used, removed documents, kept index
└── exports/<name>/    training files + manifest.json + SHA256SUMS
```

## Known limitations of the alpha

- No benchmark decontamination or model-based quality filters yet.
- The document browser's search scans without an index (bounded to 2 seconds per page); the
  SQL page is faster for searching large datasets.
- Personal-information masking is pattern-based: emails, phone numbers, IPs and card numbers,
  not names or postal addresses.
- Near-duplicate removal trusts LSH band collisions without re-checking similarity.
- Tested on Windows 11. macOS and Linux installers are built by CI but not tested by hand.
