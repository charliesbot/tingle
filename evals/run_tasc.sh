#!/usr/bin/env bash
# Same as evals/run.sh but pipes tingle output through tasc_dict.py
# before scoring. Tests whether dictionary substitution preserves
# agent task quality.
#
# Usage: evals/run_tasc.sh <repo> <questions> [tingle flags...]

set -euo pipefail

REPO="$1"
QUESTIONS="$2"
shift 2
TINGLE_FLAGS=("$@")
TINGLE_FLAGS+=()

TINGLE_BIN="${TINGLE_BIN:-rust/target/release/tingle}"
CLAUDE_BIN="${CLAUDE_BIN:-claude}"
MODEL="${EVAL_MODEL:-claude-sonnet-4-6}"
DICT_TOP="${TASC_TOP:-20}"
DICT_THRESH="${TASC_THRESH:-4}"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Generate + dict-substitute.
"$TINGLE_BIN" "${TINGLE_FLAGS[@]+${TINGLE_FLAGS[@]}}" "$REPO" 2>/dev/null \
  | python3 evals/tasc_dict.py --top "$DICT_TOP" --threshold "$DICT_THRESH" \
  > "$tmp/map.txt"

map_bytes=$(wc -c < "$tmp/map.txt")
map_tokens=$(python3 -c "
text = open('$tmp/map.txt').read()
try:
    import tiktoken
    print(len(tiktoken.get_encoding('cl100k_base').encode(text)))
except Exception:
    print(max(1, len(text) // 4))
")

flags_str="${TINGLE_FLAGS[*]+${TINGLE_FLAGS[*]}}"
echo "=== eval+TASC: $(basename "$REPO") flags=[$flags_str] tasc_top=$DICT_TOP map=${map_bytes}B / ~${map_tokens} tok ==="
echo

python3 - "$QUESTIONS" "$tmp/map.txt" "$CLAUDE_BIN" "$MODEL" <<'PY'
import json, subprocess, sys, yaml
q_path, map_path, claude_bin, model = sys.argv[1:5]
questions = yaml.safe_load(open(q_path))
map_text = open(map_path).read()

system_prompt = (
    "You are answering questions about a codebase. "
    "Use ONLY the tingle codebase map provided below as context. "
    "The map MAY include a `# dict:` line near the top defining short "
    "aliases like `$0=...`, `$1=...`. When you see `$N` in the body, "
    "expand it via the dict before reasoning. Do NOT speculate. "
    "Be concise (2-4 sentences). Quote file paths verbatim from the map.\n\n"
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
    results.append(dict(id=qid, score=score, pass_thresh=pass_thresh, passed=passed,
                        misses=misses, forbidden_hits=misses_for))
    flag = "✓" if passed else "✗"
    miss_str = f"  miss: {misses}" if misses else ""
    print(f"  {flag} {qid:25s} score={score:.2f} (need {pass_thresh:.2f}){miss_str}")

agg_score = sum(r["score"] for r in results) / len(results)
agg_pass = sum(1 for r in results if r["passed"])
print()
print(f"  aggregate: {agg_pass}/{len(results)} passed, mean score {agg_score:.2f}")
print(json.dumps(dict(n=len(results), passed=agg_pass, mean_score=agg_score, results=results)),
      file=sys.stderr)
PY
