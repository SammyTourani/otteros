#!/usr/bin/env bash
# Creates the Python environment for the Otter reference tooling (brief
# M7-T1) at /Volumes/SammyDisk/venvs/otteros -- DECISIONS.md D28: downloads
# and the venv itself live on the external SSD, never the internal disk.
# CPU-only torch (no CUDA/MPS wheels -- this is a correctness oracle run
# once per fixture, not a training job) plus `transformers` and the small
# libraries it needs to load safetensors checkpoints and fast tokenizers.
#
# Safe to re-run: `python -m venv` is idempotent and `pip install` is a
# no-op once the pinned-by-`pip` latest versions are already present.
#
# Usage: scripts/llm-venv.sh
set -euo pipefail

VENV=/Volumes/SammyDisk/venvs/otteros
PY=python3.13

if [ ! -d /Volumes/SammyDisk ]; then
  echo "llm-venv: /Volumes/SammyDisk is not mounted; refusing to create a venv on the internal disk" >&2
  exit 1
fi

if ! command -v "$PY" >/dev/null 2>&1; then
  echo "llm-venv: $PY not found (torch wheels lag the newest CPython; install via 'brew install python@3.13')" >&2
  exit 1
fi

if [ ! -x "$VENV/bin/python" ]; then
  echo "llm-venv: creating venv at $VENV"
  "$PY" -m venv "$VENV"
fi

echo "llm-venv: installing/upgrading pip"
"$VENV/bin/pip" install --upgrade pip --quiet

echo "llm-venv: installing CPU torch + transformers (pip cache is already on the SSD, see ~/.claude/CLAUDE.md)"
"$VENV/bin/pip" install --quiet \
  --index-url https://download.pytorch.org/whl/cpu torch \
  --extra-index-url https://pypi.org/simple transformers safetensors tokenizers numpy

"$VENV/bin/python" -c "import torch, transformers; print(f'llm-venv: torch {torch.__version__}, transformers {transformers.__version__}')"
