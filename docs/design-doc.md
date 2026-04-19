# tingle — Efficient Architecture Mapper for AI Agents

**Status:** Final — approved by architect + reviewer
**Author:** charliesbot
**Date:** 2026-04-17

> Named after [Tingle](https://zelda.fandom.com/wiki/Tingle), the map seller in The Legend of Zelda.

## Problem

AI agents exploring unfamiliar codebases walk them file-by-file through `ls` + `cat` loops. Observed: a Claude subagent doing this on a small side project takes ~3 minutes. The bottleneck is LLM token roundtrips, not disk I/O — LLMs are slow readers, filesystems are not.

`tingle` is a language-agnostic CLI that produces a fast, AI-first codebase digest. Any agent (subagent, orchestrator, top-level) invokes it once at session start and reads the output instead of walking the repo itself.

Goals:

1. Generate an AI-first codebase digest in **<2s** on a typical small/mid project (<2k files).
2. Produce stable, navigable output the agent can skim _once_ instead of re-reading the repo.
3. Stateless — no cache, no persistent artifacts. Each run is a full rebuild from source.
4. Fast enough that "just run it again" is the correct answer to staleness.

Non-goals:

- Large monorepos (v2+).
- Replacing real code reading for implementation work. This is orientation, not comprehension.
- A generic "repo packer" that dumps everything. repomix already does that well (and has `--compress` for signatures-only). Our differentiation is _ranked, navigable_ orientation — not bulk packing.

## Prior art

### aider's repomap

Aider's `aider/repomap.py` (~1k lines Python) is the reference implementation. It walks files, parses with tree-sitter, extracts defs/refs per language, builds a symbol graph, runs personalized PageRank, and emits a single text blob under a token budget. Tags cached in SQLite keyed by mtime.

**Keep:**

- **Tree-sitter** for language-agnostic parsing. Regex and ctags both produce too much noise.
- **Symbol graph** (def→ref edges, file-to-symbol membership).
- **Signatures-only output.** No function bodies. The point is context efficiency.
- **One flat artifact** the agent reads once.

**Drop / simplify:**

- **Personalized PageRank seeded by "files in chat."** Aider recomputes per turn because files get added/removed in the chat loop. We don't have that loop — one static rank per run.
- **PageRank itself, initially.** On small graphs it amplifies the same signal as in-degree. And in-degree alone produces garbage for entry points (see §Ranking). The fix isn't "better ranking algorithm," it's "better heuristic." Both PageRank and in-degree are out for v1.
- **Token-budget binary search.** Overkill. Cap top-N symbols per file (default 10) and top-N entry points (default 20). Agent can open a file for details if it wants more.
- **SQLite cache.** A single JSON file is simpler, diffable, debuggable with `jq`. Size at our scale (<5MB for 2k files) makes binary formats pointless.
- **mtime-based invalidation.** Content hashing is nearly as fast and doesn't lie when editors touch files or `git checkout` resets them.
- **One giant string output.** Structured Markdown with stable headers gives the agent anchors to cite back.

### repomix

TypeScript CLI (23k+ stars, mature). Packs an entire repo into one XML/Markdown/text file for LLM consumption. Git-aware. `--compress` uses tree-sitter to strip bodies and keep signatures. Ships on npm / brew / web / browser extension. Token counting, secretlint-based secret detection, remote repo support, stdin file-list input.

`repomix --compress` is ~80% of what we're building. The gap is where our value lives:

**What repomix does well (steal, don't rebuild):**

- `--stdin` file list input (`git ls-files '*.ts' | tingle --stdin`). Lets the user pick files via their own tools instead of us inventing filters.
- Token count in output header so the agent knows what it's consuming.
- Secret scanning before emitting output.
- `.repomixignore` convention → we use `.tingleignore`.

**What repomix doesn't do (our differentiation):**

- **No ranking.** Every file renders equal. Agent still has to figure out what matters.
- **No module graph.** Signatures only — no import/dep edges.
- **Not navigable.** One blob, no lazy expansion.
- **Alphabetical ordering** of files, not by importance.

**Hard question:** is the ranking + graph worth building, or is `repomix --compress` genuinely good enough? Spike B answers this.

## Pre-work: de-risking spikes (half-day)

Two checks before we write v1. Spikes produce numbers, not kept code. After spikes the design is locked: either v1 proceeds as specified, or the design is revised and the doc updated.

**Spike A — tree-sitter-in-Go is fast enough.**
Parse ~2k TS files end-to-end on a laptop. Measure four things: cold parse time, binary size (with `-ldflags="-s -w"`), cgo call overhead per parse, peak RSS. Targets: **<2s** total parse, **<30MB** binary, **<200MB** peak RSS, cgo overhead not dominant. If any target misses by >2×, the plan changes before v1 starts.

**Spike B — a ranked, compact digest actually beats existing tools.**
Three contestants on the same real repo + same agent task:

0. `tingle` regex-only prototype — no tree-sitter, just `git ls-files` + language-aware regex for top-level defs and imports (~200 lines of Go). This is the "dumb baseline." **Must correctly handle multi-line imports, commented-out exports, template literals with code-like content, and string-embedded `export`/`import` tokens.** A regex that passes only clean code doesn't count as winning — that's self-deception, not a result.
1. `repomix --compress` (tree-sitter signatures, flat, alphabetical). The real competitor — a mature tool that does ~80% of what we propose.
2. Thin `tingle` prototype that emits the compact tag-prefixed format from real tree-sitter output (not a hand-written mock — we're validating what the tool will actually produce).

Measure two dimensions, not one:

- **Task quality:** did the agent complete the task correctly?
- **Tokens consumed:** how many tokens did the digest take?

Side test (same spike, cheap to run): **ASCII vs Unicode symbol prefixes.** Run contestant (2) in two variants — `f`/`c`/`m` vs `ƒ`/`©`/`ι`. Measure on Claude's tokenizer (`cl100k_base` as a proxy — results can differ on `o200k` and others). BPE tokenizers often split rare Unicode into multiple byte-level tokens, so Unicode may be **more** expensive despite looking denser. Pick the default by measurement, not aesthetics.

Both matter. Cheap output with equal quality is a win. Higher quality but 3× tokens is a loss (eats headroom for actual work). If (0) wins or ties on both dimensions, tree-sitter is over-engineering — ship the regex version and skip most of this plan. If (1) wins, pivot to "tingle = repomix wrapper + ranking overlay." Only if (2) wins on both dimensions does the full tree-sitter plan hold.

**Decision gate:** A passes and B shows (2) wins on both dimensions → proceed with full plan. If (0) wins, ship regex version as v1 and tree-sitter becomes v2. Otherwise, plan changes.

**Don't rig the spike.** If contestant (0) produces comparable task quality at lower token cost, we ship it — even if it means discarding most of this design. Sunk engineering cost is not justification to ignore spike results.

## Approach

A single Go binary, stateless, writes Markdown to stdout:

1. Enumerates files via `git ls-files -com --exclude-standard` (cached, others, modified — minus gitignored). **No-git fallback:** if `.git` is missing, falls back to `filepath.WalkDir` with default ignores (`node_modules`, `dist`, `build`, `.venv`, `venv`, `target`, `.next`, `out`, `coverage`). Applies `.tingleignore` on top either way.
2. Parses each file in parallel with tree-sitter → `{defs, refs, imports}` per file.
3. **Import resolution (heuristic).** Path math for relative imports + try common extensions (`.ts`/`.js`/`.py`/`.go`) and index/package files. Optional `--alias PREFIX:PATH` flag applies user-supplied prefix substitutions (`@`→`src/`) before path math. External imports (`@okta/sdk`, `django.db`) and unmapped aliases still render raw. Full config-parsing (tsconfig, go.mod, etc.) is post-MVP.
4. Builds symbol graph → ranks entry points (heuristic, see §Ranking).
5. Renders Markdown to stdout.

Agent invocation (any subagent, orchestrator, or top-level agent):

```
tingle                  # prints Markdown to stdout; agent captures into context
tingle > map.md         # or redirect to a file if the agent wants one
```

No cache. No `.tingle/` directory. No on-disk state. If the agent's context gets compacted, it re-runs the CLI — which at sub-second parse time is cheaper than cache invalidation bugs.

### Architecture

```
          ┌────────────────────────────────────────────────┐
          │                    tingle                     │
          │                                                │
 git ─▶   │  1. enumerate (git ls-files -com)              │
          │         │                                      │
 fs ─▶    │         ▼                                      │
          │  2. parse all files in parallel (tree-sitter)  │
          │         │                                      │
          │         ▼                                      │
          │  3. resolve imports (heuristic path math)      │
          │         │                                      │
          │         ▼                                      │
          │  4. rank entry points (heuristic)              │
          │         │                                      │
          │         ▼                                      │
 stdout ◀─│  5. render Markdown                            │
          └────────────────────────────────────────────────┘
```

### Tech picks

| Concern         | Choice                                              | Why                                                                |
| --------------- | --------------------------------------------------- | ------------------------------------------------------------------ |
| Language        | Go                                                  | Fast cold startup (agents call once per session); easy parallelism |
| Tree-sitter     | `tree-sitter/go-tree-sitter` (official)             | Maintained; grammars as separate cgo modules                       |
| Git enumeration | Shell out to `git ls-files -com --exclude-standard` | Faster than go-git; handles ignore rules for free                  |
| Parallelism     | `errgroup` + GOMAXPROCS worker pool                 | Parsing is CPU-bound                                               |

Note on Go vs TS: repomix proves TS distribution works fine (npm + brew + web). We pick Go for cold startup — an agent invoking the tool once per session pays node/npx startup on every run, and Go is ~instant.

### Ranking (replaces in-degree / PageRank)

In-degree surfaces utilities (`log`, `types`, `utils/date`), not entry points. PageRank amplifies the same signal. Real entry points come from multiple signals:

- **Filename conventions:** `main.go`, `index.ts`, `manage.py`, `App.kt`, files containing `if __name__ == "__main__"`, `func main()`, `bootstrap(` at top level.
- **Manifest-declared entries:** `package.json` `bin`/`main`/`exports` targets, `go.mod` `cmd/*` roots.
- **Shebangs:** any file starting with `#!` — executable, always an entry.
- **Low in-degree, high out-degree** — the code calls many things but is called by nothing.
- **Public exports from package roots** (`src/index.ts`, `cmd/*/main.go`).

Ranking is an equal-weighted blend of these signals + (out − in) degree. Adjust weights only if real use shows bias. Framework route detection (`app.get`, `@app.route`, etc.) is post-MVP.

**Caveat on `(out − in)` quality.** The degree signal is measured only on resolved imports. On repos with heavy aliased imports (TS `@/` paths, Go internal modules), a file's out-degree is systematically undercounted — alias imports render raw but don't contribute to the graph. `--alias PREFIX:PATH` reduces this for repos with known alias maps; full config-aware resolution (post-MVP) closes the rest. If Spike B shows ranking adds no value, check whether it's an algorithm issue or a coverage artifact before reaching for PageRank/HITS.

Utilities get their own section (`## Core utilities`) ranked _by_ in-degree — they're useful to surface, just not as entry points.

### Output format (stdout)

Compact, tag-prefixed, single-line records. Minimal Markdown — just `##` section headers for citability ("see Entry points section"). No backticks, no redundant keywords. Every def carries a line-number anchor so the agent can do `Read(path, line=N)` without a search step.

```
# tingle v1  gen=2026-04-17  commit=abc1234  files=142  tokens~4.2k  tokenizer=cl100k_base
# legend: S=manifest EP=entry U=utility M=module-edge F=file  [M]=modified [untracked]=new-unstaged [test]=test-file  [path:line]=def  f=func c=class m=method

## Manifests
S package.json  scripts: build=tsc test=jest dev=nodemon lint=eslint
S package.json  bin: mytool->dist/cli.js  main: dist/index.js
S go.mod        module=github.com/user/repo  go=1.22

## Entry points
EP src/main.ts:12 bootstrap (out=14 in=0)
EP cmd/server/main.go:3 main (out=9 in=0)

## Utilities
U src/utils/date.ts:4 formatDate (in=23)
U src/log/index.ts:2 logger (in=19)

## Modules
M src/app -> src/auth src/store src/ui
M src/auth -> src/store

## Files
F src/main.ts [M] imp: ./auth/login ./store ./ui/root
 12 f bootstrap() -> Promise<void>
 18 c App
  25 m start() -> void
  42 m stop() -> void

F src/auth/login.test.ts [test] imp: ../login ./fixtures
 3 f makeMockUser() -> User

F src/auth/login.ts  imp: ../store @okta/sdk @/config/env
 5  c AuthService
  8  m login(user, pass) -> Promise<Session>
  20 m logout() -> void

F src/components/Button.tsx  imp: @/utils/classnames react
 5 f Button(props) -> JSX.Element
```

Design notes:

- Legend at line 2 teaches the format once; prefix-cached across invocations in the same session.
- No bodies. If the agent needs one, it opens the source file at the anchored line.
- **Activity tags.** Modified tracked files tagged `[M]`; new-untracked files tagged `[untracked]`; test files (path-matched `.test.`, `.spec.`, `__tests__/`, `_test.go`) tagged `[test]` so the agent can deprioritize them when looking for production code. Git-log-based `[hot]` tag → post-MVP.
- **Manifest surface (`S` records).** Top-of-output summary of `package.json` (scripts, bin, main) and `go.mod` (module path). Architect feedback: "the scripts block alone saves me a Read every time." `pyproject.toml`/`Cargo.toml` → post-MVP. If no manifest is detected (pure Python repo with no `pyproject.toml`, scratch folder, etc.), the `## Manifests` section is omitted entirely rather than rendered empty.
- **Unresolved imports stay raw.** Two different failure modes render the same way on `imp:` lines:
  - **External packages** (`@okta/sdk`, `react`, `django.db`) — never resolve to repo-internal files; they stay raw forever. Correct behavior.
  - **Aliased imports** (`@/config/env`, `@/utils/classnames` via tsconfig `paths`) — _could_ resolve if we knew the mapping. MVP offers `--alias PREFIX:PATH` (e.g. `--alias '@:src'`) as a manual override when the caller knows the aliases. Full config-parsing post-MVP.

  Without `--alias`, both failure modes contribute to missing `M` module graph edges on alias-heavy repos. Agent can still grep the raw path if needed.

- **`--no-legend`.** Legend line (`# legend: ...`) is ~80 tokens. Same-session re-invocations can pass `--no-legend` to skip emitting it. Micro-optimization; useful when an agent runs `tingle` repeatedly and prefix caching isn't helping.
- **Filenames carry information and are never folded by default.** `auth.ts`, `UserProfile.tsx`, `migrations/0042_users.sql` are structural signals on their own. `--max-depth` / `--expand` exist as opt-in escape valves for unusually large repos, not as a default.
- **Soft token warning.** When output exceeds a threshold (default 20k tokens), the header adds a line: `# warning: 32k tokens — consider --max-depth N`. No automatic pruning; agent decides. Beats Gemini's auto-truncate-at-5k approach because truncation hides filenames, which are information.
- Validate empirically in Spike B: measure tokens + task quality, not "looks nice."

### State: none

v1 ships stateless. No cache, no persistent files, no `.tingle/` directory. Every run is a full rebuild from source.

Rationale: at sub-second parse time, "re-run the CLI" is cheaper than cache invalidation. Cross-file invalidation (A changes its exports → B's resolved refs go stale even though B's content didn't change) is the hardest part of a cache, and we delete the problem entirely by not caching.

If Spike A finds parse time too slow to support "just run it again," the design changes before v1 starts — not a pre-approved v2 escape.

### Go data shapes

```go
type Symbol struct {
    Name      string
    Kind      SymbolKind // func, class, const, type, method
    Signature string     // single-line, no body
    Line      int
    Children  []Symbol   // methods under a class
}

type FileIndex struct {
    Path    string
    Tags    []string  // "M", "test", "untracked" — rendered as [M], [test], [untracked]
    Imports []string  // repo-relative when resolvable, else verbatim (e.g. "@okta/sdk", "@/foo")
    Defs    []Symbol
}

// No Refs field. v1 ranking is file-level (derived from Imports aggregation across
// Graph.Files), not symbol-level. Symbol-level fan-in is post-v1 scope.

type Graph struct {
    Files map[string]*FileIndex // the working in-memory repo state
}

// MapOutput is what the renderer consumes. One struct = one invocation's output.
type MapOutput struct {
    Manifests []string    // pre-rendered S records
    Entries   []string    // pre-rendered EP records, ranked
    Utilities []string    // pre-rendered U records, ranked by in-degree
    Edges     []string    // pre-rendered M records (dir → dir)
    Files     []FileIndex // source for F records; renderer walks Defs for line-numbered sigs
    Warnings  []string    // rendered as "# warning: ..." lines in header
}
```

### CLI surface

```
tingle                      # print compact codebase map to stdout
tingle --max-depth N        # collapse dirs deeper than N; prints summary (e.g. "utils/ [15 files omitted]")
tingle --expand PATH        # override --max-depth for paths matching PATH (drill-down)
tingle --stdin              # read file list from stdin (e.g. `git ls-files '*.ts' | tingle --stdin`)
tingle --alias PREFIX:PATH  # map an import prefix to a repo path; repeatable (e.g. `--alias '@:src' --alias '@app:src/app'`)
tingle --no-legend          # omit the legend header line (for re-invocations within a session)
```

`--json`, `--root`, `--remote`, `languages`, `doctor` deferred until someone needs them. No `--force` because there's no cache to force past.

`.tingleignore` is respected if present (repo root), same semantics as `.gitignore`. Applied on top of `git ls-files` filtering.

## v1 — the MVP

Scoped for side projects. Solid MVP, not a feature-complete parser. If a bullet isn't here, it's in §Discovered scope.

- **Binary:** Go, statically compiled, includes tree-sitter grammars for TS/JS, Python, Go, Kotlin, C++. Kotlin was validated in Spike A on a 167-file real Android repo (0 parse errors). Rust explicitly out of v1. Extensions parsed: `.ts` `.tsx` `.js` `.jsx` `.mjs` `.py` `.go` `.kt` `.kts` `.cc` `.cpp` `.cxx` `.h` `.hpp` `.hxx`.
- **Enumeration:** `git ls-files -com --exclude-standard` plus `.tingleignore` on top. No-git fallback: `filepath.WalkDir` with baked-in default ignores (`node_modules`, `dist`, `build`, `.venv`, `venv`, `target`, `.next`, `out`, `coverage`). `--stdin` accepts a pre-filtered list.
- **Parsing:** tree-sitter via `tree-sitter/go-tree-sitter` official bindings.
- **Extraction:** one language-agnostic Go function consumes standard `tags.scm` captures (`@definition.function`, `@definition.class`, `@reference.call`). Adding a language = drop in grammar + `tags.scm`, no Go code.
- **Import resolution:** heuristic — path math for relative imports + common-extension guessing. Optional `--alias PREFIX:PATH` flag (repeatable) handles user-supplied prefix maps (`@`→`src/`). External imports and unmapped aliases render verbatim. Full config-parsing (tsconfig, go.mod) → post-MVP.
- **Module graph:** resolved imports aggregated into `dir → dir` edges, emitted as `M` records.
- **Manifest surface:** parse `package.json` (scripts, bin, main) + `go.mod` (module path). Emit `S` records. `pyproject.toml`/`Cargo.toml` → post-MVP.
- **Tags:** `[M]` (modified tracked), `[untracked]`, `[test]` (path match on `.test.`, `.spec.`, `__tests__/`, `_test.go`). Git-log-based `[hot]` tag → post-MVP.
- **Ranking:** equal-weight heuristic — filename conventions + shebangs + manifest-declared entries + (out − in) degree + root-export bonus. Adjust weights only if real use shows bias.
- **Output:** compact tag-prefixed format (§Output format), stdout, line-number anchors on every def.
- **CLI:** `tingle`, `tingle --stdin`, `tingle --max-depth N`, `tingle --expand PATH`.
- **State:** none. Stateless. Stdout only.

## Discovered scope (post-MVP)

Intentionally deferred to keep the MVP focused. Each is added when real use surfaces a concrete need.

- **Config-aware import resolution** — tsconfig `paths`/`baseUrl`/`extends`, `go.mod` module root, Python `__init__.py` package walks, Kotlin package declarations. MVP accepts unresolved imports rendering as raw strings.
- **Framework route detection** — Express, Flask, FastAPI, Go `net/http` + `chi`/`gorilla`, Angular routes, Django, Fastify, Koa, NestJS, Click, Cobra. Each needs its own `tags.scm` query set. Scope caveat: tree-sitter queries catch literal-string routes only; runtime-constructed paths, mounted routers, and decorator-referenced handlers need semantic analysis.
- **Test → source mapping (`T` records).** "Which test covers `AuthService.login`?" Cheap to build (name-match on test-file imports), but best-effort: misses parameterized tests and over-emits when tests import helpers as utilities rather than SUTs.
- **`[hot]` tag** for files touched in the last 2 weeks via one `git log --since="2 weeks ago"` call.
- **Flat-layout fallback for `M` records.** Auto-switch to depth-2 granularity when the repo has ≤3 top-level dirs.
- **Secret redaction** — regex sweep on extracted string literals with `<redacted>` substitution. Needs a concrete pattern set (gitleaks rules) and a false-positive story before shipping.
- **`pyproject.toml` / `Cargo.toml` manifest parsing.**
- **External dep usage map** — for each third-party import, which internal files use it.
- **Config surface** — `process.env.*` / `os.Getenv` references with file:line.
- **Per-symbol fan-in** — resolving refs to specific symbol sites, not just file-level.
- `--json` output if agents request it.
- Monorepo / multi-workspace support.
- Homebrew tap + release binary hosting.

## Known gaps & risks

- **cgo multi-grammar link.** Each tree-sitter grammar is its own cgo module. Linking TS+JS+Py+Go+Kotlin means 5 cgo deps. **Target: <30MB binary** with `-ldflags="-s -w"`; fail the plan if >40MB. Reference points: `gh` ships ~40MB, `docker` ~70MB — our bracket is acceptable. Cross-compilation across OS/arch is painful; validate in Spike A. **Alternative considered: WASM tree-sitter via `wazero`** — pure Go, zero cgo, trivial cross-compilation. Rejected for v1 because wazero adds 8-10MB to the binary and WASM-compiled grammars run 2-3× slower than native C, pushing our <2s parse target to borderline. Revisit if cgo cross-compile pain outweighs the perf loss in practice.
- **Memory pressure from parallel parsing.** Parsing 2k files concurrently can spike RAM (each tree-sitter parse allocates its own tree). Bound the worker pool to `runtime.NumCPU()` via `errgroup.SetLimit()` — enough parallelism to saturate cores without piling up in-flight parse trees. Measure peak RSS in Spike A.
- **Kotlin grammar is community-maintained** (`fwcd/tree-sitter-kotlin`) and documented as incomplete on generics/DSLs. Validated in Spike A against a 167-file Android project: 0 parse errors. Risk closed for v1; re-open if larger/more complex Kotlin codebases show failures.
- **gotreesitter grammar edge cases (pure-Go reimplementation, v0.15.x).** After migrating to `gotreesitter` (zero cgo, 206 grammars), regression tests surface two known parser quirks where pure-Go captures differ from C tree-sitter:
  - **Kotlin `object Foo { ... }` declarations are not captured as classes** by `(object_declaration (type_identifier) @name)`. Methods inside the object still extract but the object itself is invisible. Workaround: use `class` or `class Foo : ParentClass` for DI modules/constants if visibility in the map matters. Upstream issue candidate.
  - **Python methods with f-string bodies in a class followed by top-level `def` can cause the following top-level function to be silently skipped.** Reproducer: class with `def greet(self): return f"hello {self.x}"`, then `def read_lines(path): ...` at module level — `read_lines` is missed. Likely a tokenizer/state bug in the pure-Go Python grammar. Workaround: move f-string-heavy classes to separate files from functions that follow them, or use `"hello " + str(x)` instead of f-strings in library code. Upstream issue candidate.
  - Both accepted for v1. Guarded by `internal/parse/parse_test.go` regression tests with explicit comments so future grammar upgrades can re-enable captures when fixed upstream.
- **Generated code / `node_modules` types.** `git ls-files` excludes them. Architecture understanding occasionally needs `.d.ts` from deps or generated protobuf. Known gap — not v1 scope.
- **Unresolved and dynamic imports render as raw strings.** MVP resolves only relative paths with common-extension guessing. Aliased imports (`@/foo` via tsconfig), module-path imports (`github.com/user/x`), and dynamic `require`/`importlib` loads all stay as raw strings in `imp:` on F records. They don't participate in the module graph. Acceptable for orientation; fixed by config-aware resolution post-MVP.
- **Secret leakage via extracted string constants.** Signatures rarely contain secrets but top-level const values can (hardcoded keys, URLs with tokens). MVP doesn't redact — the source code is already on the agent's disk, so this is less of a net-new leak than a latent risk. Redaction is post-MVP once we commit to a specific pattern set (gitleaks rules) with an honest false-positive story.

## Appendix: reviewer pushback (folded in)

The `@reviewer` agent surfaced the following before this doc was written. Resolutions:

| Reviewer concern                    | Resolution                                                                    |
| ----------------------------------- | ----------------------------------------------------------------------------- |
| cgo + multi-grammar reality check   | Phase 0 Spike A validates before commit                                       |
| Perf premise unmeasured             | Phase 0 Spike A, target <2s parse + <30MB binary + <200MB peak RSS            |
| Is structured digest > dumb dump?   | Phase 0 Spike B (3-way vs regex-only Go, `repomix --compress`)                |
| In-degree ranking produces junk     | Replaced with heuristic (§Ranking); utilities get separate section            |
| Import resolution can't be deferred | MVP heuristic only (path math + extension guessing); aliased imports stay raw |
| Cross-file cache invalidation       | Dropped — v1 is stateless, no cache                                           |
| msgpack unjustified at this scale   | Dropped — no cache means no serialization format                              |
| CLI surface too wide                | Trimmed to `tingle` + `--stdin` + `--max-depth` + `--expand`                 |
| macOS case-insensitive FS           | Dropped concern — no cache keys to normalize                                  |
| Generated code / node_modules types | Noted in §Known gaps as known limitation                                      |
