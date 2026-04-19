# v2 — Rust Rewrite (Future Phase)

**Status:** Deferred. Captured for future consideration; v1 ships in Go and stays in Go until a concrete trigger justifies the rewrite cost.
**Author:** charliesbot
**Date:** 2026-04-18

---

## Why this doc exists

v1 runs on Go + [`gotreesitter`](https://github.com/odvcencio/gotreesitter) — a pure-Go reimplementation of the tree-sitter runtime. The migration from cgo gave us trivial cross-compile (`CGO_ENABLED=0 GOOS=linux go build` just works), but two real costs surfaced during testing:

1. **Grammar reimplementation bugs** — gotreesitter's Kotlin grammar doesn't capture `object Foo { ... }` declarations as classes; its Python grammar silently drops the top-level `def` that follows a class with an f-string method body. Both documented in [`design-doc.md § Known gaps`](design-doc.md#known-gaps--risks) and guarded by [`internal/parse/parse_test.go`](../internal/parse/parse_test.go) regression tests.
2. **Performance tax** — gotreesitter's pure-Go runtime is ~18× slower per-parse than native C tree-sitter (11 ms → 204 ms on 167 Kotlin files) and takes ~36× more memory at peak (20 MB → 715 MB before tuning, 167 MB after). Still within target for our scope, but we burned most of our headroom.

These costs are the price we paid for zero cgo. Rust would give us back most of what we gave up, without reintroducing the cgo cross-compile problem. This doc captures why and when.

---

## Non-goals

- **Not a rewrite-for-rewrite's-sake proposal.** Migration only makes sense if concrete v1 pain justifies the cost.
- **Not a Rust evangelism doc.** Go was a fine first choice; gotreesitter was a reasonable second choice given the constraints. This doc is about what the *third* choice would look like.
- **Not a full spec.** High-level plan only; detailed design happens if/when the rewrite is triggered.

---

## Why Rust, specifically

### The tree-sitter ecosystem in Rust is first-class

- `tree-sitter` crate is the canonical binding to the native C runtime. Exposes idiomatic `Parser`, `Tree`, `Query`, `QueryCursor`, `Node` types. Zero reimplementation, zero bugs-we-don't-share-with-the-rest-of-the-ecosystem.
- Every grammar ships as a `tree-sitter-<lang>` crate: `tree-sitter-typescript`, `tree-sitter-kotlin`, `tree-sitter-vue`, `tree-sitter-svelte`, `tree-sitter-astro`, `tree-sitter-markdown`. Adding a language = `cargo add tree-sitter-vue` + one entry in the language registry.
- Grammar maintenance is distributed across the ecosystem. When Kotlin's grammar gets a fix upstream, we bump the crate and get it.

### Cross-compile works

Rust's target support is wide. `cargo build --release --target x86_64-unknown-linux-gnu` builds a Linux binary from macOS. For a more hands-off experience, the [`cross`](https://github.com/cross-rs/cross) tool runs target-specific build containers — produces clean binaries for 50+ targets without dealing with toolchains yourself.

Not quite as trivial as pure Go's `GOOS=linux go build`, but vastly better than cgo cross-compile, and well-established enough that every Rust release pipeline does it routinely.

### Performance comes back

With native tree-sitter via Rust bindings, we're back to the original cgo numbers:

| Metric | Go + smacker (cgo) | Go + gotreesitter (current) | Rust + tree-sitter |
|---|---|---|---|
| Parse time (167 Kotlin files) | 11 ms | 204 ms | ~11 ms (same C runtime) |
| Peak RSS | 20 MB | 167 MB (after tuning) | ~20 MB |
| Binary size (stripped) | 13 MB | 22 MB | ~15-25 MB |
| Cross-compile | Painful (cgo) | Trivial | Good (`cross` / `--target`) |
| Grammar bugs | None (canonical C) | Two known | None (canonical C) |

Rust gives us back the perf we lost without reintroducing the cgo cross-compile problem that made us migrate in the first place.

### Developer experience is solid

- Strong static types, excellent error messages, mature tooling (`cargo`, `rustfmt`, `clippy`, `rust-analyzer`).
- The ownership model maps well to tree-sitter's lifetime semantics — no manual `Release()` calls, no sync.Pool-and-remember-to-Put dance. Trees drop when they go out of scope.
- Excellent serde support makes JSON output (if we ever add `--json`) a one-liner.
- Error handling via `Result<T, E>` + `?` is ergonomic for our pipeline.

### What we give up

- **Compile times.** Rust is noticeably slower to build than Go. `cargo build` on a cold cache takes a few minutes; incremental builds are fast but not as fast as Go's. For a small CLI this hurts developer iteration but doesn't affect shipped users.
- **Learning curve (if you don't already know Rust).** Ownership, borrow checker, lifetimes. Proficient Go developers can get productive in a few weeks, but there's a ramp.
- **Binary size.** Similar to or slightly larger than Go. Not a meaningful difference.
- **All the v1 Go code.** Enumerate, parse, resolve, rank, render, manifest packages — probably 1500-2000 LOC to port. Tests to port too.

---

## Scope estimate

Rough breakdown if we pulled the trigger today:

| Task | Estimated effort |
|---|---|
| Project scaffold (`cargo new`, module layout, CI) | half day |
| Port `enumerate` (git + WalkDir fallback + `.tingleignore`) | half day |
| Port `parse` (tree-sitter + query execution) | 1-2 days |
| Port `resolve` (path math + `--alias`) | half day |
| Port `rank` (graph, entry-point + utility scoring) | half day |
| Port `manifest` (package.json + go.mod parsing) | half day |
| Port `render` (compact tag-prefixed output) | 1 day |
| Port query files (`.scm`) + augmentations | 1 day (mostly copy-paste) |
| Tests (port existing + add new) | 1-2 days |
| Cross-compile CI via `cross` or GitHub Actions matrix | half day |
| Release pipeline (GitHub Releases, brew tap) | 1 day |

**Total: ~7-10 focused days** for someone proficient in Rust. Add 2-4 days ramp if not.

---

## When to pull the trigger

Concrete triggers ordered by likelihood:

1. **gotreesitter bugs hit real workflows.** Object-declaration gaps or f-string quirks surface often enough that users complain or you work around them every session.
2. **Need a grammar gotreesitter doesn't have or mis-implements.** E.g., Vue/Angular/Svelte for a real project, and gotreesitter's version is flaky.
3. **Performance starts mattering.** The 18× per-parse slowdown compounds as repos grow. On a 2k-file repo this is ~2.5s at the edge of the `<2s` gate. If you ever want to ship tingle for larger codebases, Rust buys you back the perf.
4. **You want a polished v2 release.** For a tool you want people to actually adopt (brew install, Homebrew tap, public distribution), having the canonical tree-sitter under the hood is a quality signal that matters.
5. **You're motivated to learn Rust.** Not strictly a trigger, but a legitimate side-project reason.

### When NOT to migrate

- v1 is in active daily use, bugs aren't biting, nobody's asking for Vue.
- You have other priorities — Kotlin FQCN resolution, test coverage, the release itself.
- You're building something else and tingle is in maintenance mode.

---

## Migration strategy (if it ever happens)

Not a full rewrite in one sitting. Incremental:

1. **Scaffold a Rust project alongside the Go one.** Same repo, separate `rust/` directory. Matching CLI surface.
2. **Port enumerate first** — simplest package, validates the overall structure.
3. **Port parse + queries** — the hard part. Use Rust's `tree-sitter` + per-language crates. Port the `.scm` query files unchanged (they're language-agnostic data).
4. **Port resolve, rank, manifest, render** — straightforward translations.
5. **Match v1 output byte-for-byte on the three test repos.** The Go binary's output is the oracle.
6. **When Rust binary passes the golden-file tests, retire the Go version.** Rename directories, update README, tag `v1.0.0` (the first "real" release).

Parallel operation for a short period means you can A/B compare easily. Both binaries, same repo, diff the outputs. Any divergence is a bug in the new code.

---

## Decision record

**Current status:** deferred. v1 runs on Go + gotreesitter. Cross-compile works, known gaps are bounded, tool is usable today.

**If a trigger fires (see above):** start Phase 1 of the incremental migration. Don't rewrite everything at once.

**If no trigger fires:** keep maintaining v1 in Go. File upstream issues with gotreesitter for the two known bugs. Revisit this doc in 6-12 months.

---

## What this doc deliberately does NOT cover

- Zig or C as alternatives — both analyzed and rejected in the design discussion leading to this doc. Zig has better cross-compile but less mature tree-sitter bindings; C is too low-level for productivity at our scope.
- Specific crate choices beyond `tree-sitter` and `tree-sitter-<lang>` — defer to implementation time.
- Deep mode (`--deep` flag) design — orthogonal. See [`deep-mode.md`](deep-mode.md). If deep mode ships in v2 Rust, the design carries over unchanged.
- Full-repo packing (Option C from the deep-mode alternatives) — out of scope here too.
- Release signing / notarization for macOS — real concern for distribution but orthogonal to language choice. Same problem applies regardless of Go or Rust.
