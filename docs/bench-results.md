# Rust vs Go bench results

Measured: 2026-04-19T05:25:57Z
Host: Darwin 25.4.0 arm64

## Binary sizes (stripped release)

| Binary | Size |
| --- | --- |
| Go (./tingle) | 24 MB |
| Rust (rust/target/release/tingle) | 12 MB |

## Per-repo results

| Repo | Go wall-clock | Rust wall-clock | Speedup | Go peak RSS | Rust peak RSS |
| --- | --- | --- | --- | --- | --- |
| advent-of-code | 0.893s | 0.061s | 14.74× | 876 MB | 99 MB |
| charliesbot.dev | 0.124s | 0.041s | 3.00× | 73 MB | 51 MB |
| one | 0.503s | 0.060s | 8.38× | 145 MB | 90 MB |
