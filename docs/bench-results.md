# Bench results

Measured: 2026-04-19T21:53:32Z
Host: Darwin 25.4.0 arm64
Binary: rust/target/release/tingle

## Binary size (stripped release)

12 MB

## Per-repo results (default invocation)

| Repo | Files | Wall-clock | Peak RSS | Output bytes | Output tokens |
| --- | --- | --- | --- | --- | --- |
| advent-of-code | 91 | 60 ms | 105 MB | 15.8 KB | 5.3k |
| charliesbot.dev | 32 | 41 ms | 54 MB | 5.1 KB | 1.7k |
| one | 166 | 58 ms | 91 MB | 83.3 KB | 20.9k |

## Output-shrink flags on the largest repo

Demonstrates `--scope` and `--skeleton` on the largest repo — the
knobs agents reach for when the default output is too big to fit in one
tool-result turn.

Largest repo: `one`

| Invocation | Output bytes | Output tokens |
| --- | --- | --- |
| default | 83.3 KB | 20.9k |
| `--compact` | 37.9 KB | 9.2k |
| `--skeleton` | 18.1 KB | 4.6k |
| `--scope app` | 24.7 KB | 6.2k |
| `--scope app --compact` | 17.5 KB | 4.5k |
| `--scope complications` | 19.6 KB | 4.9k |
| `--scope complications --compact` | 14.4 KB | 3.8k |
