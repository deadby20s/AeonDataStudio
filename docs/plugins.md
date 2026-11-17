# Plugins

A plugin is a program that a pipeline's **Plugin** step runs on every document. Use plugins
for anything the built-in steps don't do: model-based quality classifiers (fastText,
FineWeb-Edu), toxicity or topic filters, custom heuristics, or a tool you already have. They
can be written in any language. Python is the usual choice.

## Installing a plugin

Each plugin is a folder inside the workspace's `plugins/` folder (**Settings → Plugins → Open
plugins folder**) with a `plugin.toml`:

```
my-workspace/
  plugins/
    wordlist-filter/
      plugin.toml
      main.py
```

The folder name is the plugin's id. A pipeline step refers to it by that id:

```toml
[[steps]]
kind = "plugin"
plugin = "wordlist-filter"
options = ["words = casino, viagra", "max_per_1000_words = 5"]
```

Settings → Plugins, `aeon plugins` and the API's `GET /plugins` list the installed plugins and
their options. A `plugin.toml` that can't be read is listed with the error.

## plugin.toml

```toml
name = "Word list filter"
description = "Drops documents that use listed words too often."
command = ["python", "main.py"]   # started inside the plugin folder
workers = 4                       # processes per step (optional; default: up to 4)

[[options]]
key = "words"
default = "casino,viagra"
help = "Comma-separated words or phrases."
```

- `command[0]` is a file in the plugin folder if one exists there (e.g. `./classify.exe`).
  Otherwise it is looked up on `PATH` (`python`, `node`, …).
- A step's `options` lines (`key=value`) override the defaults.

## Protocol

The app starts `workers` processes for each Plugin step. It keeps them running for the whole
preview or run and stops them afterwards. It talks to them with JSON lines:

- **The app writes** one line per document to the plugin's stdin: `{"text": "..."}`.
- **The plugin answers** with exactly one line on stdout, in the same order:
  - `{}` or `{"keep": true}`: keep the document unchanged
  - `{"keep": false, "reason": "..."}`: drop it. The reason is shown in the Inspector, with
    rule `plugin`.
  - `{"text": "..."}`: keep it with new text.
- **Options** arrive as a JSON object in the `AEON_PLUGIN_OPTIONS` environment variable.
- **stderr** goes to the app log (Settings → Logs), so print debugging output there, never to
  stdout.

Before a run starts, each plugin is started once and sent a test document, so a missing
interpreter or a crash shows up immediately. If a plugin dies or answers with invalid JSON
during a run, the run stops with that error. It does not drop documents silently.

Flush stdout after each answer (`print(..., flush=True)` in Python). The app sets
`PYTHONUNBUFFERED=1` and `PYTHONIOENCODING=utf-8` for Python plugins.

### Minimal plugin (Python)

```python
import json, os, sys

opts = json.loads(os.environ.get("AEON_PLUGIN_OPTIONS", "{}"))
for line in sys.stdin:
    text = json.loads(line)["text"]
    if "lorem ipsum" in text.lower():
        print(json.dumps({"keep": False, "reason": "placeholder text"}), flush=True)
    else:
        print("{}", flush=True)
```

## Examples

The repository's [`examples/plugins`](../examples/plugins) folder has three examples to copy
into a workspace:

| Plugin | Needs | What it does |
|---|---|---|
| `wordlist-filter` | Python | Drops documents with too many listed words per 1,000 words |
| `fasttext-classifier` | `pip install fasttext-wheel numpy` and a `.bin` model | Keeps documents a fastText classifier labels as wanted (DCLM-style quality filtering) |
| `fineweb-edu-classifier` | `pip install transformers torch` | Scores educational value 0–5 with `HuggingFaceFW/fineweb-edu-classifier` and keeps 3+, as in FineWeb-Edu |

Only `wordlist-filter` is covered by tests. The other two are templates: they are
syntax-checked but haven't been run against their models.

## Performance

Each document is a round trip to a plugin process. With 4 Python workers, the
`wordlist-filter` example handles the 3,000-document demo in about 0.3 s. Model-based plugins
are limited by the model. Put them after cheap filters and deduplication, so fewer documents
reach them.

Dedup steps make extra passes over the data. Steps before a dedup step run once per pass, so
a plugin placed before a dedup step runs more than once per document. That works, but it costs
more.

## Security

A plugin is a program that runs with your permissions, so only install plugins you trust.
Plugins are never downloaded automatically.
