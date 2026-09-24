#!/bin/sh
# Stage finished pure crates for a commit while other crates in crates/ are still work in progress.
# Adds crates/<name> for each argument, plus a crates/Cargo.toml and crates/Cargo.lock that list only
# the crates committed at HEAD and these ones, after proving that exact tree compiles (tests included)
# in a clean temporary worktree on the build volume. The working copies of Cargo.toml/Cargo.lock are
# left untouched. Usage: scripts/stage-crates.sh <crate>... ; then git commit as usual.
set -eu
[ $# -ge 1 ] || { echo "usage: $0 <crate>..." >&2; exit 2; }
repo=$(cd "$(dirname "$0")/.." && pwd)
root=${OTTEROS_BUILD_ROOT:-$repo/build}
wt="$root/stage-wt.$$"
cleanup() { git -C "$repo" worktree remove --force "$wt" >/dev/null 2>&1 || rm -rf "$wt"; }
trap cleanup EXIT
git -C "$repo" worktree add --detach -q "$wt" HEAD
for c in "$@"; do
  [ -d "$repo/crates/$c" ] || { echo "no crate crates/$c" >&2; exit 2; }
  # Copy exactly what git would commit (tracked + untracked, ignored files excluded).
  (cd "$repo" && git ls-files --cached --others --exclude-standard "crates/$c") | while read -r f; do
    mkdir -p "$wt/$(dirname "$f")" && cp "$repo/$f" "$wt/$f"
  done
done
python3 - "$wt/crates/Cargo.toml" "$@" <<'EOF'
import re, sys
path, names = sys.argv[1], sys.argv[2:]
s = open(path).read()
m = re.search(r'members = \[(.*?)\]', s, re.S)
members = re.findall(r'"([^"]+)"', m.group(1))
for n in names:
    if n not in members:
        members.append(n)
s = s[:m.start()] + 'members = [' + ', '.join('"%s"' % x for x in members) + ']' + s[m.end():]
open(path, 'w').write(s)
EOF
(cd "$wt/crates" && cargo metadata -q --format-version 1 --offline >/dev/null)
for c in "$@"; do
  (cd "$wt/crates" && cargo build -q -p "$c" --tests) || { echo "stage: crates/$c does not build from a clean tree" >&2; exit 1; }
done
for f in Cargo.toml Cargo.lock; do
  sha=$(git -C "$repo" hash-object -w "$wt/crates/$f")
  git -C "$repo" update-index --add --cacheinfo "100644,$sha,crates/$f"
done
for c in "$@"; do git -C "$repo" add "crates/$c"; done
echo "staged: $* (+ crates/Cargo.toml, crates/Cargo.lock for HEAD members + these)"
