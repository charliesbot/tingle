# Deep Mode — Design Doc (Future Phase)

**Status:** Deferred. Captured for future consideration; not scoped for the current v1 release.
**Author:** charliesbot
**Date:** 2026-04-18

---

## Problem

tingle today is a pure orienter: signatures, ranked entry points, utilities, module graph, activity tags. Great for session-start orientation, but when an agent's next question is *"now show me what that utility actually does"*, it has to follow up with a file `Read` — cheap but adds a roundtrip.

[repomix](https://github.com/yamadashy/repomix) solves a different problem: pack the whole repo for full-LLM analysis. It emits every file with signatures (via `--compress`) or full bodies. That's 50k-500k tokens per repo — great for one-shot chat ("review my code"), too expensive for repeated agent-loop orientation.

**The gap between them:** there's no tool that gives you *ranked orientation + bodies of the ranked files*. That middle ground is what this design is about.

---

## Non-goals

- **Not a repomix replacement.** We're not packing every file. repomix owns that use case.
- **Not a config-parser.** No new languages, no new extractors. This is an output-shape change, nothing else.
- **Not two binaries.** Same `tingle` CLI, same pipeline, one extra flag.

---

## Proposals

Three paths exist for giving an agent the ability to do real analysis on top of tingle's orientation. Each is described below with pros, cons, and when it would be the right choice. Proposal B is tagged `[PREFERRED]`.

### Proposal A — Unix pipe composition

Keep tingle pure orienter. Add a minimal `--list-ranked` flag that emits just the file paths of the ranked subset (EPs + qualifying Us) with no signatures, then compose with another tool:

```bash
tingle /path/to/repo --list-ranked | repomix --stdin
```

**Output shape:** plain list of file paths, one per line, ordered by ranking.

**Pros:**

- Cleanest separation of concerns. tingle's identity stays "orient only." repomix owns packing. Unix philosophy wins.
- Smallest possible tingle-side change (~20 LOC: one flag, one alternate output mode).
- Users who already have repomix get the analyzer path for free. No reinvention.
- Two tools composed means each one evolves independently.

**Cons:**

- Requires both tools installed. Harder to ship to users who want "one binary that does it all."
- Output format of the packed content is repomix's, not tingle's. Agent-friendly formatting (legend-based records, inline ranking context) is lost in the downstream stage.
- Ranking signal is *consumed* by the filter but not *propagated* to the output. The packed result is flat alphabetical; the agent still has to re-derive "which of these matters most."
- Two tools = two release cycles, two install docs, two bug-report surfaces.

**When this would be the right choice:**

If you already assume a Unix-savvy user environment with both tools, want to keep tingle's surface minimal, and don't care about preserving the ranked structure in the output.

---

### Proposal B — `--deep` flag in tingle  `[PREFERRED]`

tingle emits its normal orientation map *plus* the full source bodies of its top-ranked files inline, under their `EP`/`U` records, in fenced code blocks.

#### Output shape

Without `--deep` (current behavior):

```
# tingle v1  gen=2026-04-18  commit=abc1234  tokens~5k
# legend: ...

## Entry points
EP src/main.ts:12 bootstrap (out=14 in=0)

## Utilities
U utils.ts (in=18)  ← day01.ts day02.ts ...
 1 f getInputLines async (fileName: string) -> Promise<string[]>
 6 f getParagraphs async (fileName: string)

## Files
F src/main.ts imp: ./utils
 12 f bootstrap
```

With `--deep`:

```
# tingle v1  gen=2026-04-18  commit=abc1234  tokens~48k  mode=deep
# legend: ...

## Entry points
EP src/main.ts:12 bootstrap (out=14 in=0)
```ts
// full src/main.ts body here, fenced
export async function bootstrap() { ... }
```

## Utilities
U utils.ts (in=18)  ← day01.ts day02.ts ...
 1 f getInputLines async (fileName: string) -> Promise<string[]>
 6 f getParagraphs async (fileName: string)
```ts
// full utils.ts body here, fenced
const getInputLines = async (fileName: string): Promise<string[]> => { ... };
```
```

Each ranked file's body is emitted as a fenced code block directly under its `EP` or `U` record. The `F` section stays signature-only.

#### Which files get bodies?

- **All emitted `EP` records** — top entry points by ranking heuristic. Default cap: 15 (same as today's cap).
- **All emitted `U` records with `in >= 3`** — load-bearing utilities. Lower-ranked utilities (in=2) stay signature-only.
- **No `F` records get bodies.** The file list stays lightweight; bodies are surfaced only for *ranked* files.

Rationale: we're emitting bodies for files the agent would likely want to read next anyway. Saves 1-15 follow-up `Read` tool calls per session.

#### Flags

```
tingle --deep                        # include bodies for EP records + U records with in>=3
tingle --deep --deep-max-files N     # cap bodies at top-N combined (default 25)
tingle --deep --deep-max-bytes N     # cap per-file body bytes (default 50KB)
```

Reasonable defaults. Override for tight contexts.

#### Token-budget expectation

Rough estimates on the three test repos:

| Repo                     | v1 tokens | `--deep` est. | Ratio | vs repomix `--compress`      |
| ------------------------ | --------- | ------------- | ----- | ---------------------------- |
| `charliesbot.dev`        | 1,891     | ~8,000        | 4.2×  | still **2.3× smaller** (18k) |
| `one` (Kotlin)           | 27,168    | ~55,000       | 2.0×  | still **2.5× smaller** (137k) |
| `advent-of-code` (multi) | 5,824     | ~20,000       | 3.4×  | still **12.8× smaller** (257k) |

Numbers are directional — actual depends on file sizes of ranked files. The pattern holds: `--deep` stays well under repomix because ranking prunes aggressively.

#### Why it matters (when and for whom)

**Who:** agents that finish orientation and immediately need to read what the map surfaced. Any "where does auth live, and show me that file" workflow.

**When to reach for it:**

- Session start on an unfamiliar repo, wanting both the map and key file contents in one invocation.
- Agent loops that want to avoid per-file `Read` roundtrips.
- Chat-style workflows where you paste one thing into context and ask questions.

**When not:**

- Agents doing targeted work on known files — better to `tingle` for orientation then `Read` the 1-2 files they care about.
- Full-repo review / cross-cutting refactors — use repomix; tingle's ranking doesn't cover "every file."

#### Implementation scope

**Estimated LOC:** ~80-120.

**Where code changes land:**

- `cmd/tingle/main.go` — parse `--deep`, `--deep-max-files`, `--deep-max-bytes` flags. Pass options through to render.
- `internal/render/render.go` — new path in render: when `Options.Deep == true`, collect the set of files that got `EP` or qualifying `U` records, read their bodies (bounded by `--deep-max-bytes`), emit fenced code blocks inline after the record.
- `internal/render/render_test.go` (new) — golden-file test asserting `--deep` output has fenced code blocks under ranked records, no body for unranked F records, byte-cap respected.

**What doesn't change:**

- `internal/enumerate`, `internal/parse`, `internal/resolve`, `internal/rank`, `internal/manifest` — zero changes. Same pipeline.
- Output format prefix structure (`## Entry points`, `U`, `F`, etc.) — unchanged. Just additional fenced blocks appear under records.
- Backwards compatibility: default behavior (no flag) is identical to current v1.

**Pros:**

- One binary, one install, one invocation. Matches the minimalist identity.
- Ranking signal survives into the output — the agent sees "here's the most important file + its body" in a single structured block, not "here are files + here are contents" as two disjoint things.
- Token budget stays bounded by ranking, not by repo size. Small-to-mid repos: 20-55k tokens at `--deep`, still cheaper than repomix.
- Backwards compatible — default behavior unchanged.

**Cons:**

- Adds a mode. Two behaviors to document, test, and maintain forever. Slight violation of "do one thing well."
- Opens a design surface (which files qualify, how to truncate large ones, secret redaction, etc.) — each decision is a small debate that wouldn't exist in Proposal A.
- Body emission duplicates a capability repomix already has. Not wrong, just not unique.

**When this is the right choice:**

For a solo developer / small-team target, having one binary that handles both orient and analyze-ranked-files is meaningfully more ergonomic than requiring a two-tool pipeline. The ranking-preserves-in-output property is the specific thing neither repomix nor Proposal A can give you.

---

### Proposal C — full packer mode

Add a `--pack` flag that makes tingle emit every file with full bodies, wrapped in structured delimiters. Essentially build a repomix replacement into tingle.

**Output shape:** whole repo, every file, full contents, tagged by path. Similar to repomix's default output but in our own format.

**Pros:**

- Single tool for literally every LLM-facing codebase packaging need.
- No dependency on repomix's maintenance, version cadence, or install story.
- Users who prefer tingle's format get a complete product.

**Cons:**

- ~200+ LOC for the packer itself. Doubles tingle's scope.
- Two modes with orthogonal concerns (orient vs pack) will diverge in design over time — what's the right default for each, what flags apply to both, how do warnings compose, etc.
- Directly duplicates a mature tool (23k⭐ repomix) that already solves this well and has a corresponding ecosystem (browser ext, web UI, VS Code extension we'd never match).
- Abandons the "do one thing well" principle. Violates the stated project philosophy.
- Doesn't actually add capability beyond what `tingle --list-ranked | repomix` composes for free.

**When this would be the right choice:**

If we were building a commercial or team-scale tool and needed to own the full packaging stack end-to-end. For a side-project orienter, it's scope creep with no net user value.

---

### Decision summary

| Criterion                         | Proposal A (pipe) | Proposal B (`--deep`) `[PREFERRED]` | Proposal C (`--pack`) |
| --------------------------------- | ----------------- | ------------------------------------ | --------------------- |
| Implementation cost               | ~20 LOC           | ~100 LOC                             | ~200+ LOC             |
| Preserves ranking in output       | ❌                | ✅                                   | ❌                    |
| One binary install                | ❌                | ✅                                   | ✅                    |
| Matches "do one thing well"       | ✅                | ◐                                    | ❌                    |
| Complements repomix               | ✅                | ✅                                   | ❌ (competes)         |
| Ergonomics for solo dev           | ◐                | ✅                                   | ✅                    |

Proposal B is preferred because it preserves the ranking structure through to the output — the actual differentiator that neither A nor C offers — while keeping tingle a single binary. Proposal A is the defensible fallback if we later decide the mode surface is too much to carry.

---

## Open questions (for when the preferred proposal is picked up)

1. **Body format:** fenced Markdown (triple backticks with language hint) or a tagged record form (`B <path>` followed by content lines)? Fenced is human-readable and widely recognized; tagged is more parseable by automation. Lean fenced for agent-first consumption.
2. **Truncation strategy:** when a file exceeds `--deep-max-bytes`, truncate to the first N bytes with a `… [truncated, use Read(path, offset=N) for more]` marker? Or skip the body entirely and note `[body omitted: file too large]`? Former is more useful.
3. **Which utilities qualify?** `in >= 3` is a guess. Maybe `in >= max(3, median(in))`? Wait for real-use feedback before tuning.
4. **Does `--max-depth` / `--expand` interact?** If those flags are implemented (deferred in current v1), does `--deep` respect them? Probably yes — if a dir is collapsed, its files don't get bodies either.
5. **Secret redaction.** If we emit full bodies, any hardcoded secrets in those files go straight into the output. Current tingle is signature-only so this is less of an issue. `--deep` makes it a real concern — need a token-level regex sweep before emission, or a `--redact` flag that enables it.

---

## Decision record

**Current status:** deferred. v1 ships as pure orienter. Real-world use will tell us whether the orient → Read roundtrip is actually friction worth eliminating, or whether agents do fine without this.

**Triggers to pick this up:**

- You notice yourself (or an agent you built) running `tingle` and then immediately running `Read` on 3-5 of the ranked files. That loop is what `--deep` eliminates.
- A concrete user — even yourself — says "I want tingle to give me the code, not just the signatures."
- repomix ecosystem starts overtaking tingle in agent workflows specifically because agents want bodies along with orientation.

**Triggers to drop this idea:**

- Real agent workflows use tingle for orientation and don't follow up with bulk `Read`s — they use signature info to navigate the code semantically, not by reading whole files.
- LLM context windows grow further and repomix-style full packs become default.

**If the preferred proposal is dropped, the fallback is Proposal A** — `--list-ranked` composed with repomix via pipe. It preserves the key capability (ranked file selection for analysis) without any of the mode-surface downsides.

---

## What this doc deliberately does NOT cover

- Any new language support or extraction work — orthogonal.
- Release + distribution of v1 — separate concern; see `implementation.md`.
- The `--list-ranked` flag spec for Proposal A — only sketched. If A is ever picked up, that flag's design surface would be this doc's responsibility to expand.
