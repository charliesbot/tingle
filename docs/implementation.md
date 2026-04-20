# Implementation Guide

How tingle is built and how its output is generated. Design rationale lives in [design-doc.md](design-doc.md); rate–distortion measurements in [bench-results.md](bench-results.md).

---

## One-line summary

A Rust CLI that takes a repo path and writes a compact, ranked, tag-prefixed orientation map to `<repo>/.tinglemap.md` (or to stdout with `--stdout` for pipelines). Five pipeline stages, one output, no cache.

---

## Pipeline

```
repo path ──▶ enumerate ──▶ parse ──▶ resolve ──▶ rank ──▶ render ──▶ .tinglemap.md | stdout
```

Each stage reads the previous stage's output. No branching, no side effects, no retries.

### Stage 1 — Enumerate (`rust/src/enumerate.rs`)

**Goal:** ordered list of candidate files, each tagged with activity state.

```
files := `git ls-files -com --exclude-standard -z`     # NUL-separated; survives weird filenames
if .git missing:
    files := walkdir::WalkDir(repo)
    filtered by baked-in ignores: node_modules, dist, build, .venv, venv, target,
                                   .next, out, coverage, .git
apply .tingleignore patterns on top (either path)

# Dedupe — `ls-files -com` emits the union of cached/other/modified WITHOUT
# deduping. A tracked-and-modified file appears in both `-c` and `-m`.
dedup_preserving_order(files)

# Activity tags
modified  := `git ls-files -m`
untracked := `git ls-files -o --exclude-standard`
for each file:
    if path matches test heuristic:                    add "test" tag
    if path in modified:                                add "M" tag
    if path in untracked:                               add "untracked" tag
```

**Test heuristic** (covers Node, Python, Go, Android Gradle conventions):
- contains `.test.`, `.spec.`, `__tests__/`
- ends with `_test.go`
- starts with `tests/`, contains `/tests/`
- contains `/src/test/`, `/src/androidTest/`, `/src/testDebug/`, `/src/testRelease/`

### Stage 2 — Parse (`rust/src/parse/`)

**Goal:** per-file `(defs, imports, package)` via tree-sitter queries.

Dispatch by extension:

| ext | grammar crate | query file |
|---|---|---|
| `.ts` `.tsx` | `tree-sitter-typescript` | `typescript-tags.scm` / `tsx-tags.scm` |
| `.js` `.jsx` `.mjs` | `tree-sitter-javascript` | `javascript-tags.scm` |
| `.py` | `tree-sitter-python` | `python-tags.scm` |
| `.go` | `tree-sitter-go` | `go-tags.scm` |
| `.kt` `.kts` | `tree-sitter-kotlin-ng` | `kotlin-tags.scm` |
| `.cc` `.cpp` `.cxx` `.h` `.hpp` `.hxx` | `tree-sitter-cpp` | `cpp-tags.scm` |
| (anything else) | — | enumerated only |

```
for each parseable file in parallel (rayon::par_iter):
    tree := Parser::new().set_language(L).parse(file_bytes)
    matches := QueryCursor::new().matches(tags.scm, root, src)

    for each match's captures:
        capture_name ∈ {
            definition.{function, class, method, interface, enum, type, object, module},
            name.definition.*,
            name.reference.import,
            name.reference.package,    # Kotlin only — populates FileIndex.package
        }
```

**Language-agnostic extractor.** Adding a language = drop in a grammar crate, a `.scm` query file, and one entry in `LANG_DEFS`. No language-specific Rust code.

**Method attachment.** After the query pass, methods are attached to their enclosing class by byte-range containment. Unattached methods (e.g., top-level TS function signatures captured as `definition.method` by mistake) render as top-level functions.

**Signature rendering.** Raw text from `name_node.end_byte` to first of `{`, `=>`, ` where `, `;`, `\n\n`. Trim leading `= `. Convert trailing `: T` return annotation to `-> T`. Cap at 180 chars with `…`.

**Kotlin annotation workaround** (`decl_start_row` in `extract.rs`). The `tree-sitter-kotlin-ng` grammar has two known issues with `@Annotation`-prefixed declarations: (a) sometimes parses `@A @B private fun Foo() { … }` as a nested `annotated_expression` wrapping `infix_expression` rather than `function_declaration`; (b) call-style annotations like `@OptIn(...)` land as preceding-sibling `annotated_expression` nodes rather than being folded into the declaration's modifiers. Both are handled at extraction time so the reported line matches the original annotation line.

### Stage 3 — Resolve (`rust/src/resolve.rs`)

**Goal:** turn import strings into repo-relative paths for the graph stage. Maintain two parallel lists:
- `imports` — what RENDERS (display string)
- `resolved_imports` — what FEEDS THE GRAPH (repo-relative path)

For most languages these are the same; Kotlin differs because resolved paths (`core/src/main/java/com/x/.../Foo.kt`) are far longer than the canonical reference (`com.x.Foo` or compact `core/Foo`).

```
for each import in each file:
    if import points to a known repo file (already resolved):
        graph_edge ← import,  display ← import,  continue

    if import is relative (starts with .) → path math:
        target := clean_join(parent_dir(file), import)
        try: target, target+ext, target/index.ext, target/__init__.py
        if found: graph_edge ← target, display ← target

    if file is Kotlin → FQCN resolution:
        - Build package_index[fqcn_pkg][class_name] = file_path  (once, from parsed files)
        - Try longest-prefix split of the FQCN against package_index
        - If found: graph_edge ← full_path
                    display ← compact `<module>/<ClassName>` form
                    (e.g. `com.x.shared.core.constants.AppConstants` →
                          graph: `core/src/main/java/com/x/shared/core/constants/AppConstants.kt`,
                          display: `core/AppConstants`)

    fallback (Kotlin only): collapse_dotted
        if ≥3 dot segments: `androidx.compose.foundation.background` → `androidx.compose`
        Python explicitly excluded — middle segments carry signal.

    dedupe imports + resolved_imports per file
```

`clean_join` mirrors Go's `filepath.Clean(filepath.Join(base, rel))`: strips `.`, pops on `..`, preserves leading `..` segments that escape the base.

### Stage 4 — Rank (`rust/src/rank.rs`)

**Goal:** populate `out_deg` / `in_deg` and produce ranked Entry-Point + Utility lists.

```
graph(files):
    for each (src_file, imp in src_file.resolved_imports):
        if imp in repo:
            target.in_deg += 1
            callers[imp].push(src_file)
            if parent_dir(imp) ≠ parent_dir(src_file):
                src_file.out_deg += 1
                dir_edges[parent_dir(src)].insert(parent_dir(imp))

entry_points(files, opts):
    for f in files where lang ≠ "" AND defs ≠ [] AND "test" not in tags:
        score = sum(
            +10 if basename in {main.go, index.ts, index.tsx, index.js,
                                 server.ts, server.js, app.ts, app.tsx,
                                 cli.ts, manage.py, __main__.py}
            +8  if basename starts with "App."
            +10 if path declared in package.json bin/main/exports or go.mod cmd/*
            +10 if file starts with #!
            +5  if file at <module-root>/{src,cmd,pkg,internal}
            +(out_deg − in_deg)
        )
        if score > 0: include, sorted desc, capped at 15

utilities(files):
    every file with in_deg ≥ 2, sorted desc by in_deg
```

**Test files are excluded from EP** by tag-filter. Tests are probes of entry points, not entries themselves; without the filter, a test importing the system-under-test plus mocks plus fixtures would outrank the production class it tests.

**Orphan detection** runs in render. A file gets the `[orphan]` tag when `in_deg == 0` AND has defs AND not in entry list AND not test-tagged. Useful as "what's not referenced via static imports" — surfaces likely dead code. Limitation: tingle reads import statements only, so files wired through runtime registration (DI containers, AndroidManifest.xml, reflection-based routing, platform-callable Activities/Services/Workers/TileServices) all look orphan even when they're not. Documented honestly in the legend: `[orphan]=no-import-callers(may-be-runtime-registered)`. Agent verifies whether the absence of import edges actually means dead code.

### Stage 5 — Render (`rust/src/render.rs`)

**Goal:** compact tag-prefixed output to stdout. Section order is invariant; empty sections are omitted.

```
1. Build body (all sections in order)
2. Compute approx tokens (body + header so far) / 4
3. Build header:
     # tingle <ver>  gen=<date>  commit=<sha>  files=<n>  tokenizer=cl100k_base
     # legend: <context-aware — only mentions what's in THIS run's body>
     # warning: ~Nk tokens — ...   (only if approx > 20k)
4. Concatenate header + body
```

#### Anchor vs label paths

Critical distinction governing every path-rendering decision:

- **Anchors** are paths the agent will use with `Read(path, line=N)`. They must stay full and accurate. EP records, U record paths, F record paths, and `### <dir>` group headers are anchors.
- **Labels** are architecture signals. The agent never `Read`s a directory or a U-record's caller list entry. M record dirs and U caller lists are labels.

Only labels can be compressed. `compact_label_path` strips the Gradle source-root boilerplate (`<module>/src/main/<lang>/com/<org>/<proj>/`) from labels; anchors are always full.

#### Section emission

```
## Manifests       (omitted if no manifests)
S package.json  scripts: ...
S package.json  bin: ... main: ...
S go.mod        module=... go=...

## Entry points   (omitted if empty)
EP <full-path>:<line> <name> (out=N in=M) [hub]?
                                          # [hub] only if file ALSO appears in U
                                          # — orchestrator role doesn't fit
                                          # cleanly as either entry or utility

## Utilities      (omitted if empty)
U <full-path> (in=N)  ← <caller-label>+              # 1 caller by default,
                                                     # 3 with --full;
                                                     # callers use compact labels
                                                     # (drop Gradle boilerplate)

## Modules        (omitted if no edges)
M <src-label> -> <dst-label>+            # All labels via compact_label_path,
                                         # then deduped by compacted form
                                         # (multiple raw src dirs that compact
                                         # to the same label merge into one M
                                         # line; self-edges dropped).

## Files          (omitted entirely with --skeleton)
### <parent-dir-anchor>                  # Group header for dirs with ≥2 files;
                                         # singleton dirs skip the header (the
                                         # `###` would cost more than it saves)
F <basename-or-full-path> <tag-string>  imp: <imports> (+N more)?
                                         # tags: [M] [untracked] [test] [hub] [orphan]
                                         # imports: display strings, capped at 10
                                         # with `(+N more)` overflow
                                         # No def listings by default;
                                         # `--full` enables them
 <line> <kind> <signature>               # Only with --full; one space prefix
  <line> <kind> <signature>              # Class-attached method; two spaces
```

#### Context-aware legend

Only mentions categories that actually appear in this run:

```
# legend:
   {S=manifest}? {EP=entry(out=imports-out,in=imports-in)}?
   {U=utility(in=fan-in)}? {M=module-edge}? {F=file}?
   {[M]=modified}? {[untracked]=new-unstaged}? {[test]=test-file}?
   {[hub]=both-entry-and-utility}?
   {[path:line]=def f=func c=class m=method i=interface t=type e=enum}?  # only with --full
```

Prevents the "legend over-promises" UX bug where agents see `S=manifest` in the legend on a Kotlin Gradle project (no `package.json`/`go.mod` → no S records) and waste effort looking for what isn't there.

#### Soft token warning

When `(body + header) / 4 > 20_000`, prepend a line:

```
# warning: ~Nk tokens — consider --compact, --skeleton, or --scope PATH
```

No automatic pruning. Agent decides. Threshold is char/4 — a rough cl100k_base approximation. 20k matches the typical "fits in one tool result with room for reply" budget across most agent environments.

---

## CLI surface

```
tingle [REPO]                           # writes <REPO>/.tinglemap.md; prints status line
tingle --stdout [REPO]                  # print map to stdout (for pipelines)
tingle --out PATH [REPO]                # write to PATH instead of .tinglemap.md
tingle --full [REPO]                    # add per-file def signatures + 3 callers/U
tingle --scope PATH [REPO]              # filter F section to subtree
tingle --skeleton [REPO]                # drop F section entirely
tingle --alias PREFIX:PATH [REPO]       # repeatable; alias-substitute imports
tingle --no-legend [REPO]               # skip the legend line
tingle --version
```

File-write mode (default) dodges agent Bash-tool preview caps — agents
`Read('./.tinglemap.md')` instead of parsing truncated stdout. Status line
on stdout after a successful write:

```
wrote .tinglemap.md (36528 bytes, ~9.1k tokens)
```

tingle also prints a one-time hint if `.gitignore` in the repo root
doesn't cover `.tinglemap.md` — the file is a generated artifact and
committing it invites PR-review drift.

Atomic write: tingle writes to `.tinglemap.md.tmp.<pid>` and then
`rename()`s. Concurrent tingle invocations in the same CWD produce
last-writer-wins, not a torn read.

`--compact` is accepted as a hidden no-op for backwards compat (it's now the default).

---

## Data shapes (`rust/src/model.rs`)

```rust
pub enum SymbolKind { Func, Class, Method, Type, Interface, Enum }

pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub signature: String,   // single-line, name first
    pub line: u32,           // 1-indexed
    pub children: Vec<Symbol>,
}

pub struct FileIndex {
    pub path: String,
    pub ext: String,
    pub lang: String,            // "ts", "kt", "go", "" for unsupported
    pub tags: Vec<String>,       // "test", "M", "untracked"
    pub defs: Vec<Symbol>,
    pub imports: Vec<String>,    // DISPLAY strings (compact for Kotlin)
    pub resolved_imports: Vec<String>,  // GRAPH edges (full repo paths)
    pub package: String,         // Kotlin `package` header; "" elsewhere
    pub out_deg: u32,
    pub in_deg: u32,
}
```

Why `imports` and `resolved_imports` are decoupled: full Kotlin repo paths (`core/src/main/java/com/x/...`) are several times longer than the FQCN they resolved from. The graph needs the path; the agent reading the F record's `imp:` list does not. Splitting the field saved ~15% on the largest test repo with no information loss.

---

## State: `.tinglemap.md` only, no cache

Default behavior writes `<repo>/.tinglemap.md` — a generated artifact, not state. Every invocation regenerates from scratch; tingle never reads `.tinglemap.md` back. At sub-second parse time on test repos (see `bench-results.md`), "re-run the CLI" is cheaper than correct cache invalidation would be.

We originally rejected ALL on-disk output to avoid cross-file cache hell (a stale per-file cache entry pointing to wrong data). A whole-repo output artifact is a different beast — it's what the agent reads instead of tingle's stdout. No invalidation logic because there's no cache to invalidate.

---

## Build + install

```bash
make build                          # release binary at rust/target/release/tingle
make install                        # cargo install --path rust → ~/.cargo/bin/tingle
make test                           # cargo test
make bench                          # hyperfine + RSS + token counts → docs/bench-results.md
```

Cross-compile via standard Cargo targets. No cgo dependency.

---

## Eval harness (`evals/`)

Compression decisions are gated on agent task quality, not just token count. The harness scores answer quality across format variants by piping tingle output to `claude --print` against seeded questions.

```bash
bash evals/compare.sh /path/to/repo evals/questions/<repo>.yaml
```

Each question declares `expected_substrings` and `min_score`. Substring matching is dumb but reproducible up to LLM stochasticity. See `evals/README.md` for the scoring rules and the rejection of TASC L2 dictionary substitution (rate–distortion result: ~3-5% token win, not worth the implementation cost).

The eval framework is what justifies the compact-by-default flip: 47-58% token reduction across three real repos with mean answer score ≥ 0.97.

---

## Adding a language

Drop in:

1. A `tree-sitter-<lang>` crate dependency in `rust/Cargo.toml`.
2. A `<lang>-tags.scm` file under `rust/src/parse/queries/` using the standard aider capture names (`@definition.function`, `@name.definition.class`, `@reference.import`, etc.).
3. One entry in `LANG_DEFS` in `rust/src/parse/mod.rs`.

No language-specific Rust code anywhere else. The signature renderer, method attachment, ranking, and rendering are all language-agnostic.

If the language has a non-standard project layout that produces fat boilerplate prefixes (Android Gradle, Kotlin Multiplatform, etc.), `compact_label_path` in `render.rs` may need its source-root marker list updated.

---

## What this doc deliberately does NOT cover

- **Why** any of these choices were made → [design-doc.md](design-doc.md).
- **Measurement methodology** and rate–distortion data → [bench-results.md](bench-results.md), [evals/README.md](../evals/README.md).
- **Future scope** (deep mode, etc.) → [deep-mode.md](deep-mode.md).
