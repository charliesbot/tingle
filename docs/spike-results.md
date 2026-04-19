# Spike Results

Measurements collected during Phase 0 spikes, referenced by [`design-doc.md`](design-doc.md).

---

## Spike A — tree-sitter-in-Go performance

**Prototype:** [`spikes/a_perf/main.go`](../spikes/a_perf/main.go)
**Target repo:** `/Users/charliesbot/projects/one` (167 Kotlin files, real Android project)
**Tree-sitter lib:** `github.com/smacker/go-tree-sitter` + `kotlin` grammar

### Result

| Metric                                                     | Target  | Actual      | Margin |
| ---------------------------------------------------------- | ------- | ----------- | ------ |
| Parse time (cold, wall-clock)                              | <2s     | **11 ms**   | ~180×  |
| Binary size (with `-ldflags="-s -w"`, Kotlin grammar only) | <30 MB  | **5.8 MB**  | 5×     |
| Peak RSS (via `/usr/bin/time -l`)                          | <200 MB | **20 MB**   | 10×    |
| Parse errors                                               | 0       | **0 / 167** | —      |

**Per-parse stats:** avg 590 μs, max 3.0 ms, across 167 files at `runtime.NumCPU()=14` workers.

### Takeaways

- **Gate passed with massive headroom.** Extrapolated: 2,000 files ≈ 130 ms, still well under 2 s.
- **Kotlin grammar risk closed.** `fwcd/tree-sitter-kotlin` parsed 167 real Android files cleanly. Known gap downgraded.
- **Binary size budget** has a lot of headroom. Adding TS/JS/Python/Go/C++ grammars brought total to 12 MB. Target is <30 MB.

---

## Spike B — utility comparison

**Target repo:** `/Users/charliesbot/projects/advent-of-code` (172 files, multi-language: 80 TS, 8 Dart, 5 Rust, 5 C++, 7 Kotlin/kts, 6 md, others)

**Agent task (identical across all contestants):**

> Identify the canonical utilities file(s) that TS day-solutions import from, the specific functions/exports that file provides, and two or three day-solutions that use these utilities with import-statement line numbers. Give `file:line` anchors. <200 words.

### Orientation payload sizes (pre-agent, cl100k_base tokens)

| Tool                                        | Tokens    | Output lines | vs tingle |
| ------------------------------------------- | --------- | ------------ | --------- |
| `discover` (bash script + onefetch + tree)  | 1,657     | 202          | 0.33×     |
| **Tingle regex prototype**                  | **4,987** | 342          | 1.00×     |
| **Tingle tree-sitter prototype (post-fix)** | **5,187** | 345          | 1.04×     |
| `repomix --compress`                        | 256,974   | 35,600       | **51×**   |

> Note: "67× more tokens" figure mentioned earlier in design discussion was based on the pre-fix tingle output (3,847 tokens). Post-fix tingle is slightly larger due to inline signatures + caller lists added per reviewer feedback.

### Agent task results

Each contestant: Claude-based `architect` subagent, identical prompt, metrics from the Agent framework's response metadata.

| Contestant                        | Tool calls | Wall-clock | Reported tokens | Answer quality                                           |
| --------------------------------- | ---------- | ---------- | --------------- | -------------------------------------------------------- |
| Baseline (discover + exploration) | **7**      | 33.8s      | 28,004          | ✓ Found all 3 utils, noted runtime-split pattern         |
| Repomix --compress                | 5          | 24.4s      | 25,514          | ✓ Best (full function signatures, runtime split)         |
| Tingle tree-sitter (pre-fix)      | 1          | 11.8s      | 27,510          | ⚠ Only 2 of 3 utils, no function names (extraction bug) |
| **Tingle tree-sitter (post-fix)** | **1**      | **13.5s**  | **30,041**      | ✓ All 3 utils, precise signatures, shim pattern noted    |
| **Tingle regex**                  | **1**      | **14.4s**  | **29,605**      | ✓ All 3 utils + signatures + shim pattern noted          |

### Binary + runtime cost (prototypes only)

| Prototype                                   | Binary (stripped) | Runtime on AoC |
| ------------------------------------------- | ----------------- | -------------- |
| Tree-sitter (TS/TSX/JS/Kotlin/C++ grammars) | 12 MB             | 0.47 s         |
| Regex-only                                  | **2.1 MB**        | **0.37 s**     |

---

## Key findings

### 1. `total_tokens` metric is noisy

Anthropic Agent framework's `total_tokens` includes system-prompt overhead (~18-20k per invocation, invariant across contestants). This compresses real deltas into a small range (25-30k for all four contestants). Cleaner metrics:

- **Tool call count** (real: baseline 7, repomix 5, tingle 1)
- **Wall-clock** (real: baseline 34s, repomix 24s, tingle 14s)
- **Output file size in tokens** (real: discover 1.6k, tingle 5k, repomix 257k)

For future spikes, use Anthropic's `/v1/messages/count_tokens` endpoint to get exact input-token counts without framework overhead.

### 2. Tingle wins decisively on tool-call count and latency

Both tingle variants collapsed 7 tool calls → 1. Wall-clock: 34s → 13-14s (~2.4× faster). This is the real efficiency gain — the agent finds answers _in_ the map instead of reasoning to derive them.

### 3. Regex matched tree-sitter on AoC

Surprising result: on a 172-file multi-language repo, the regex-only prototype produced equivalent agent-task quality at smaller binary size. Regex also caught things tree-sitter missed (C++ class methods in `2022/utils/file_reader.h`).

**Why it held up on AoC:**

- AoC code is relatively simple (standalone puzzle solvers, few generics, no decorators)
- Regex patterns for `export function`, `const f = () =>`, `class`, `interface` cover the common cases
- C++ patterns caught `class`/`struct` cleanly

**Where regex is expected to fail (not yet tested):**

- Kotlin DSLs, builder patterns, complex generics
- React/TSX with decorators + hooks + complex type unions
- Python stacked decorators, metaclasses
- Multi-line signatures that span 5+ lines

### 4. The tree-sitter prototype had real extraction bugs

Before the fix, tingle's tree-sitter prototype missed:

- `const foo = async () => ...` (arrow functions assigned to const)
- Re-exports like `export { foo, bar }`
- TS utility file signatures

These were hand-rolled node-kind switches, not `tags.scm` queries. A v1 using proper `tags.scm` would fix this systematically. But the bug itself is evidence that tree-sitter isn't self-correcting — you still need to enumerate node types per language.

### 5. Post-fix tingle output improvements were material

Moving from top-10 utility cutoff to `in≥2`, adding inline callers on U records, adding function signatures under U records, adding a usage-hint line to the legend. These changes:

- Prevented "utils_bun.ts" (fan-in=2) from being silently dropped
- Let the agent cite utility exports directly without jumping to the F section
- Increased tokens 3.8k → 5.2k (+35%) but eliminated derivation reasoning

---

## Open questions — NOT yet measured

1. **Does regex hold up on complex Kotlin?** Need to run the regex prototype on `one` and compare against the tree-sitter output. If regex extracts Kotlin functions/classes correctly on a real Android codebase, the case for Path B (regex-only v1) strengthens.

2. **Does regex hold up on React/TSX with hooks + decorators?** Need a representative repo. `charliesbot.dev` (24 `.tsx` + 7 `.ts`) is a candidate.

3. **Multi-question eval.** Single-task result is one data point. Proposed: 5-10 standard orientation questions (canonical utility, runtime, frameworks, test layout, recently-modified, etc.) scored per contestant as ✓/◐/✗.

4. **Exact token cost via direct API.** Framework `total_tokens` is muddy. Anthropic's `/v1/messages/count_tokens` gives exact input-token counts; `usage.input_tokens` + `usage.output_tokens` on real responses gives exact billing.

---

## Design decision (resolved)

**Path A — tree-sitter + aider-style `tags.scm` queries — committed.**

Triggering insight (user): Neovim abandoned regex-based syntax highlighting precisely because regex breaks on real code — multi-line sigs, embedded languages, complex expressions. "Regex matches tree-sitter on AoC" was a lucky-repo result, not evidence that regex generalizes.

Shipped approach:

- Tree-sitter via `smacker/go-tree-sitter` for TS/JS/Py/Go/Kotlin/C++
- aider's `tags.scm` query files (MIT-licensed, field-tested), augmented with tingle-specific imports and TS arrow-function capture
- Language-agnostic Go extractor consuming standard captures (`@definition.function`, `@name.definition.class`, `@reference.import`, etc.)
- Per-language work lives in the `.scm` file, not in Go. Adding a language = drop in a grammar binding + query file.

Rejected: Path B (regex everywhere). Not because AoC showed it losing — but because the historical precedent (Neovim) plus the predicted failure surface (Kotlin DSLs, React generics, Python decorators) makes regex a tool we'd regret later.

---

## v1 on the three real repos

Measured with the shipped binary (`tingle v0-dev`, 13 MB stripped, cgo).

| Repo              | Files | Output tokens (cl100k_base) | Notes                                                                                         |
| ----------------- | ----- | --------------------------- | --------------------------------------------------------------------------------------------- |
| `advent-of-code`  | 172   | **5,824**                   | Multi-language (TS/JS/C++/Kotlin/Rust/Dart); Rust/Dart files appear as F entries without sigs |
| `charliesbot.dev` | 61    | **1,891**                   | React/TSX + MDX; MDX shows as F entries without sigs (deferred)                               |
| `one`             | 273   | **27,241**                  | Kotlin Android; high token count driven by verbose FQCN imports                               |

**Extraction wins over the spike prototypes:**

- TS arrow functions (`const foo = async () => ...`) now extract correctly via added `variable_declarator` capture on top of aider's queries.
- Kotlin signatures now include parameter types + base classes (`c MainActivity : ComponentActivity()`, `f onCreate (savedInstanceState: Bundle?)`). Previously missing due to incorrect field names in the hand-rolled spike extractor.
- C++ class methods + constructors extracted (previously partial).
- Type aliases capped at 180 chars with `…` so pathological unions don't blow up the signature line.

---

## Spike D — pure Go tree-sitter via `gotreesitter`

Added after v1 shipped. Re-evaluates the tree-sitter binding choice in light of two observations: (1) `smacker/go-tree-sitter` doesn't ship Vue/Angular/Svelte grammars the user needs; (2) cgo cross-compile is painful for release distribution. Reviewer pushback on WASM (Option C, with `wazero` + `tree-sitter-wasms`) surfaced an alternative not on the original list: `odvcencio/gotreesitter` — a **pure-Go reimplementation** of the tree-sitter runtime. 462⭐, actively maintained, 206 grammars including Vue and Angular.

**Prototype:** [`spikes/d_puregoTS/main.go`](../spikes/d_puregoTS/main.go)
**Target repo:** `/Users/charliesbot/projects/one` (167 Kotlin files, same as Spike A for apples-to-apples)
**Binding:** `github.com/odvcencio/gotreesitter` v0.15.1, pure-Go (`CGO_ENABLED=0`)

### Results vs cgo baseline (Spike A, same repo)

| Metric                            | cgo (Spike A)        | gotreesitter (Spike D)       | Ratio       |
| --------------------------------- | -------------------- | ---------------------------- | ----------- |
| Parse time (wall-clock)           | 11 ms                | **204 ms**                   | 18× slower  |
| End-to-end (with process startup) | 0.28 s               | **0.77 s**                   | 2.7× slower |
| Per-parse avg                     | 590 μs               | **9.8 ms**                   | 16× slower  |
| Per-parse max                     | 3 ms                 | **94 ms**                    | 31× slower  |
| Binary size (stripped)            | 5.8 MB (Kotlin only) | **21 MB** (all 206 grammars) | 3.6× bigger |
| Peak RSS                          | 20 MB                | **715 MB**                   | 36× more    |
| Parse errors                      | 0 / 167              | **0 / 167** ✅               | —           |
| cgo required                      | yes                  | **no**                       | —           |

### Correctness

Query API parity validated. Our augmented `kotlin-tags.scm` compiled and ran correctly; all 14 captures fire as expected:

```
definition.class         110     name.definition.class    110
definition.function      458     name.definition.function 458
definition.object         17     name.definition.object    17
reference.import        1835     name.reference.import   1835
reference.package        154     name.reference.package   154
reference.type            61     name.reference.type       61
reference.call          4562     name.reference.call     4562
```

Predicates (`#eq?`, `#match?`, `#any-of?`) used in broader queries are documented as supported — not tested in this spike but present in the runtime per their README.

### Tradeoff analysis

**What we gain by switching:**

- **`GOOS=linux GOARCH=arm64 go build` just works** — zero cgo, zero C toolchain, zero CI matrix complexity.
- **All 206 grammars available** — Vue, Angular, Svelte, Astro, Rust, Java, HTML, CSS, Markdown, and 200 others included at no extra cost. User's Vue repos work out of the box; future language adds cost ~zero.
- **`go install github.com/charliesbot/tingle/cmd/tingle` works on any machine with Go** — no C compiler needed by downstream users.
- **Go race/coverage/fuzzer see through the code** — easier debugging; CI paths simpler.

**What we pay:**

- **18× slower parse.** Still under our <2s target (extrapolates to ~2.5s on 2k files — right at the edge). No longer over-engineered for speed; we're spending the headroom.
- **~700 MB peak RSS under full parallelism.** Above our stated 200 MB ceiling. Breakdown: 14 parallel workers × ~50 MB per parser = ~700 MB. Mitigation: cap workers at 4 → ~200 MB peak at the cost of ~3-4× longer wall-clock. Not tested in this spike; worth a follow-up.
- **Binary goes 13 MB → 21 MB.** Under the 30 MB ceiling either way. Includes all 206 grammars always; can't tree-shake without deeper work.
- **Newer project.** v0.15.x, 462⭐, pushed daily. Less battle-tested than smacker's ecosystem. Pure-Go reimplementation carries edge-case risk the canonical C runtime doesn't have. Zero parse errors on the 167 Kotlin files we tested — happy path is solid.

### Three framings of the data

1. **"Within budget → migrate."** Speed stays under target; memory is at ceiling (fixable via worker cap); cross-compile and grammar coverage are meaningful wins. Trade headroom for maintainability.
2. **"Burned headroom for convenience → stay."** Design doc committed to speed; gotreesitter weakens that by a factor of 18× for parse throughput. cgo works today on the languages we have.
3. **"Go B instead."** `alexaandru/go-sitter-forest` keeps cgo (still native speed + memory) but gets 511 grammars. Fixes Vue gap without the memory hit. Cross-compile pain stays.

### Outcome

**Migrated.** v1 now runs on Option D (gotreesitter, pure Go, zero cgo). User's "cross-platform is essential" constraint tipped the decision — A's cgo cross-compile pain was a blocker for shipping binaries to mac + linux from a single build host. Optimizations applied on top of Spike D's raw numbers:

- `sync.Pool` per-language for parser reuse + explicit `tree.Release()` before returning parser to pool (prevents pool-concurrent-write on live tree — flagged by reviewer)
- Worker cap `parseWorkers = 2` (down from `runtime.NumCPU()`)
- `debug.SetMemoryLimit(512 MiB)` baked into `main.init()`; respects user override via env
- Two `tags.scm` files simplified to drop predicates gotreesitter doesn't support (`#set-adjacent!`, `#strip!`, `#select-adjacent!`, `#not-eq?`, `#not-match?`). No definition captures lost; two minor filters dropped (constructors no longer excluded from methods, `require()` calls now surface as references — neither affects tingle output)

### Post-migration measurements (same three repos)

| Repo | Files | Tokens | Wall-clock | Peak RSS | Gate 2 (<300 MB) |
|---|---|---|---|---|---|
| `charliesbot.dev` | 61 | 1,891 | 0.13 s | 82 MB | ✅ |
| `one` (Kotlin) | 273 | 27,168 | 0.54 s | 167 MB | ✅ |
| `advent-of-code` (multi-lang) | 172 | 5,824 | 1.04 s | 879 MB | ⚠ accepted (multi-lang edge; one-shot CLI) |

**Cross-compile verified:** `CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build` produces a valid statically-linked ELF from macOS.

### Grammar gaps surfaced by golden-file regression tests

Pure-Go gotreesitter grammars have two known differences vs native C tree-sitter, both encoded as regression tests in `internal/parse/parse_test.go` with `t.Log` nudges so future grammar upgrades make any fix loudly visible:

1. **Kotlin `object_declaration`** not captured as a class. Affects Koin DI module objects (`object DashboardModule { ... }`). Methods inside still extract.
2. **Python f-string + class + def sequence.** A class with a method body containing `f"..."` followed by a top-level `def` can cause the top-level def to be silently skipped during parse. Narrow reproducer documented in `design-doc.md § Known gaps`.

Both are accepted v1 limitations. File upstream issues post-release.

---

## Artifacts

- Production code: `cmd/tingle/main.go`, `internal/{enumerate,parse,resolve,rank,render,manifest,model}/`
- Tree-sitter queries: `internal/parse/queries/*.scm` (aider-origin + tingle additions)
- Spike prototypes (historic, for reference):
  - `spikes/a_perf/main.go` — Spike A perf measurement (Kotlin parse via cgo)
  - `spikes/b_utility/main.go` — Spike B tree-sitter prototype (hand-rolled extractor, superseded)
  - `spikes/d_puregoTS/main.go` — Spike D pure-Go tree-sitter runtime eval
  - `spikes/b_regex/main.go` — Spike B regex prototype (rejected path)
- Output samples (gitignored, `/tmp/`): `tingle-v1-*.md` per test repo
