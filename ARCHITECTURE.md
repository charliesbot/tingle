# Architecture

Internals reference for `tingle`. For the user-facing surface (install, flags, output format), see [`README.md`](README.md).

## Algorithm

1. Walk the repo; parse each source file with tree-sitter.
2. Extract top-level defs and imports per file.
3. Rank files as **entry points** (high imports-out) and **utilities** (high imports-in).
4. Fold module-to-module import edges into the `M` layer.
5. Emit the ranked sections + per-file records with line anchors.

Every run regenerates — no cache. Sub-second parse time across benched repos makes "just re-run it" correct.

Parsing uses the canonical C tree-sitter via the `tree-sitter` crate, with parallel parsing via `rayon`.

Full algorithm, data shapes, and the language-adapter recipe: [`docs/implementation.md`](docs/implementation.md).

## Metrics

### Performance (`make bench`)

| Repo                          | Files | Wall-clock | Peak RSS |
| ----------------------------- | ----- | ---------- | -------- |
| `charliesbot.dev` (React/TSX) | 32    | ~40 ms     | ~50 MB   |
| `advent-of-code` (multi-lang) | 91    | ~60 ms     | ~100 MB  |
| `one` (Kotlin Android)        | 166   | ~60 ms     | ~90 MB   |

Binary: 12 MB stripped.

### Eval (agent task quality)

The compact default layout preserves task quality at ≥0.97 mean score across three real repos, at 47-58% of the token cost vs `--full`. Harness in [`evals/README.md`](evals/README.md).

## Development

```bash
make build     # release build at rust/target/release/tingle
make test      # cargo test
make bench     # hyperfine + RSS + token counts
make install   # cargo install --path rust --force
```

## Further reading

- [`docs/design-doc.md`](docs/design-doc.md) — problem, prior art, approach, CLI surface, known gaps.
- [`docs/implementation.md`](docs/implementation.md) — algorithm + data shapes + adding-a-language recipe.
- [`docs/spike-results.md`](docs/spike-results.md) — performance + utility spike results.
- [`docs/deep-mode.md`](docs/deep-mode.md) — future `--deep` flag proposal.
- [`docs/bench-results.md`](docs/bench-results.md) — latest bench numbers.
- [`evals/README.md`](evals/README.md) — rate–distortion measurement harness.
