# tingle

A stateless Rust CLI that produces an **AI-first codebase map** for agent orientation. Named after [Tingle](https://zelda.fandom.com/wiki/Tingle), the map seller in The Legend of Zelda.

AI agents exploring unfamiliar codebases walk them file-by-file through `ls` + `cat` loops, burning minutes of LLM token roundtrips per session. `tingle` replaces that walk with a single-turn digest the agent reads once — ranked entry points, load-bearing utilities, a dir-to-dir module graph, and per-file signatures with line-number anchors.

See [`docs/design-doc.md`](docs/design-doc.md) for design and [`docs/spike-results.md`](docs/spike-results.md) for measurements.

## Install

```bash
cargo install --path rust
```

This builds a release binary and drops it at `~/.cargo/bin/tingle` — add that directory to your `PATH` if it isn't already (most `rustup` installs do this automatically). After that, `tingle` is a global command.

For development builds:

```bash
make build     # release build at rust/target/release/tingle
make test      # cargo test
make bench     # hyperfine + RSS on the configured REPOS
```

## Usage

```bash
tingle                                # print compact map to stdout (default: cwd)
tingle /path/to/repo                  # map a specific repo
tingle --full /path/to/repo           # include per-file def signatures + 3 callers per Utility
tingle --scope core /path/to/repo     # F section only covers paths under `core/`
tingle --skeleton /path/to/repo       # omit F section; architecture layer only
tingle --alias '@:src' /path/to/repo  # apply an import alias (repeatable)
tingle --no-legend /path/to/repo      # skip the legend line
tingle --version
```

**Default is the compact layout**: F records list paths/imports/tags only and Utility records show 1 caller each. Eval (`evals/`) on three real repos showed this preserves agent task quality (≥0.97 mean score) at 47-58% of the token cost vs `--full`.

Output-size knobs, smallest savings first:

- `--scope PATH` — filter F section to a subtree; architecture layer still whole-repo.
- `--skeleton` — drop F section entirely (architecture layer only).

Flags compose (`--scope app --skeleton`). The header emits `# warning: ~Nk tokens — consider ...` when output exceeds ~20k tokens.

Use `--full` to recover the previous default (per-file signatures, 3 callers per U record) — useful when you want signatures for navigation, e.g. on a small repo where size isn't the constraint.

### When the output exceeds your agent's preview limit

Agent CLIs cap inline tool-result previews at varying sizes (Claude Code, Cursor, etc. all differ). When tingle's output is bigger than your environment's preview, redirect to a file and read it as a normal artifact:

```bash
tingle /path/to/repo > /tmp/map.md
# then in your agent: Read('/tmp/map.md')
```

This isn't tingle-specific — it's the standard answer for any tool whose output you want to consume in pieces. The soft `# warning: ~Nk tokens — consider ...` line in the header is your cue that you'll likely want to do this (or reach for `--skeleton` / `--scope`).

Parsed languages: TypeScript, JavaScript (JSX, MJS), Python, Go, Kotlin (+ KTS), C++. No state, no cache, stdout only. Re-run whenever the repo changes — it's faster than cache invalidation.

## Output shape

```
# tingle 0.1.0  gen=2026-04-19  commit=c66bbef  files=273  tokenizer=cl100k_base
# legend: S=manifest EP=entry U=utility M=module-edge F=file  [M]=modified [untracked]=new-unstaged [test]=test-file  [path:line]=def  f=func c=class m=method i=interface t=type e=enum

## Manifests
S go.mod        module=github.com/user/repo  go=1.22

## Entry points
EP cmd/server/main.go:3 main (out=9 in=0)

## Utilities
U src/utils/date.ts (in=23)  ← src/auth/login.ts src/components/Form.tsx (+20 more)
 4 f formatDate (d: Date) -> string

## Modules
M src/app -> src/auth src/store src/ui

## Files
F src/main.ts  imp: ./auth/login ./store
 12 f bootstrap () -> Promise<void>
```

See [`docs/design-doc.md § Output format`](docs/design-doc.md) for the full spec.

## Performance

Measured on three real repos (see [`docs/bench-results.md`](docs/bench-results.md)):

| Repo | Files | Wall-clock | Peak RSS |
| --- | --- | --- | --- |
| `charliesbot.dev` (React/TSX + MDX) | 61 | ~40 ms | ~50 MB |
| `advent-of-code` (multi-lang) | 172 | ~60 ms | ~100 MB |
| `one` (Kotlin Android) | 273 | ~60 ms | ~90 MB |

Binary: 12 MB stripped. Uses canonical C tree-sitter via the `tree-sitter` crate, parallel parsing via `rayon`.

## Docs

- [`docs/design-doc.md`](docs/design-doc.md) — problem, prior art (aider, repomix), approach, CLI surface, known gaps.
- [`docs/spike-results.md`](docs/spike-results.md) — performance + utility spike results.
- [`docs/deep-mode.md`](docs/deep-mode.md) — future `--deep` flag proposal (emit ranked-file bodies inline).
- [`docs/bench-results.md`](docs/bench-results.md) — latest bench numbers.
