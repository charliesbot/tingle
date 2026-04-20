# Bench results

Measured: 2026-04-20T02:46:28Z
Host: Darwin 25.4.0 arm64
Binary: rust/target/release/tingle

## Binary size (stripped release)

12 MB

## Per-repo results (default invocation)

| Repo | Files | Wall-clock | Peak RSS | Output bytes | Output tokens |
| --- | --- | --- | --- | --- | --- |
| advent-of-code | 91 | 61 ms | 111 MB | 7.5 KB | 2.8k |
| charliesbot.dev | 32 | 41 ms | 51 MB | 3.3 KB | 1.0k |
| one | 166 | 61 ms | 91 MB | 37.2 KB | 8.9k |

## Output-shrink flags on the largest repo

Demonstrates `--scope` and `--skeleton` on the largest repo — the
knobs agents reach for when the default output is too big to fit in one
tool-result turn.

Largest repo: `one`

| Invocation | Output bytes | Output tokens |
| --- | --- | --- |
| default | 37.2 KB | 8.9k |
| `--compact` | 37.2 KB | 8.9k |
| `--skeleton` | 13.2 KB | 3.4k |
| `--scope app` | 16.8 KB | 4.2k |
| `--scope app --compact` | 16.8 KB | 4.2k |
| `--scope complications` | 13.7 KB | 3.5k |
| `--scope complications --compact` | 13.7 KB | 3.5k |
