"""AeonDataStudio plugin: drop documents that use listed words too often.

Protocol: one JSON object per line on stdin ({"text": ...}); answer one line per document:
{} to keep, {"keep": false, "reason": "..."} to drop, {"text": "..."} to replace the text.
Options arrive as JSON in the AEON_PLUGIN_OPTIONS environment variable.
"""
import json
import os
import re
import sys

opts = json.loads(os.environ.get("AEON_PLUGIN_OPTIONS", "{}"))
words = [w.strip().lower() for w in opts.get("words", "").split(",") if w.strip()]
limit = float(opts.get("max_per_1000_words", "2"))
pattern = re.compile(r"\b(" + "|".join(re.escape(w) for w in words) + r")\b", re.IGNORECASE) if words else None

for line in sys.stdin:
    text = json.loads(line)["text"]
    if pattern is None:
        print("{}", flush=True)
        continue
    hits = len(pattern.findall(text))
    n_words = max(1, len(text.split()))
    rate = hits * 1000 / n_words
    if rate > limit:
        answer = {"keep": False, "reason": f"{hits} listed words ({rate:.1f} per 1,000)"}
    else:
        answer = {}
    print(json.dumps(answer), flush=True)
