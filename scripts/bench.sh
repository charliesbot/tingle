#!/usr/bin/env bash
# Benchmark Go vs Rust tingle on real repos. Writes docs/bench-results.md.
#
# Measures:
#   - wall-clock via hyperfine (3 warmup + 10 runs)
#   - peak RSS via /usr/bin/time -l (single run each)
#   - binary size (stripped release)

set -euo pipefail

GO_BIN="./tingle"
RUST_BIN="rust/target/release/tingle"
OUT="docs/bench-results.md"

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "hyperfine not found — brew install hyperfine" >&2
  exit 1
fi

# Portable RSS extractor for `/usr/bin/time -l` output (macOS: bytes).
extract_rss() {
  local line
  line=$(grep 'maximum resident set size' "$1" || true)
  # Field order on macOS: "<bytes>  maximum resident set size"
  awk '{print $1}' <<<"$line"
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

{
  echo "# Rust vs Go bench results"
  echo
  echo "Measured: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "Host: $(uname -msr)"
  echo

  echo "## Binary sizes (stripped release)"
  echo
  echo "| Binary | Size |"
  echo "| --- | --- |"
  go_size=$(stat -f '%z' "$GO_BIN" 2>/dev/null || stat -c '%s' "$GO_BIN")
  rust_size=$(stat -f '%z' "$RUST_BIN" 2>/dev/null || stat -c '%s' "$RUST_BIN")
  echo "| Go ($GO_BIN) | $((go_size / 1024 / 1024)) MB |"
  echo "| Rust ($RUST_BIN) | $((rust_size / 1024 / 1024)) MB |"
  echo

  echo "## Per-repo results"
  echo
  echo "| Repo | Go wall-clock | Rust wall-clock | Speedup | Go peak RSS | Rust peak RSS |"
  echo "| --- | --- | --- | --- | --- | --- |"

  for repo in "$@"; do
    name=$(basename "$repo")
    # hyperfine JSON for mean extraction
    hyperfine --warmup 3 --runs 10 --export-json "$tmp/$name.json" \
      "$GO_BIN $repo > /dev/null" \
      "$RUST_BIN $repo > /dev/null" >/dev/null 2>&1 || true

    go_mean=$(python3 -c "import json; d=json.load(open('$tmp/$name.json')); print(f\"{d['results'][0]['mean']:.3f}s\")")
    rust_mean=$(python3 -c "import json; d=json.load(open('$tmp/$name.json')); print(f\"{d['results'][1]['mean']:.3f}s\")")
    speedup=$(python3 -c "import json; d=json.load(open('$tmp/$name.json')); a=d['results'][0]['mean']; b=d['results'][1]['mean']; print(f\"{a/b:.2f}×\")")

    /usr/bin/time -l "$GO_BIN" "$repo" > /dev/null 2> "$tmp/$name.go.rss" || true
    /usr/bin/time -l "$RUST_BIN" "$repo" > /dev/null 2> "$tmp/$name.rust.rss" || true
    go_rss=$(extract_rss "$tmp/$name.go.rss")
    rust_rss=$(extract_rss "$tmp/$name.rust.rss")
    go_rss_mb=$((go_rss / 1024 / 1024))
    rust_rss_mb=$((rust_rss / 1024 / 1024))

    echo "| $name | $go_mean | $rust_mean | $speedup | ${go_rss_mb} MB | ${rust_rss_mb} MB |"
  done
} > "$OUT"

echo "Wrote $OUT"
cat "$OUT"
