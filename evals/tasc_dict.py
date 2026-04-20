#!/usr/bin/env python3
"""
TASC Layer 2 (dictionary substitution) post-processor.

Reads tingle output from stdin, identifies the top-N most-repeated tokens
in F-record import lists + U-record caller lists, emits a dictionary
header, and substitutes references in the body. Writes result to stdout.

Usage:
  tingle <repo> | python3 evals/tasc_dict.py [--top N] [--threshold T]

  --top N         Hard cap on dictionary size (default 20).
  --threshold T   Only alias strings appearing >= T times (default 4).

Exists to test the L2 hypothesis from issues/1: does dictionary
substitution save tokens AT THE COST OF agent quality? Pair with
evals/run.sh to measure both.

Aliases use the form `$N` (dollar + number). Single-token under
cl100k_base, locally adjacent to the dictionary header for resolution.
"""
import re
import sys
from collections import Counter

def parse_args(argv):
    top = 20
    thresh = 4
    args = iter(argv[1:])
    for a in args:
        if a == "--top":
            top = int(next(args))
        elif a == "--threshold":
            thresh = int(next(args))
        else:
            raise SystemExit(f"unknown arg: {a}")
    return top, thresh

def main():
    top, thresh = parse_args(sys.argv)
    text = sys.stdin.read()
    lines = text.splitlines()

    # Collect candidates from F-record imports + U-record callers.
    # We only alias strings that look like paths or compact-form refs —
    # not bare keywords. A safe filter: at least one of `/`, `:`, `.`.
    candidates = Counter()
    in_files = False
    in_utilities = False
    for line in lines:
        if line.startswith("## "):
            section = line[3:].strip()
            in_files = section == "Files"
            in_utilities = section == "Utilities"
        elif in_files and line.startswith("F ") and "  imp: " in line:
            _, imps = line.rsplit("  imp: ", 1)
            for tok in imps.split():
                if any(c in tok for c in "/:."):
                    candidates[tok] += 1
        elif in_utilities and line.startswith("U ") and " ← " in line:
            _, after = line.split(" ← ", 1)
            after = re.sub(r"\s*\(\+\d+ more\)\s*$", "", after)
            for tok in after.split():
                if any(c in tok for c in "/:."):
                    candidates[tok] += 1

    # Rank by *byte-savings potential* = (count - 1) * len(tok).
    # Subtract 1 because the dictionary header itself emits the token once.
    # Also subtract alias-reference cost (~3 chars: `$N`).
    def savings(tok, n):
        ref_cost = 2 + len(str(n))  # roughly $0..$99
        return (n - 1) * (len(tok) - ref_cost) - len(tok)

    ranked = sorted(
        ((s, n) for s, n in candidates.items() if n >= thresh),
        key=lambda kv: -savings(kv[0], kv[1]),
    )[:top]

    # Build alias table.
    alias = {}
    for i, (s, _n) in enumerate(ranked):
        alias[s] = f"${i}"

    # Emit transformed output.
    out_lines = []

    # Find header insertion point — after legend, before first ## section.
    inserted = False
    in_files = False
    in_utilities = False
    for line in lines:
        if not inserted and line.startswith("## "):
            # Insert dict header just before this section.
            if alias:
                out_lines.append(f"# dict: " + " ".join(f"{v}={k}" for k, v in alias.items()))
            inserted = True

        if line.startswith("## "):
            section = line[3:].strip()
            in_files = section == "Files"
            in_utilities = section == "Utilities"
            out_lines.append(line)
        elif in_files and line.startswith("F ") and "  imp: " in line:
            head, imps = line.rsplit("  imp: ", 1)
            new_imps = " ".join(alias.get(t, t) for t in imps.split())
            out_lines.append(f"{head}  imp: {new_imps}")
        elif in_utilities and line.startswith("U ") and " ← " in line:
            prefix, after = line.split(" ← ", 1)
            tail_match = re.search(r"\s*(\(\+\d+ more\))\s*$", after)
            tail = tail_match.group(1) if tail_match else ""
            after_core = re.sub(r"\s*\(\+\d+ more\)\s*$", "", after)
            new_callers = " ".join(alias.get(t, t) for t in after_core.split())
            new_line = f"{prefix} ← {new_callers}"
            if tail:
                new_line += f" {tail}"
            out_lines.append(new_line)
        else:
            out_lines.append(line)

    print("\n".join(out_lines))

if __name__ == "__main__":
    main()
