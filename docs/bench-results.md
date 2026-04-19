# Bench results

Measured: 2026-04-19T17:14:45Z
Host: Darwin 25.4.0 arm64
Binary: rust/target/release/tingle

## Binary size (stripped release)

12 MB

## Per-repo results (default invocation)

| Repo | Files | Wall-clock | Peak RSS | Output bytes | Output tokens |
| --- | --- | --- | --- | --- | --- |
| advent-of-code | 91 | 60 ms | 111 MB | 17.5 KB | 5.8k |
| charliesbot.dev | 32 | 41 ms | 51 MB | 5.9 KB | 1.9k |
| one | 166 | 58 ms | 91 MB | 116.7 KB | 29.4k |

## Output-shrink flags on the largest repo

Demonstrates `--scope` and `--skeleton` on the largest repo — the
knobs agents reach for when the default output is too big to fit in one
tool-result turn.

Largest repo: `one`

| Invocation | Output bytes | Output tokens |
| --- | --- | --- |
| default | 116.7 KB | 29.4k |
| `--skeleton` | 46.6 KB | 11.9k |
| `--scope app` | 53.7 KB | 13.6k |
| `--scope complications` | 48.2 KB | 12.2k |
