# tingle

A stateless Go CLI that produces an **AI-first codebase map** for agent orientation. Named after [Tingle](https://zelda.fandom.com/wiki/Tingle), the map seller in The Legend of Zelda.

AI agents exploring unfamiliar codebases walk them file-by-file through `ls` + `cat` loops, burning ~3 minutes per session on LLM token roundtrips. `tingle` replaces that walk with a single-turn, tokenized-for-agents digest the agent reads once.

See [`docs/design-doc.md`](docs/design-doc.md) for design, [`docs/implementation.md`](docs/implementation.md) for the algorithm, and [`docs/spike-results.md`](docs/spike-results.md) for measurements.

**Status:** v0-dev. End-to-end pipeline ships. Parsed languages: TS/JS/JSX/MJS, Python, Go, Kotlin (+ KTS), C++.

## Usage

```bash
tingle /path/to/repo                  # print compact map to stdout
tingle --alias '@:src' /path/to/repo  # apply an import alias (repeatable)
tingle --no-legend /path/to/repo      # skip the legend line (for re-invocation)
tingle --version
```

Default output: a legend, ranked entry points, utilities with inline callers, a `dir → dir` module graph, and per-file signatures with line-number anchors. No state, no cache, stdout only.

## Build

```bash
go build -ldflags="-s -w" -o tingle ./cmd/tingle
```

Binary currently lands around 13 MB with all grammars linked (target: <30 MB).
