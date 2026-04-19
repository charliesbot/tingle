#!/usr/bin/env bash
# Measure tingle on a set of real repos. Writes docs/bench-results.md.
#
#   scripts/bench.sh <tingle-binary> <repo>...
#
# Per-repo metrics:
#   - Wall-clock via hyperfine (3 warmup + 10 runs)
#   - Peak RSS via /usr/bin/time -l (one run)
#   - Output bytes (KB) — direct file size
#   - Output tokens (cl100k_base via tiktoken, or char/4 fallback)
#   - Files covered (from the tingle header)

set -euo pipefail

BIN="$1"
shift

OUT="docs/bench-results.md"

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "hyperfine not found — brew install hyperfine" >&2
  exit 1
fi

extract_rss() {
  local line
  line=$(grep 'maximum resident set size' "$1" || true)
  awk '{print $1}' <<<"$line"
}

count_tokens() {
  # cl100k_base if tiktoken installed; else heuristic (chars/4).
  python3 - "$1" <<'PY'
import sys
path = sys.argv[1]
text = open(path, 'r', errors='replace').read()
try:
    import tiktoken
    enc = tiktoken.get_encoding('cl100k_base')
    print(len(enc.encode(text)))
except Exception:
    # Fallback: approx 4 chars per cl100k_base token.
    print(max(1, len(text) // 4))
PY
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
  echo "$((size / 1024 / 1024)) MB"
  echo

  echo "## Per-repo results (default invocation)"
  echo
  echo "| Repo | Files | Wall-clock | Peak RSS | Output bytes | Output tokens |"
  echo "| --- | --- | --- | --- | --- | --- |"

  for repo in "$@"; do
    name=$(basename "$repo")

    hyperfine --warmup 3 --runs 10 --export-json "$tmp/$name.json" \
      "$BIN $repo > /dev/null" >/dev/null 2>&1 || true
    mean=$(python3 -c "import json; d=json.load(open('$tmp/$name.json')); print(f\"{d['results'][0]['mean']*1000:.0f} ms\")")

    /usr/bin/time -l "$BIN" "$repo" > "$tmp/$name.out" 2> "$tmp/$name.rss" || true
    rss=$(extract_rss "$tmp/$name.rss")
    rss_mb=$((rss / 1024 / 1024))

    bytes=$(stat -f '%z' "$tmp/$name.out" 2>/dev/null || stat -c '%s' "$tmp/$name.out")
    bytes_kb=$(awk -v b="$bytes" 'BEGIN{printf "%.1f", b/1024}')

    tokens=$(count_tokens "$tmp/$name.out")
    tokens_k=$(awk -v t="$tokens" 'BEGIN{printf "%.1f", t/1000}')

    files=$(head -1 "$tmp/$name.out" | grep -oE 'files=[0-9]+' | head -1 | cut -d= -f2)

    echo "| $name | $files | $mean | ${rss_mb} MB | ${bytes_kb} KB | ${tokens_k}k |"
  done

  echo
  echo "## Output-shrink flags on the largest repo"
  echo
  echo "Demonstrates \`--scope\` and \`--skeleton\` on the largest repo — the"
  echo "knobs agents reach for when the default output is too big to fit in one"
  echo "tool-result turn."
  echo
  largest=""
  largest_bytes=0
  for repo in "$@"; do
    name=$(basename "$repo")
    b=$(stat -f '%z' "$tmp/$name.out" 2>/dev/null || stat -c '%s' "$tmp/$name.out")
    if [ "$b" -gt "$largest_bytes" ]; then
      largest="$repo"
      largest_bytes="$b"
    fi
  done
  if [ -n "$largest" ]; then
    echo "Largest repo: \`$(basename "$largest")\`"
    echo
    echo "| Invocation | Output bytes | Output tokens |"
    echo "| --- | --- | --- |"
    # Top-level directories with source code. Skip dotfiles, build output,
    # and dirs with no parsable content.
    skip_re='^(build|node_modules|dist|target|out|coverage|gradle|docs)$'
    scopes=$(
      find "$largest" -mindepth 1 -maxdepth 1 -type d -not -name '.*' 2>/dev/null \
        | while read -r d; do
            name=$(basename "$d")
            if echo "$name" | grep -Eq "$skip_re"; then
              continue
            fi
            if find "$d" -type f \( -name '*.ts' -o -name '*.tsx' -o -name '*.js' \
                -o -name '*.py' -o -name '*.go' -o -name '*.kt' -o -name '*.kts' \
                -o -name '*.cc' -o -name '*.cpp' -o -name '*.h' \) -print -quit \
                | grep -q .; then
              echo "$name"
            fi
          done \
        | sort | head -2
    )
    row() {
      local label="$1"; shift
      "$BIN" "$@" > "$tmp/variant.out" 2>/dev/null
      local b=$(stat -f '%z' "$tmp/variant.out" 2>/dev/null || stat -c '%s' "$tmp/variant.out")
      local t=$(count_tokens "$tmp/variant.out")
      local bkb=$(awk -v b="$b" 'BEGIN{printf "%.1f", b/1024}')
      local tk=$(awk -v t="$t" 'BEGIN{printf "%.1f", t/1000}')
      echo "| $label | ${bkb} KB | ${tk}k |"
    }
    row "default" "$largest"
    row "\`--compact\`" --compact "$largest"
    row "\`--skeleton\`" --skeleton "$largest"
    while IFS= read -r s; do
      [ -z "$s" ] && continue
      row "\`--scope $s\`" --scope "$s" "$largest"
      row "\`--scope $s --compact\`" --scope "$s" --compact "$largest"
    done <<<"$scopes"
  fi
} > "$OUT"

echo "Wrote $OUT"
cat "$OUT"
