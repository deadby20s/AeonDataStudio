# Overview

## Purpose

Pretraining a language model needs large amounts of clean, deduplicated, well-understood
text. Getting there usually means a pile of one-off scripts. AeonDataStudio replaces those
scripts with a single local app where you can:

1. **Bring in raw data** from local files (text, JSONL, CSV, Parquet, HTML, Markdown, code).
2. **Clean and filter it** with configurable, repeatable steps.
3. **Remove duplicates**, both exact and near-duplicate.
4. **Inspect it**: browse samples, see statistics, and check what each step removed and why.
5. **Export it** as training-ready shards, optionally tokenized.

It is a *personal* tool: single user, runs offline on one machine, and keeps all data local.

## Goals

- **Reproducible.** A pipeline is a saved recipe. Running the same recipe on the same input
  gives the same output, and every output records the recipe and versions that produced it.
- **Transparent.** Every document that gets dropped or changed can be traced to the step
  and rule responsible. Looking at the data is a first-class feature, not an afterthought.
- **Scales past RAM.** Processing streams over data in batches, so datasets larger than
  memory work on a desktop machine.
- **Fast.** Heavy work runs in Rust on the backend, in parallel, off the UI thread.
- **Private.** No telemetry and no network access needed to process data.

## Non-goals

- Training or fine-tuning models.
- Multi-user collaboration or a hosted service.
- Distributed processing across a cluster. Very large corpora (many TB) are out of scope;
  the target is what one workstation can handle.
- Labeling/annotation workflows for supervised data (may come later, not a focus).

## Core concepts

| Concept | Meaning |
|---|---|
| **Source** | A reference to raw input: a file, folder or glob, plus how to read it (format, text field, encoding). |
| **Dataset** | A versioned collection of documents stored by the studio. Created by importing sources or by running a pipeline. Never modified in place. |
| **Document** | One unit of text plus metadata (`id`, `text`, `source`, `lang`, custom fields). |
| **Step** | One operation in a pipeline, e.g. "normalize Unicode", "drop docs under 200 chars", "MinHash dedup". Each step has typed parameters. |
| **Pipeline** | An ordered list of steps saved as a recipe file. |
| **Run** | One execution of a pipeline over an input dataset. Produces an output dataset, a log of removed documents with reasons, and statistics. |
| **Export** | Writing a dataset out in a training format (JSONL, Parquet, tokenized binary shards). |

## Typical workflow

```
import sources ──► raw dataset ──► run pipeline ──► clean dataset ──► inspect ──► export shards
                                        ▲                                │
                                        └──── adjust steps/thresholds ◄──┘
```

The loop in the middle is where most time goes: try thresholds, look at what got removed,
adjust, run again. The studio is designed to make that loop short.
