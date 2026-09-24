#!/usr/bin/env python3
"""
Fetch html5lib-tests repository and WHATWG entities.json into third_party/
"""

import os
import json
import subprocess
import hashlib
from pathlib import Path
import urllib.request

# Repository details
HTML5LIB_REPO = "https://github.com/html5lib/html5lib-tests.git"

# Paths
THIRD_PARTY = Path(__file__).parent.parent / "third_party"
HTML5LIB_DIR = THIRD_PARTY / "html5lib-tests"
ENTITIES_FILE = THIRD_PARTY / "entities.json"

def ensure_third_party():
    """Ensure third_party directory exists."""
    THIRD_PARTY.mkdir(exist_ok=True)
    print(f"[*] third_party directory: {THIRD_PARTY}")

def fetch_html5lib():
    """Clone or update html5lib-tests repository."""
    if HTML5LIB_DIR.exists():
        print(f"[*] html5lib-tests already cloned at {HTML5LIB_DIR}")
        return

    print(f"[*] Cloning html5lib-tests from {HTML5LIB_REPO}")
    subprocess.run(
        ["git", "clone", HTML5LIB_REPO, str(HTML5LIB_DIR)],
        check=True
    )
    print(f"[+] html5lib-tests fetched")

def fetch_entities():
    """Download entities.json from WHATWG."""
    if ENTITIES_FILE.exists():
        print(f"[*] entities.json already exists at {ENTITIES_FILE}")
        return

    entities_url = "https://html.spec.whatwg.org/entities.json"
    print(f"[*] Downloading entities.json from {entities_url}")
    urllib.request.urlretrieve(entities_url, str(ENTITIES_FILE))

    # Validate it's valid JSON
    with open(ENTITIES_FILE, 'r', encoding='utf-8') as f:
        data = json.load(f)

    print(f"[+] entities.json fetched ({len(data)} entities)")

def main():
    ensure_third_party()
    fetch_html5lib()
    fetch_entities()

    # List available test files
    test_dir = HTML5LIB_DIR / "tokenizer"
    if test_dir.exists():
        print(f"\n[*] Available tokenizer tests:")
        for f in sorted(test_dir.glob("*.test")):
            print(f"    - {f.name}")

    tree_dir = HTML5LIB_DIR / "tree-construction"
    if tree_dir.exists():
        print(f"\n[*] Available tree-construction tests:")
        for f in sorted(tree_dir.glob("*.dat")):
            print(f"    - {f.name}")

if __name__ == "__main__":
    main()
