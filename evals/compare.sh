#!/usr/bin/env bash
# evals/compare.sh — A/B agent task quality across format variants.
#
# Usage:
#   evals/compare.sh <repo-path> <questions.yaml>
#
# Runs evals/run.sh once per format variant, gathers JSON, prints a
# comparison table to stdout and writes raw results to evals/results/.

set -euo pipefail

REPO="$1"
QUESTIONS="$2"
RESULTS_DIR="evals/results/$(date +%s)"
mkdir -p "$RESULTS_DIR"

variants=(
  "default::"
  "full:--full:"
  "skeleton:--skeleton:"
)

# Header
printf "\n%-12s  %-8s  %-8s  %-12s\n" "variant" "tokens" "pass/n" "mean_score"
printf "%-12s  %-8s  %-8s  %-12s\n" "------------" "--------" "--------" "------------"

for spec in "${variants[@]}"; do
  IFS=':' read -r name flags _ <<<"$spec"
  flags_arr=()
  if [ -n "$flags" ]; then
    # shellcheck disable=SC2206
    flags_arr=($flags)
  fi
  out_json="$RESULTS_DIR/$name.json"
  out_log="$RESULTS_DIR/$name.log"
  bash evals/run.sh "$REPO" "$QUESTIONS" "${flags_arr[@]+${flags_arr[@]}}" \
    > "$out_log" 2> "$out_json" || true
  tokens=$(grep -oE '~[0-9]+ tok' "$out_log" | head -1 | tr -d '~ tok' || echo "?")
  pass=$(python3 -c "import json; d=json.load(open('$out_json')); print(f\"{d['passed']}/{d['n']}\")" 2>/dev/null || echo "?")
  score=$(python3 -c "import json; d=json.load(open('$out_json')); print(f\"{d['mean_score']:.2f}\")" 2>/dev/null || echo "?")
  printf "%-12s  %-8s  %-8s  %-12s\n" "$name" "$tokens" "$pass" "$score"
done

echo
echo "raw logs in $RESULTS_DIR/"
