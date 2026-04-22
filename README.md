# tingle

A Rust CLI that produces an **AI-first codebase map** for agent orientation. Named after [Tingle](https://zelda.fandom.com/wiki/Tingle), the map seller in The Legend of Zelda.

AI agents exploring unfamiliar codebases walk them file-by-file through `ls` + `cat` loops, burning minutes of LLM token roundtrips per session. `tingle` replaces that walk with a single-turn digest the agent reads once — ranked entry points, load-bearing utilities, a dir-to-dir module graph, and per-file signatures with line-number anchors.

See [`docs/design-doc.md`](docs/design-doc.md) for design and [`docs/implementation.md`](docs/implementation.md) for the algorithm.

## Install

### Homebrew (macOS, Linux)

```bash
brew install charliesbot/tap/tingle
```

### From source

```bash
cargo install --path rust
```

Drops the binary at `~/.cargo/bin/tingle` — add that directory to your `PATH` if it isn't already (most `rustup` installs do this automatically).

For development:

```bash
make build     # release build at rust/target/release/tingle
make test      # cargo test
make bench     # hyperfine + RSS + token counts
make install   # cargo install --path rust --force
```

## Usage

```bash
tingle                                # write .tinglemap.md in the CWD
tingle /path/to/repo                  # map a specific repo → .tinglemap.md in it
tingle --stdout /path/to/repo         # print to stdout instead (for pipelines)
tingle --out PATH /path/to/repo       # write to a custom path
tingle --full /path/to/repo           # include per-file def signatures + 3 callers per Utility
tingle --scope core /path/to/repo     # F section only covers paths under `core/`
tingle --skeleton /path/to/repo       # omit F section; architecture layer only
tingle --alias '@:src' /path/to/repo  # apply an import alias (repeatable)
tingle --no-legend /path/to/repo      # skip the legend line
tingle --version
```

### Default: write `.tinglemap.md`

Running `tingle /path/to/repo` writes the map to `/path/to/repo/.tinglemap.md` and prints a one-line status on stdout:

```
wrote .tinglemap.md (36528 bytes, ~9.1k tokens)
```

Agents then read the file with their `Read` tool — bypassing Bash-tool preview caps that truncate inline output on medium+ repos.

**Gitignore it.** tingle prints a one-time hint if `.gitignore` doesn't already cover `.tinglemap.md`. It's a generated artifact (regenerated every run); committing it invites drift across PRs.

**Every run regenerates.** tingle has no cache and doesn't try to be clever about staleness. Sub-second parse time across the benched repos makes "just re-run it" correct. If you changed code and want the fresh map, run tingle again.

### Compact by default

F records show paths/imports/tags only. Utility records show 1 caller. Eval (`evals/`) on three real repos confirmed the compact layout preserves agent task quality (≥0.97 mean score) at 47-58% of the token cost vs `--full`.

Use `--full` when you want per-file signatures and 3 callers per U record — useful on small repos where size isn't the constraint.

### Output-size knobs (large repos)

- `--scope PATH` — filter F section to a subtree; architecture layer still whole-repo. Lossless within scope.
- `--skeleton` — drop F section entirely (architecture layer only). Lossy; use when you only need the module graph.

Flags compose: `tingle --scope app --skeleton ...`.

### Pipelines

```bash
tingle --stdout /path/to/repo | jq '...'       # old stdout behavior
tingle --stdout /path/to/repo > out.md         # custom destination via shell redirect
```

`--stdout` opts out of the file-writing default. The stdout output still carries the soft `# warning: ~Nk tokens — ...` line when the map would exceed typical agent preview caps.

## Output shape

```
# tingle 0.1.0  gen=2026-04-20  commit=c66bbef  files=166  tokenizer=cl100k_base
# legend: EP=entry(out=imports-out,in=imports-in) U=utility(in=fan-in) M=module-edge F=file  [M]=modified [test]=test-file  [hub]=both-entry-and-utility  [orphan]=no-import-callers(may-be-runtime-registered)

## Entry points
EP cmd/server/main.go:3 main (out=9 in=0)
EP wear/.../WearTodayViewModel.kt:27 WearTodayViewModel (out=8 in=2) [hub]

## Utilities
U src/utils/date.ts (in=23)  ← src/auth/login.ts (+22 more)

## Modules
M app -> app/core/components app/navigation core/notifications
M core/domain -> core/constants core/models

## Files
### src/auth
F login.ts   imp: ../store @okta/sdk
### src
F main.ts [M]  imp: ./auth/login ./store
F unused.ts [orphan]  imp: core/utils
```

See [`docs/design-doc.md § Output format`](docs/design-doc.md) for the full spec, and [`docs/implementation.md`](docs/implementation.md) for how each piece is computed.

## Performance

Measured on three real repos (`make bench`):

| Repo | Files | Wall-clock | Peak RSS |
| --- | --- | --- | --- |
| `charliesbot.dev` (React/TSX) | 32 | ~40 ms | ~50 MB |
| `advent-of-code` (multi-lang) | 91 | ~60 ms | ~100 MB |
| `one` (Kotlin Android) | 166 | ~60 ms | ~90 MB |

Binary: 12 MB stripped. Canonical C tree-sitter via the `tree-sitter` crate; parallel parsing via `rayon`.

Parsed languages: TypeScript, JavaScript (JSX, MJS), Python, Go, Kotlin (+ KTS), C++.

## Docs

- [`docs/design-doc.md`](docs/design-doc.md) — problem, prior art, approach, CLI surface, known gaps.
- [`docs/implementation.md`](docs/implementation.md) — algorithm + data shapes + adding-a-language recipe.
- [`docs/spike-results.md`](docs/spike-results.md) — performance + utility spike results.
- [`docs/deep-mode.md`](docs/deep-mode.md) — future `--deep` flag proposal (emit ranked-file bodies inline).
- [`docs/bench-results.md`](docs/bench-results.md) — latest bench numbers.
- [`evals/README.md`](evals/README.md) — rate–distortion measurement harness.
