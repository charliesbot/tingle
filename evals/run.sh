#!/usr/bin/env bash
# evals/run.sh — score one (repo, question-set, tingle-flags) tuple.
#
# Usage:
#   evals/run.sh <repo-path> <questions.yaml> [tingle flags...]
#
# Pipes `tingle <flags> <repo>` output as context to `claude --print` for
# each question, scores the answer by substring-matching against
# `expected_substrings`. Emits a per-question table and an aggregate.
#
# Requires: `claude` CLI on PATH, python3 with pyyaml, jq.

set -euo pipefail

REPO="$1"
QUESTIONS="$2"
shift 2
TINGLE_FLAGS=("$@")
# Workaround `set -u`: ensure indexing the array is safe even when empty.
TINGLE_FLAGS+=()

TINGLE_BIN="${TINGLE_BIN:-rust/target/release/tingle}"
CLAUDE_BIN="${CLAUDE_BIN:-claude}"
MODEL="${EVAL_MODEL:-claude-sonnet-4-6}"

if [ ! -x "$TINGLE_BIN" ]; then
  echo "tingle binary not found at $TINGLE_BIN — run \`make build\` first" >&2
  exit 1
fi
if ! command -v "$CLAUDE_BIN" >/dev/null 2>&1; then
  echo "claude CLI not on PATH" >&2
  exit 1
fi
if ! python3 -c "import yaml" 2>/dev/null; then
  echo "python3 yaml module missing — pip3 install pyyaml" >&2
  exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Generate the tingle output once.
"$TINGLE_BIN" "${TINGLE_FLAGS[@]+"${TINGLE_FLAGS[@]}"}" "$REPO" > "$tmp/map.txt" 2>/dev/null
map_bytes=$(wc -c < "$tmp/map.txt")
map_tokens=$(python3 -c "
import sys
text = open('$tmp/map.txt').read()
try:
    import tiktoken
    enc = tiktoken.get_encoding('cl100k_base')
    print(len(enc.encode(text)))
except Exception:
    print(max(1, len(text) // 4))
")

flags_str="${TINGLE_FLAGS[*]+${TINGLE_FLAGS[*]}}"
echo "=== eval: $(basename "$REPO") flags=[$flags_str] map=${map_bytes}B / ~${map_tokens} tok ==="
echo

# Iterate questions.
python3 - "$QUESTIONS" "$tmp/map.txt" "$CLAUDE_BIN" "$MODEL" "$tmp" <<'PY'
import json, os, subprocess, sys, yaml

q_path, map_path, claude_bin, model, tmp = sys.argv[1:6]
questions = yaml.safe_load(open(q_path))
map_text = open(map_path).read()

system_prompt = (
    "You are answering questions about a codebase. "
    "Use ONLY the tingle codebase map provided below as context — do NOT "
    "speculate, do NOT use prior knowledge of any specific project. "
    "Be concise (2-4 sentences). Quote file paths verbatim from the map "
    "when naming files.\n\n"
    "=== tingle map ===\n"
    + map_text
    + "\n=== end map ===\n"
)

results = []
for q in questions:
    qid = q["id"]
    prompt = system_prompt + "\n\nQuestion: " + q["q"]
    try:
        r = subprocess.run(
            [claude_bin, "--model", model, "--print", prompt],
            capture_output=True, text=True, timeout=120,
        )
        answer = r.stdout.strip()
    except subprocess.TimeoutExpired:
        answer = "[TIMEOUT]"
    # Cast to str: YAML can parse `2024` as int, breaking `.lower()`.
    expected = [str(s) for s in q.get("expected_substrings", [])]
    forbidden = [str(s) for s in q.get("forbidden_substrings", [])]
    al = answer.lower()
    hits = sum(1 for s in expected if s.lower() in al)
    misses = [s for s in expected if s.lower() not in al]
    misses_for = [s for s in forbidden if s.lower() in al]
    score = hits / max(1, len(expected))
    if misses_for:
        score = max(0.0, score - 0.5 * len(misses_for))
    pass_thresh = q.get("min_score", 1.0)
    passed = score >= pass_thresh
    results.append(dict(
        id=qid, score=score, pass_thresh=pass_thresh, passed=passed,
        misses=misses, forbidden_hits=misses_for,
    ))
    flag = "✓" if passed else "✗"
    miss_str = f"  miss: {misses}" if misses else ""
    print(f"  {flag} {qid:20s} score={score:.2f} (need {pass_thresh:.2f}){miss_str}")

agg_score = sum(r["score"] for r in results) / len(results)
agg_pass = sum(1 for r in results if r["passed"])
print()
print(f"  aggregate: {agg_pass}/{len(results)} passed, mean score {agg_score:.2f}")
print()

# Emit JSON for downstream comparison.
out_json = {
    "n": len(results),
    "passed": agg_pass,
    "mean_score": agg_score,
    "results": results,
}
print(json.dumps(out_json), file=sys.stderr)
PY
