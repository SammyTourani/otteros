#!/usr/bin/env bash
# Fetches HuggingFaceTB/SmolLM2-135M-Instruct (Apache-2.0) from huggingface.co
# into $OTTEROS_MODEL_DIR (brief M7-T1; DECISIONS.md D21/D28: model weights are
# data, never committed, and live under OTTEROS_MODEL_DIR on the external SSD,
# pinned by SHA-256). Record the licence in THIRD_PARTY_NOTICES.md (D11's
# "assets are data" precedent), never in git.
#
# Every file's SHA-256 was captured the first time this script downloaded it
# and is pinned below; a mismatch on a later run means the upstream file
# changed (or something went wrong) and this script refuses to proceed.
#
# Usage: scripts/fetch-models.sh
set -euo pipefail

if [ -z "${OTTEROS_MODEL_DIR:-}" ]; then
  echo "fetch-models: OTTEROS_MODEL_DIR is not set; source scripts/env.sh first" >&2
  exit 1
fi

REPO="HuggingFaceTB/SmolLM2-135M-Instruct"
DEST="$OTTEROS_MODEL_DIR/$REPO"
BASE="https://huggingface.co/$REPO/resolve/main"
mkdir -p "$DEST"

sha256_of() {
  shasum -a 256 "$1" | awk '{print $1}'
}

# name -> sha256. model.safetensors's pin is the LFS object's own sha256 (the
# HF API's tree listing exposes it directly for LFS-tracked files, so this one
# was pinned without needing a throwaway download); the small JSON/text files
# aren't LFS-tracked, so their pins were captured by downloading once and
# hashing the result.
FILES="config.json tokenizer.json tokenizer_config.json special_tokens_map.json generation_config.json model.safetensors"
sha256_for() {
  case "$1" in
    config.json) echo "8eb740e8bbe4cff95ea7b4588d17a2432deb16e8075bc5828ff7ba9be94d982a" ;;
    tokenizer.json) echo "9ca9acddb6525a194ec8ac7a87f24fbba7232a9a15ffa1af0c1224fcd888e47c" ;;
    tokenizer_config.json) echo "4ec77d44f62efeb38d7e044a1db318f6a939438425312dfa333b8382dbad98df" ;;
    special_tokens_map.json) echo "2b7379f3ae813529281a5c602bc5a11c1d4e0a99107aaa597fe936c1e813ca52" ;;
    generation_config.json) echo "87b916edaaab66b3899b9d0dd0752727dff6666686da0504d89ae0a6e055a013" ;;
    model.safetensors) echo "5af571cbf074e6d21a03528d2330792e532ca608f24ac70a143f6b369968ab8c" ;;
    *) echo "fetch-models: no pin recorded for $1" >&2; exit 1 ;;
  esac
}

for f in $FILES; do
  out="$DEST/$f"
  expected="$(sha256_for "$f")"
  if [ -f "$out" ] && [ "$(sha256_of "$out")" = "$expected" ]; then
    echo "fetch-models: using cached $f"
    continue
  fi
  echo "fetch-models: downloading $f"
  curl -sL --fail --max-time 300 -o "$out" "$BASE/$f"
  actual="$(sha256_of "$out")"
  if [ "$actual" != "$expected" ]; then
    echo "fetch-models: SHA-256 mismatch for $f" >&2
    echo "  expected $expected" >&2
    echo "  actual   $actual" >&2
    rm -f "$out"
    exit 1
  fi
done

echo "fetch-models: OK -> $DEST"
