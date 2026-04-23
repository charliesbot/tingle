# Bench results

Measured: 2026-04-20T03:14:27Z
Host: Darwin 25.4.0 arm64
Binary: rust/target/release/tingle

## Binary size (stripped release)

12 MB

## Per-repo results (default invocation)

| Repo | Files | Wall-clock | Peak RSS | Output bytes | Output tokens |
| --- | --- | --- | --- | --- | --- |
| advent-of-code | 91 | 62 ms | 96 MB | 7.9 KB | 3.0k |
| charliesbot.dev | 32 | 42 ms | 51 MB | 3.4 KB | 1.1k |
| one | 166 | 74 ms | 85 MB | 35.4 KB | 8.6k |

## Output-shrink flags on the largest repo

Demonstrates `--scope` on the largest repo — the knob agents reach for
when the default output is too big to fit in one tool-result turn.

Largest repo: `one`

| Invocation | Output bytes | Output tokens |
| --- | --- | --- |
| default | 35.4 KB | 8.6k |
| `--scope app` | 16.0 KB | 4.1k |
| `--scope complications` | 13.3 KB | 3.4k |
