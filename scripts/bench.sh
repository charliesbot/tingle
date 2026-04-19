#!/usr/bin/env bash
# Measure tingle on a set of real repos. Writes docs/bench-results.md.
#
#   scripts/bench.sh <tingle-binary> <repo>...
#
# Measures:
#   - wall-clock via hyperfine (3 warmup + 10 runs)
#   - peak RSS via /usr/bin/time -l (single run each)
#   - binary size (stripped release)

set -euo pipefail

BIN="$1"
shift

OUT="docs/bench-results.md"

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "hyperfine not found — brew install hyperfine" >&2
  exit 1
fi

# Portable RSS extractor for `/usr/bin/time -l` output.
extract_rss() {
  local line
  line=$(grep 'maximum resident set size' "$1" || true)
  awk '{print $1}' <<<"$line"
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

{
  echo "# Bench results"
  echo
  echo "Measured: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "Host: $(uname -msr)"
  echo "Binary: $BIN"
  echo

  size=$(stat -f '%z' "$BIN" 2>/dev/null || stat -c '%s' "$BIN")
  echo "## Binary size (stripped release)"
  echo
  echo "$(($size / 1024 / 1024)) MB"
  echo

  echo "## Per-repo results"
  echo
  echo "| Repo | Files | Wall-clock | Peak RSS |"
  echo "| --- | --- | --- | --- |"

  for repo in "$@"; do
    name=$(basename "$repo")

    hyperfine --warmup 3 --runs 10 --export-json "$tmp/$name.json" \
      "$BIN $repo > /dev/null" >/dev/null 2>&1 || true
    mean=$(python3 -c "import json; d=json.load(open('$tmp/$name.json')); print(f\"{d['results'][0]['mean']*1000:.0f} ms\")")

    /usr/bin/time -l "$BIN" "$repo" > "$tmp/$name.out" 2> "$tmp/$name.rss" || true
    rss=$(extract_rss "$tmp/$name.rss")
    rss_mb=$((rss / 1024 / 1024))

    # Count files from the header line: "files=N"
    files=$(head -1 "$tmp/$name.out" | grep -oE 'files=[0-9]+' | head -1 | cut -d= -f2)

    echo "| $name | $files | $mean | ${rss_mb} MB |"
  done
} > "$OUT"

echo "Wrote $OUT"
cat "$OUT"
