"""AeonDataStudio plugin: FineWeb-Edu educational-quality classifier.

Install: pip install transformers torch
The model is downloaded from Hugging Face on first use.
"""
import json
import os
import sys

import torch
from transformers import AutoModelForSequenceClassification, AutoTokenizer

opts = json.loads(os.environ.get("AEON_PLUGIN_OPTIONS", "{}"))
name = opts.get("model", "HuggingFaceFW/fineweb-edu-classifier")
min_score = float(opts.get("min_score", "3"))
device = "cuda" if torch.cuda.is_available() else "cpu"
tokenizer = AutoTokenizer.from_pretrained(name)
model = AutoModelForSequenceClassification.from_pretrained(name).to(device).eval()

for line in sys.stdin:
    text = json.loads(line)["text"]
    inputs = tokenizer(text, return_tensors="pt", truncation=True, max_length=512).to(device)
    with torch.no_grad():
        score = model(**inputs).logits.squeeze(-1).float().item()
    if score < min_score:
        print(json.dumps({"keep": False, "reason": f"edu score {score:.2f} < {min_score}"}), flush=True)
    else:
        print("{}", flush=True)
