# evals/ — distortion measurement for tingle output

Tingle is graded on **rate–distortion**, not just bytes. Compression that
shrinks tokens but degrades agent task quality is a loss. This harness
measures both axes.

## Usage

```bash
# A/B compare format variants on a repo:
bash evals/compare.sh /path/to/repo evals/questions/<repo>.yaml

# Single variant:
bash evals/run.sh /path/to/repo evals/questions/<repo>.yaml [tingle flags...]
```

Both scripts shell out to `claude --print` (Claude Code CLI) using
`claude-sonnet-4-6` by default. Override with `EVAL_MODEL=...`.

## How scoring works

Each question in `questions/<repo>.yaml` declares:

- `q`: the prompt the agent answers (using ONLY the tingle map as context)
- `expected_substrings`: list of strings the answer should mention
- `min_score`: pass threshold (fraction of expected strings present)
- `forbidden_substrings` (optional): strings that deduct from score

Score = `(hits / len(expected)) - 0.5 * (forbidden_hits)`, clamped to
[0, 1]. Pass = score >= min_score.

Substring matching is dumb but reproducible. An LLM judge would give
better signal but adds non-determinism.

## First baseline (one repo, 166 Kotlin files)

Historical numbers from when three output shapes existed:

| variant | tokens | pass/n | mean_score |
|---|---|---|---|
| default (full) | 20,927 | 10/10 | 1.00 |
| compact (now default) | 9,208 | 10/10 | 0.97 |
| skeleton (removed) | 4,567 | 8/10 | 0.90 |

The compact layout — now the only default — saved 56% tokens for 3%
quality loss, which is what justified making it the baseline. The
skeleton variant saved 78% for 10% loss, but the failing questions
(`test_layout`, `settings_storage`) showed exactly what was being
dropped: the F section's per-file detail. Rather than ship a flag with
known quality holes, we collapsed to a single shape and removed
skeleton entirely — use `--scope PATH` to zoom in instead.

## Adding questions for a new repo

1. Pick 8-12 questions covering different agent skills: architecture
   awareness, hub identification, entry-point detection, data flow,
   test layout, dependency framework.
2. For each, pick `expected_substrings` that a correct answer would
   mention — favor unique repo-specific terms (class names, file paths)
   over generic words.
3. Ground-truth substrings should be lowercase-comparable
   (matching is case-insensitive).

## Why this exists

The competing TASC compression proposal (issues/1) frames the work as
rate–distortion but never specifies how distortion is measured. Without
this harness, every compression decision is faith-based. With it,
compression changes either earn their tokens or get reverted.
