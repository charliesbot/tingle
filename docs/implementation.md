# Implementation Guide

How tingle is built. Design rationale lives in [design-doc.md](design-doc.md); measurement data in [spike-results.md](spike-results.md).

---

## One-line summary

A Go CLI that takes a repo path and prints a ranked, compact, tag-prefixed orientation map to stdout. Five pipeline steps, one output, no state.

---

## Algorithm

```
repo path ──▶ [1. enumerate] ──▶ [2. parse] ──▶ [3. resolve] ──▶ [4. rank] ──▶ [5. render] ──▶ stdout
```

Each step reads the previous step's output. No branching, no side effects, no retries.

### Step 1: Enumerate

**Goal:** ordered list of candidate files, each tagged with activity state.

```
files := `git ls-files -com --exclude-standard`     # git-aware, respects .gitignore
if .git missing:
    files := filepath.WalkDir(repo)
    filtered by baked-in ignores: node_modules, dist, build, .venv, venv, target,
                                    .next, out, coverage
apply .tingleignore patterns on top (either path)

for each file:
    if path matches test heuristic (.test. / .spec. / __tests__ / _test.go / tests/):
        add "test" tag
    if file appears in `git ls-files -m`:
        add "M" tag (modified tracked)
    if file appears in `git ls-files -o --exclude-standard`:
        add "untracked" tag
```

### Step 2: Parse (tree-sitter + tags.scm)

**Goal:** per-file `(defs, imports)` via tree-sitter queries.

```
dispatch by extension → (grammar, tags.scm query file):
    .ts  | .tsx       → tree-sitter-typescript + ts-tags.scm   (shipped)
    .js  | .jsx | .mjs → tree-sitter-javascript + js-tags.scm  (shipped)
    .py                → tree-sitter-python    + py-tags.scm   (shipped)
    .go                → tree-sitter-go        + go-tags.scm   (shipped)
    .kt  | .kts        → tree-sitter-kotlin    + kt-tags.scm   (shipped)
    .cc  | .cpp | .h .hpp → tree-sitter-cpp   + cpp-tags.scm   (shipped)
    .vue               → tree-sitter-vue + TS injection        (DEFERRED — see §Open questions)
    .mdx               → regex (frontmatter + top-level imports) (DEFERRED — see §Open questions)
    other              → enumerate only; no defs/imports

for each parseable file in parallel (bounded by runtime.NumCPU via errgroup.SetLimit):
    tree := parser.Parse(file_bytes)
    matches := query.Exec(tags.scm, tree.RootNode())

    for each match:
        capture_name ∈ { definition.function, definition.class, definition.method,
                         name, reference.call, import }
        by capture name → append to defs[] or imports[]
```

**Key design:** extraction code is **language-agnostic**. Adding a language = drop in a grammar binding + its `tags.scm` file. No Go code added per language. This is the aider/repomix pattern.

### Step 3: Resolve

**Goal:** relative imports → repo-relative paths. External and unmappable imports stay raw.

```
for each import in each file:
    if import does not start with "." or "..":
        keep raw (external package or unknown alias)
        continue

    if --alias flag supplied:
        apply any matching PREFIX:PATH substitution

    target := Clean(dirname(file.path) + "/" + import)

    try in order:
        have(target)                               → target
        have(target + ".ts"/.tsx/.js/.py/.go/.kt)  → target + matching ext
        have(target + "/index.ts"/...)             → index file
        have(target + "/__init__.py")              → Python package
        otherwise                                  → keep raw
```

No config file parsing (no tsconfig, no go.mod traversal) in v1. Keeps the resolver bounded and language-agnostic. Unresolved aliased imports are a documented limitation (see `--alias` flag as manual override).

### Step 4: Rank

**Goal:** produce the `Entry points` and `Utilities` lists.

**Entry-point score** per file with ≥1 def:

```
score =
      +10  if filename ∈ { main.go, index.ts, manage.py, App.kt, ... }
      +10  if file starts with #! (shebang)
      +10  if path is declared in package.json "bin"/"main"/"exports" or go.mod cmd/*/
       +5  if file sits at a package root (src/, cmd/, pkg/, internal/)
   + (out_degree − in_degree)    # calls many, called by few
```

Take top 15 files with score > 0, emit as `EP` records.

**Utility rank** = `in_degree`. Every file with `in ≥ 2`, sorted descending, emitted as `U` records.

Both thresholds are empirical. Weights land where Spike B feedback shows they're wrong; do not over-tune without data.

### Step 5: Render

**Goal:** compact tag-prefixed output to stdout. Format is fixed (§ Output format in design-doc.md).

Order of sections is invariant:

```
# tingle v<version>  gen=<iso>  commit=<sha>  files=<n>  tokens~<k>  tokenizer=cl100k_base
# legend: ...
# (optional) # warning: <message>

## Manifests        (omitted if no manifests detected)
## Entry points
## Utilities
## Modules          (omitted if graph is empty)
## Files
```

Each section emits one record per line. Def lines under F/U records are indented with a single leading space for the top-level def, two spaces for its children (class methods).

---

## Data types

```go
type SymbolKind string

const (
    KindFunc   SymbolKind = "f"
    KindClass  SymbolKind = "c"
    KindMethod SymbolKind = "m"
    KindType   SymbolKind = "t"
)

type Symbol struct {
    Name      string
    Kind      SymbolKind
    Signature string    // single-line; "name (params) -> return"
    Line      int       // 1-indexed
    Children  []Symbol  // methods under a class
}

type FileIndex struct {
    Path    string
    Tags    []string  // "M", "test", "untracked"
    Imports []string  // resolved paths or raw strings
    Defs    []Symbol

    // populated during rank step
    OutDeg int
    InDeg  int
}

type Graph struct {
    Files map[string]*FileIndex  // the in-memory repo state
}

// MapOutput is what render consumes — one struct per invocation.
type MapOutput struct {
    Manifests []string     // pre-rendered S records
    Entries   []string     // pre-rendered EP records, ranked
    Utilities []string     // pre-rendered U records, ranked by in-degree
    Edges     []string     // pre-rendered M records (dir → dir)
    Files     []FileIndex  // walked by renderer for F records
    Warnings  []string     // header warnings
}
```

---

## Package structure

```
tingle/
├── cmd/tingle/main.go             # entry point; arg parsing; pipeline wiring
├── internal/
│   ├── model/                     # shared Symbol, FileIndex, Graph, MapOutput types
│   ├── enumerate/                 # git ls-files, WalkDir fallback, .tingleignore, activity tags
│   ├── parse/                     # tree-sitter dispatch, tags.scm query execution
│   │   └── queries/               # aider-origin .scm files + tingle additions, embedded via go:embed
│   │       ├── typescript-tags.scm
│   │       ├── tsx-tags.scm
│   │       ├── javascript-tags.scm
│   │       ├── python-tags.scm
│   │       ├── go-tags.scm
│   │       ├── kotlin-tags.scm
│   │       └── cpp-tags.scm
│   ├── resolve/                   # relative-path resolution + --alias
│   ├── rank/                      # graph construction + entry-point + utility scoring
│   ├── render/                    # compact output emission
│   └── manifest/                  # package.json / go.mod → S records
├── docs/
│   ├── design-doc.md
│   ├── spike-results.md
│   └── implementation.md          # this file
├── spikes/                        # historic throwaway prototypes (kept as artifacts)
│   ├── a_perf/                    # Spike A: tree-sitter perf on real Kotlin
│   ├── b_utility/                 # Spike B: hand-rolled tree-sitter extractor (superseded)
│   └── b_regex/                   # Spike B: regex-only extractor (rejected path)
├── go.mod
├── go.sum
├── .gitignore
├── README.md
└── tingle                         # built binary (gitignored)
```

Queries live inside `internal/parse/queries/` because `go:embed` resolves paths relative to the Go source file. Each `internal/*` package exposes one or two exported types + a constructor. No cross-package mutable state.

---

## Dependencies

**Runtime:**

- `github.com/smacker/go-tree-sitter` + per-language grammar subpackages
  (`.../typescript/typescript`, `.../typescript/tsx`, `.../javascript`, `.../python`, `.../golang`, `.../kotlin`, `.../cpp`)
- `golang.org/x/sync/errgroup` — bounded worker pool
- `github.com/sabhiram/go-gitignore` — `.tingleignore` matcher
- Standard library otherwise

No hashing, no serialization library — v1 is fully stateless.

**Build:**

- Go 1.22+
- A C compiler (cgo requirement for tree-sitter grammars)

**No runtime dep on Node / Python / external binaries** (besides `git`, which we shell out to).

**Measurement tooling** (not shipped):

- `tiktoken` (Python) for token counting in spike scripts

---

## CLI surface

**Implemented in v0-dev:**

```
tingle [REPO]                   # print map to stdout (default: cwd)
tingle --alias PREFIX:PATH      # apply import-prefix substitution (repeatable)
tingle --no-legend              # omit the legend header line (for re-invocation in-session)
tingle --version                # print version and exit
```

**Planned but not yet implemented:**

```
tingle --stdin                  # read file list from stdin
tingle --max-depth N            # collapse dirs deeper than N with summary stubs
tingle --expand PATH            # override --max-depth for matching paths
```

These are in the design doc but had no concrete demand during v1 build. Add when an agent workflow actually needs them.

**Deferred indefinitely:** `--force`, `--json`, `--remote`, `--output`. Add only when a concrete need shows up.

---

## Code conventions

- **Error handling:** return `error`, propagate up, fatal errors hit stderr with nonzero exit. No `panic` in the happy path.
- **Concurrency:** every parallel section is bounded by `runtime.NumCPU()` via `errgroup.SetLimit()`. No unbounded goroutines.
- **Allocation:** re-use `[]byte` where cheap. Do not pre-optimize; profile first.
- **No global mutable state.** Test seams at package boundaries.
- **Interfaces minimized.** Concrete types unless there's a real second implementation.
- **Logging:** stderr only, warnings only, during normal runs. Info-level logging kills the CLI use case.
- **Don't hide errors behind default values.** If a file fails to parse, emit a `# warning:` line; don't silently drop it.

---

## Testing strategy

**Current state (v0-dev):** no automated tests yet. Verification has been manual — running tingle against three real repos (`advent-of-code`, `charliesbot.dev`, `one`) and spot-checking output. Not sustainable; tests are the next real gap.

**Planned:**

- **Unit tests** per internal package. Golden-file outputs for `enumerate`, `resolve`, `rank`, `render`, `manifest`.
- **Integration test** — run tingle against a small fixture repo (committed under `testdata/`), diff stdout against a golden file.
- **Per-language extraction tests** — one representative file per supported language in `testdata/languages/`, assert expected def and import lists.

**Not planned to test:**

- Tree-sitter grammar correctness (upstream's responsibility).
- Output stability across Go minor versions.
- Edge cases in languages we don't officially support.

---

## Build + ship

```bash
# Dev build
go build -o tingle ./cmd/tingle

# Release build — strip symbols; target <30 MB
go build -ldflags="-s -w" -o tingle ./cmd/tingle
```

**Current binary size:** ~13 MB stripped with TS/TSX/JS/Python/Go/Kotlin/C++ grammars linked. Well under the 30 MB ceiling.

**Cross-compile:** requires cgo and a C cross-toolchain for the target platform. Tree-sitter grammars are C code; `CGO_ENABLED=0` breaks them.

```bash
CGO_ENABLED=1 GOOS=linux GOARCH=amd64 CC=x86_64-linux-gnu-gcc go build ...
```

**Distribution:** GitHub Releases with darwin-{amd64,arm64}, linux-{amd64,arm64}, windows-amd64. Homebrew tap is post-v1.

---

## Resolved decisions (were open questions during build)

1. **`tags.scm` source:** aider's (`Aider-AI/aider/aider/queries/tree-sitter-language-pack/*.scm` + older `tree-sitter-languages/{typescript,kotlin}-tags.scm`). MIT-licensed, copied verbatim then augmented per-language with tingle-specific captures: `@reference.import` for import extraction across all languages, and a `variable_declarator` pattern for TS/JS to catch arrow functions assigned to `const`. Attribution is implicit via the query text similarity; add a NOTICE file if redistributing.
2. **Tree-sitter binding library:** `smacker/go-tree-sitter`. Covers all v1 languages. Move to `alexaandru/go-sitter-forest` only if a needed grammar is missing (Vue, if ever).
3. **Manifest surface depth:** `scripts` + `bin` + `main` / `module` only. Deps not included — `npm ls` does that job better.

## Open implementation questions

1. **Kotlin FQCN-based import resolution.** Kotlin imports are `com.foo.bar.Baz`, not `./baz`. Heuristic path math doesn't resolve them → `## Utilities` and `## Modules` are empty on Kotlin repos. Fix: parse the `package` header in each Kotlin file to build an `FQCN prefix → directory` map, then resolve imports by longest-prefix match + filename-in-dir lookup. ~60-80 lines in `internal/resolve/`. Not hard, but real per-language work. Worth doing because Kotlin output is visibly worse than TS output today.
2. **Vue support.** Deferred; designed. Current state: `.vue` files are enumerated but produce no defs/imports — agent sees them in the F section as name-only entries. User has Vue projects (e.g., `gemini-cli-slides` is Slidev/Vue) so this is a real gap, not hypothetical.

    **Planned approach:**

    - Add `github.com/alexaandru/go-sitter-forest/vue` — the Vue grammar isn't in `smacker/go-tree-sitter`; forest ships it.
    - Parse `.vue` files with the Vue grammar; locate the `script_element` node in the SFC.
    - Extract the script block's byte range and re-parse that range with the TS grammar (keeping a line offset for anchors).
    - Re-use the existing TS `tags.scm` pass over the reparsed sub-tree.
    - Template-level component usages (`<MyComponent>`) would need a separate Vue-grammar pass; defer that until someone asks.

    **Cost estimate:** ~100-150 LOC in `internal/parse/` for the SFC dispatch + offset mapping. One new dep (forest/vue). Binary size grows by ~2-3 MB for the Vue grammar.

    **Why it's not in v1:** scope discipline. v1 works well on the repos tingle users actually run it against most often (Go / TS / Kotlin). Vue support is a "next-most-common language" add, worth doing but not the highest priority over tests + Kotlin FQCN resolution.

3. **MDX support.** Deferred. MDX has no maintained tree-sitter grammar (it's markdown + embedded JSX + top-level imports). Regex handler for frontmatter + top-level `import`/`export` lines is ~30 LOC. Add together with Vue or when a real project demands it.

4. **Svelte / Astro / Solid** — same injection pattern as Vue. Once Vue is wired, these become trivial adds (grammar + same SFC dispatch). Not scoped for v1.

## Implementation notes worth capturing

- **Signature rendering:** raw text from `nameEnd` to the first of `{`, `=>`, `where`, `;`, `\n\n`. Trims leading `= ` to keep `const foo = () =>` signatures clean. Caps rendered sig length at 180 chars with `…` to prevent pathological type unions from exploding. Converts `: T` return annotation to `-> T` for cross-language consistency in output.
- **Method attachment:** after the query pass, methods are associated with their enclosing class by byte-range containment. Unattached methods (e.g., top-level function signatures in TS) render as top-level functions.
- **Def ordering within a file:** insertion-sorted by line number. Typical file has <100 defs so algorithm choice doesn't matter.
- **Concurrency:** parse step runs files in parallel, bounded at `runtime.NumCPU()` via `errgroup.SetLimit()`. Each goroutine gets its own `sitter.Parser` + `sitter.QueryCursor` — neither is safe for concurrent use. Queries themselves are compiled once via `sync.OnceValue` and shared across goroutines.
- **Kotlin `package` capture:** already wired in the query (`@name.reference.package`). Data is captured but not yet consumed — the FQCN resolver above is the planned consumer.

---

## What this doc deliberately does NOT cover

- **Why** any of these choices were made → [design-doc.md](design-doc.md).
- **Prior art** (aider, repomix) → [design-doc.md § Prior art](design-doc.md#prior-art).
- **Measurement methodology** and Spike A/B results → [spike-results.md](spike-results.md).
- **Discovered scope** (things we won't build in v1) → [design-doc.md § Discovered scope](design-doc.md#discovered-scope-post-mvp).
- **Benchmarks / eval harness** design → future doc when built.
