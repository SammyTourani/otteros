#!/usr/bin/env python3
"""Acceptance oracle for brief M7-T5 (Otter runs inside OtterOS), written by the orchestrator.

/bin/otter-run, running inside OtterOS, knows only the prompt texts (prompts.json); it prints what
it computed as `[otter-json] {...}` lines on serial. This script compares those numbers with the
host reference (Hugging Face transformers, tools/otter-convert/tests/fixtures/<model>/golden.json):

  {"kind": "tokenize", "id": ..., "ids": [...]}                       every prompt, exact ids
  {"kind": "logits", "id": ..., "position": p, "top10": [[id, logit], ...]}
                                                                      every position of the three
                                                                      detailed prompts
  {"kind": "greedy", "id": ..., "tokens": [...]}                      the greedy continuation

Logits must be within --tolerance of the reference for every reference top-10 id that also
appears in OtterOS's top-10; a reference id may be missing only if its logit is within the
tolerance of the reference's tenth logit (a tie at the boundary). Greedy continuations must match
for the first --greedy tokens. Exit status 0 when everything matches, 1 otherwise.

Usage: llm_check.py SERIAL_LOG GOLDEN_JSON [--tolerance 1e-3] [--greedy 32]
"""
import argparse
import json
import re
import sys


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("serial")
    ap.add_argument("golden")
    ap.add_argument("--tolerance", type=float, default=1e-3)
    ap.add_argument("--greedy", type=int, default=32)
    args = ap.parse_args()

    log = open(args.serial, errors="replace").read()
    golden = json.load(open(args.golden))
    records = []
    for m in re.finditer(r"\[otter-json\] (\{.*\})", log):
        try:
            records.append(json.loads(m.group(1)))
        except json.JSONDecodeError as exc:
            print(f"[llm_check] unparseable line: {m.group(1)[:120]!r} ({exc})")
            return 1

    errors = []
    tokenized = {r["id"]: r["ids"] for r in records if r.get("kind") == "tokenize"}
    for g in golden["tokenization"]:
        if tokenized.get(g["id"]) != g["token_ids"]:
            errors.append(f"tokenize {g['id']}: got {tokenized.get(g['id'])}, want {g['token_ids']}")

    worst = 0.0
    for d in golden["detailed"]:
        rows = {r["position"]: r["top10"] for r in records if r.get("kind") == "logits" and r.get("id") == d["id"]}
        for pos, gold in enumerate(d["logits_top10_per_position"]):
            got = rows.get(pos)
            if got is None:
                errors.append(f"{d['id']} position {pos}: no logits printed")
                continue
            got_map = {int(i): float(v) for i, v in got}
            tenth = gold[-1]["logit"]
            for e in gold:
                if e["id"] in got_map:
                    diff = abs(got_map[e["id"]] - e["logit"])
                    worst = max(worst, diff)
                    if diff > args.tolerance:
                        errors.append(f"{d['id']} position {pos} id {e['id']}: logit {got_map[e['id']]:.6f}, want {e['logit']:.6f}")
                elif e["logit"] - tenth > args.tolerance:
                    errors.append(f"{d['id']} position {pos}: id {e['id']} (logit {e['logit']:.6f}) missing from the top 10")
        greedy = next((r["tokens"] for r in records if r.get("kind") == "greedy" and r.get("id") == d["id"]), None)
        want = d["greedy_continuation"][: args.greedy]
        if greedy is None or greedy[: args.greedy] != want:
            errors.append(f"{d['id']} greedy: got {greedy[: args.greedy] if greedy else None}, want {want}")

    for e in errors[:20]:
        print(f"[llm_check] {e}")
    detailed = len(golden["detailed"])
    print(f"[llm_check] tokenization {len(golden['tokenization'])} prompts, logits and greedy for {detailed} prompts; "
          f"max |dlogit| {worst:.2e}; {len(errors)} mismatches")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
