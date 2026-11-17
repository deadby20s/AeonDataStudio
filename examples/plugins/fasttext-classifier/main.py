"""AeonDataStudio plugin: filter documents with a fastText classifier.

Works with quality classifiers such as the DCLM baseline classifier or your own model trained
with `fasttext supervised`. Install: pip install fasttext-wheel numpy
"""
import json
import os
import sys

import fasttext

opts = json.loads(os.environ.get("AEON_PLUGIN_OPTIONS", "{}"))
model = fasttext.load_model(opts.get("model", "model.bin"))
label = opts.get("label", "__label__hq")
min_p = float(opts.get("min_probability", "0.5"))

for line in sys.stdin:
    text = json.loads(line)["text"].replace("\n", " ")
    labels, probs = model.predict(text, k=-1)
    p = dict(zip(labels, probs)).get(label, 0.0)
    if p < min_p:
        print(json.dumps({"keep": False, "reason": f"{label} {p:.2f} < {min_p}"}), flush=True)
    else:
        print("{}", flush=True)
