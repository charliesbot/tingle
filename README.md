# tingle

A Rust CLI that produces an **AI-first codebase map** for agent orientation. Named after [Tingle](https://zelda.fandom.com/wiki/Tingle), the map seller in The Legend of Zelda.

AI agents exploring unfamiliar codebases walk them file-by-file through `ls` + `cat` loops, burning minutes of LLM token roundtrips per session. `tingle` replaces that walk with a single-turn digest the agent reads once — ranked entry points, load-bearing utilities, a dir-to-dir module graph, and per-file signatures with line-number anchors.

Supports TypeScript, JavaScript (JSX, MJS), Python, Go, Kotlin (+ KTS), C++.

## Install

```bash
brew install charliesbot/tap/tingle   # macOS, Linux
cargo install --path rust             # from source
```

## Usage

```bash
tingle                                # write .tinglemap.md in the CWD
tingle /path/to/repo                  # map a specific repo → .tinglemap.md in it
tingle --stdout /path/to/repo         # print to stdout instead (for pipelines)
tingle --out PATH /path/to/repo       # write to a custom path
tingle --full /path/to/repo           # per-file def signatures + 3 callers per Utility
tingle --scope core /path/to/repo     # F section only covers paths under `core/`
tingle --skeleton /path/to/repo       # omit F section; architecture layer only
tingle --alias '@:src' /path/to/repo  # apply an import alias (repeatable)
tingle --no-legend /path/to/repo      # skip the legend line
tingle --version
```

Default writes `.tinglemap.md` to the target repo and prints a one-line status (`wrote .tinglemap.md (36528 bytes, ~9.1k tokens)`). Gitignore it — it's a generated artifact and every run regenerates. Use `--full` for richer output on small repos; `--scope` / `--skeleton` to shrink the map on large ones. Flags compose.

## Output format

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

## Agent integration

Drop this into `CLAUDE.md` / `AGENTS.md`:

> Before exploring this repo, run `tingle` and read `.tinglemap.md`. Use it to find entry points and load-bearing utilities before opening individual files.

---

See [ARCHITECTURE.md](ARCHITECTURE.md) for algorithm, performance + eval metrics, and development workflow.
