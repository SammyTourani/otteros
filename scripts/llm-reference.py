#!/usr/bin/env python3
"""Golden reference data for OtterOS's inference runtime, produced by the
canonical Hugging Face `transformers` implementation (brief M7-T1, DECISIONS.md
D21: "correctness is proven by golden logits from a host reference, not by
reading generated text"). Every later runtime step (tokenizer, forward pass,
sampler) is judged against this independent oracle.

Usage (inside the venv from scripts/llm-venv.sh):
  /Volumes/SammyDisk/venvs/otteros/bin/python scripts/llm-reference.py \
      --model-dir <hf model dir> --prompts <prompts.json> --out <golden.json>

Writes a JSON file (schema documented in tools/otter-convert/FORMAT.md's
"golden fixture" section) with:
  - "tokenization": token ids for every prompt in `prompts.json`, tokenized
    with `add_special_tokens=False` (the prompts already spell out any
    special tokens they want, e.g. the ChatML ones), so this exercises
    exactly the tokenizer's BPE, nothing else.
  - "detailed": for a handful of prompts (--detailed-ids), the top-10 logits
    at every position of a single forward pass over the whole prompt, plus a
    32-token greedy continuation computed by a plain manual argmax loop
    (deliberately not `model.generate`, to keep this independent of any
    sampling/cache defaults that could change between transformers versions).

Always loads the model in float32 on CPU, regardless of the checkpoint's
stored dtype (SmolLM2 ships bf16 weights) -- DECISIONS.md D21's runtime uses
f32 activations, so the reference must match that, not the storage dtype.
"""
import argparse
import json
import sys

import torch
from transformers import AutoModelForCausalLM, AutoTokenizer

GREEDY_CONTINUATION_LEN = 32
TOP_K = 10


def load(model_dir: str):
    tokenizer = AutoTokenizer.from_pretrained(model_dir)
    model = AutoModelForCausalLM.from_pretrained(model_dir, dtype=torch.float32)
    model.eval()
    return tokenizer, model


def tokenize_all(tokenizer, prompts):
    out = []
    for p in prompts:
        ids = tokenizer.encode(p["text"], add_special_tokens=False)
        out.append({"id": p["id"], "token_ids": ids})
    return out


def top_k_logits(logits_row: torch.Tensor, k: int):
    values, indices = torch.topk(logits_row, k)
    result = []
    for v, i in zip(values.tolist(), indices.tolist()):
        logit_val = float(v)
        if not (logit_val == logit_val and abs(logit_val) != float('inf')):  # Check for NaN and inf
            raise ValueError(f"Non-finite logit value: {logit_val}")
        result.append({"id": int(i), "logit": round(logit_val, 6)})
    return result


@torch.no_grad()
def detailed_for_prompt(tokenizer, model, prompt, dump_continuation_logits=False):
    ids = tokenizer.encode(prompt["text"], add_special_tokens=False)
    input_ids = torch.tensor([ids], dtype=torch.long)
    out = model(input_ids=input_ids)
    logits = out.logits[0]  # [seq_len, vocab]
    per_position = [top_k_logits(logits[i], TOP_K) for i in range(logits.shape[0])]

    # Manual greedy continuation: no KV cache, no `generate()`, so the only
    # thing under test is the model's forward pass + argmax, matching what a
    # from-scratch runtime with no fancy decoding tricks yet would do first.
    continuation = []
    continuation_logits = [] if dump_continuation_logits else None
    continuation_input_ids = [] if dump_continuation_logits else None
    cur = list(ids)
    for _ in range(GREEDY_CONTINUATION_LEN):
        step_out = model(input_ids=torch.tensor([cur], dtype=torch.long))
        step_logits = step_out.logits[0, -1]  # [vocab]
        next_id = int(torch.argmax(step_logits).item())
        continuation.append(next_id)
        if dump_continuation_logits:
            continuation_logits.append(top_k_logits(step_logits, TOP_K))
            continuation_input_ids.append(list(cur))  # Record input ids at each step
        cur.append(next_id)

    result = {
        "id": prompt["id"],
        "token_ids": ids,
        "logits_top10_per_position": per_position,
        "greedy_continuation": continuation,
    }
    if dump_continuation_logits:
        result["continuation_logits_top10"] = continuation_logits
        result["continuation_input_ids"] = continuation_input_ids
    return result


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--model-dir", required=True)
    ap.add_argument("--prompts", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument(
        "--detailed-ids",
        default="p01_hello,p04_numbers_float,p11_chatml_math",
        help="comma-separated prompt ids to run full logits + greedy continuation for",
    )
    ap.add_argument(
        "--dump-continuation-logits",
        action="store_true",
        help="also dump top-10 logits at each continuation step",
    )
    args = ap.parse_args()

    with open(args.prompts, "r", encoding="utf-8") as f:
        prompts = json.load(f)

    tokenizer, model = load(args.model_dir)
    print(f"llm-reference: loaded {args.model_dir} ({model.config.architectures}), "
          f"vocab_size={model.config.vocab_size}", file=sys.stderr)

    tokenization = tokenize_all(tokenizer, prompts)

    detailed_ids = set(args.detailed_ids.split(","))
    by_id = {p["id"]: p for p in prompts}
    missing = detailed_ids - set(by_id)
    if missing:
        sys.exit(f"llm-reference: --detailed-ids not found in prompts file: {sorted(missing)}")

    detailed = [detailed_for_prompt(tokenizer, model, by_id[pid], args.dump_continuation_logits) for pid in args.detailed_ids.split(",")]

    result = {
        "model_dir": args.model_dir,
        "architecture": model.config.architectures[0],
        "vocab_size": model.config.vocab_size,
        "tokenization": tokenization,
        "detailed": detailed,
    }
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(result, f, indent=1)
        f.write("\n")
    print(f"llm-reference: wrote {args.out}", file=sys.stderr)


if __name__ == "__main__":
    main()
