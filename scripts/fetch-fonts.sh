#!/usr/bin/env bash
# Fetches Inter and JetBrains Mono (brief M4-T2) from their official GitHub
# release pages, both OFL-1.1, and installs the four static, TrueType-outline
# (glyf, not CFF; not the variable-font builds) files otter-gfx's tests and
# specimen need into third_party/fonts/:
#   third_party/fonts/inter/Inter-Regular.ttf, Inter-SemiBold.ttf
#   third_party/fonts/jetbrains-mono/JetBrainsMono-Regular.ttf, -Bold.ttf
#
# Every download (the release zip) and every extracted font file is verified
# against a SHA-256 pinned below before use. DECISIONS.md D11: fonts are data,
# not code -- licensed separately (recorded in THIRD_PARTY_NOTICES.md) and,
# like third_party/limine, fetched here rather than committed to the repo
# (third_party/ is gitignored).
#
# Usage: scripts/fetch-fonts.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/third_party/fonts"
CACHE="$ROOT/build/font-downloads"
mkdir -p "$DEST/inter" "$DEST/jetbrains-mono" "$CACHE"

INTER_ZIP_URL="https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip"
INTER_ZIP_SHA256="9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e"
JBM_ZIP_URL="https://github.com/JetBrains/JetBrainsMono/releases/download/v2.304/JetBrainsMono-2.304.zip"
JBM_ZIP_SHA256="6f6376c6ed2960ea8a963cd7387ec9d76e3f629125bc33d1fdcd7eb7012f7bbf"

INTER_REGULAR_SHA256="40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82"
INTER_SEMIBOLD_SHA256="78a843fade9d4612a5567302fb595b56976eb5fcebf4fea5a5912d638bafcde3"
JBM_REGULAR_SHA256="a0bf60ef0f83c5ed4d7a75d45838548b1f6873372dfac88f71804491898d138f"
JBM_BOLD_SHA256="5590990c82e097397517f275f430af4546e1c45cff408bde4255dad142479dcb"

sha256_of() {
  shasum -a 256 "$1" | awk '{print $1}'
}

verify() {
  local file="$1" expected="$2" actual
  actual="$(sha256_of "$file")"
  if [ "$actual" != "$expected" ]; then
    echo "fetch-fonts: SHA-256 mismatch for $file" >&2
    echo "  expected $expected" >&2
    echo "  actual   $actual" >&2
    exit 1
  fi
}

download() {
  local url="$1" out="$2" sha="$3"
  if [ -f "$out" ] && [ "$(sha256_of "$out")" = "$sha" ]; then
    echo "fetch-fonts: using cached $(basename "$out")"
    return
  fi
  echo "fetch-fonts: downloading $(basename "$out")"
  curl -sL --fail --max-time 120 -o "$out" "$url"
  verify "$out" "$sha"
}

download "$INTER_ZIP_URL" "$CACHE/Inter-4.1.zip" "$INTER_ZIP_SHA256"
download "$JBM_ZIP_URL" "$CACHE/JetBrainsMono-2.304.zip" "$JBM_ZIP_SHA256"

echo "fetch-fonts: extracting Inter Regular/SemiBold"
unzip -o -q -j "$CACHE/Inter-4.1.zip" \
  "extras/ttf/Inter-Regular.ttf" "extras/ttf/Inter-SemiBold.ttf" "LICENSE.txt" \
  -d "$DEST/inter"
mv -f "$DEST/inter/LICENSE.txt" "$DEST/inter/LICENSE-OFL.txt"

echo "fetch-fonts: extracting JetBrains Mono Regular/Bold"
unzip -o -q -j "$CACHE/JetBrainsMono-2.304.zip" \
  "fonts/ttf/JetBrainsMono-Regular.ttf" "fonts/ttf/JetBrainsMono-Bold.ttf" "OFL.txt" \
  -d "$DEST/jetbrains-mono"
mv -f "$DEST/jetbrains-mono/OFL.txt" "$DEST/jetbrains-mono/LICENSE-OFL.txt"

verify "$DEST/inter/Inter-Regular.ttf" "$INTER_REGULAR_SHA256"
verify "$DEST/inter/Inter-SemiBold.ttf" "$INTER_SEMIBOLD_SHA256"
verify "$DEST/jetbrains-mono/JetBrainsMono-Regular.ttf" "$JBM_REGULAR_SHA256"
verify "$DEST/jetbrains-mono/JetBrainsMono-Bold.ttf" "$JBM_BOLD_SHA256"

echo "fetch-fonts: OK -> $DEST"
