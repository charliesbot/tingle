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
tingle --alias '@:src' /path/to/repo  # TypeScript/webpack import alias (repeatable)
tingle --version
```

One output shape, no toggles. Default writes `.tinglemap.md` to the target repo and prints a one-line status (`wrote .tinglemap.md (36528 bytes, ~9.1k tokens)`). Gitignore it — it's a generated artifact and every run regenerates. If the map is too large for your repo, run tingle on a subdirectory (`tingle features/feed`).

## Output format

```
# tingle 0.1.0  gen=2026-04-20  commit=c66bbef  files=166  tokenizer=cl100k_base
# legend: EP=entry(out=imports-out,in=imports-in) U=utility(in=fan-in) M=module-edge(src->dst=src-imports-dst) F=file(N=loc)  [M]=modified [test]=test-file  [hub]=both-entry-and-utility  [orphan]=no-import-callers(may-be-runtime-registered)  [path:line]=def f=func c=class

## Entry points
EP cmd/server/main.go:3 main (out=9 in=0 loc=48)
EP wear/.../WearTodayViewModel.kt:27 WearTodayViewModel (out=8 in=2 loc=126) [hub]

## Utilities
U src/utils/date.ts (in=23 loc=64)  ← src/auth/login.ts src/auth/logout.ts (+21 more)

## Modules
M app -> app/core/components app/navigation core/notifications
M core/domain -> core/constants core/models

## Files
### src/auth
F login.ts (82)  imp: ../store @okta/sdk
 12 c AuthService
 18 f login (user, pass)
### src
F main.ts (41) [M]  imp: ./auth/login ./store
F unused.ts (14) [orphan]  imp: core/utils
```

## Agent integration

Drop this into `CLAUDE.md` / `AGENTS.md`:

> Before exploring this repo, run `tingle` and read `.tinglemap.md`. Use it to find entry points and load-bearing utilities before opening individual files.

---

See [ARCHITECTURE.md](ARCHITECTURE.md) for algorithm, performance + eval metrics, and development workflow.
