# Bench results

Measured: 2026-04-19T21:47:56Z
Host: Darwin 25.4.0 arm64
Binary: rust/target/release/tingle

## Binary size (stripped release)

12 MB

## Per-repo results (default invocation)

| Repo | Files | Wall-clock | Peak RSS | Output bytes | Output tokens |
| --- | --- | --- | --- | --- | --- |
| advent-of-code | 91 | 61 ms | 105 MB | 15.8 KB | 5.3k |
| charliesbot.dev | 32 | 42 ms | 51 MB | 5.1 KB | 1.7k |
| one | 166 | 66 ms | 91 MB | 98.2 KB | 24.6k |

## Output-shrink flags on the largest repo

Demonstrates `--scope` and `--skeleton` on the largest repo — the
knobs agents reach for when the default output is too big to fit in one
tool-result turn.

Largest repo: `one`

| Invocation | Output bytes | Output tokens |
| --- | --- | --- |
| default | 98.2 KB | 24.6k |
| `--compact` | 49.4 KB | 12.0k |
| `--skeleton` | 32.9 KB | 8.3k |
| `--scope app` | 39.6 KB | 9.9k |
| `--scope app --compact` | 28.9 KB | 7.3k |
| `--scope complications` | 34.5 KB | 8.6k |
| `--scope complications --compact` | 25.8 KB | 6.6k |
