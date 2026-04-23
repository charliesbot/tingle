//! Compact tag-prefixed rendering. Output is AI-first (token-efficient),
//! not human-first — we optimize for the rate in bits-per-agent-answer,
//! not for casual readability.
//!
//! Section order: Manifests, Entry points, Utilities, Modules, Files.
//! Empty sections are omitted. The legend line is context-aware — it only
//! mentions prefix/tag categories that actually appear in THIS run.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;

use crate::lang::jvm::{self, compact_label_path};
use crate::model::{FileIndex, Symbol};

#[derive(Default, Clone)]
pub struct Options {
    pub version: String,
    pub commit: String,
    pub tokenizer_id: String,
    pub no_legend: bool,
    pub tokens_approx: u32,
    /// ISO date (UTC) used in the `gen=...` header.
    pub gen_date: String,
    /// `--scope <PATH>`: filter the `## Files` section to paths under this
    /// prefix. Top sections still render whole-repo context. Empty = no
    /// filter.
    pub scope: String,
    /// `--full`: include per-file def listings in the `## Files` section
    /// AND show up to 3 callers per Utility record.
    ///
    /// Default is the compact layout: F records list paths/imports/tags
    /// only and U records show 1 caller. Eval (`evals/run.sh` × 3 real
    /// repos) showed the compact layout preserves agent task quality
    /// (≥0.97 mean score) while saving 47-58% of tokens vs `--full`.
    pub full: bool,
    /// Suppress the soft token warning. When the caller is writing to a
    /// file (the default), there's no preview to overflow, so the
    /// "consider --scope / pipe to file" advice is moot and just burns
    /// tokens. Stdout mode keeps the warning.
    pub suppress_warning: bool,
}

/// Token count above which the header includes a shrink-suggestion line.
/// Char/4 approximation of cl100k_base.
///
/// 8k chosen because that's roughly where agent CLI tool-result previews
/// start to truncate (~30-40KB of inline output). The warning's
/// actionable hint — pipe to a file the agent can Read, or shrink with
/// --scope — is what agents need at THIS size, not at 20k where the
/// output is already unrecoverable in many environments. Small repos
/// (<8k tokens fit comfortably anywhere) don't see it.
const TOKEN_WARN_THRESHOLD: usize = 8_000;

pub fn render(
    files: &[FileIndex],
    entries: &[&FileIndex],
    utilities: &[&FileIndex],
    dir_edges: &HashMap<String, Vec<String>>,
    callers: &HashMap<String, Vec<String>>,
    manifests: &[String],
    opts: &Options,
) -> String {
    // Two-pass: build body first so we can measure its token footprint,
    // then prepend a header that references the measurement. Stateless
    // and cheap at tingle's scale.
    let body = build_body(
        files, entries, utilities, dir_edges, callers, manifests, opts,
    );
    let mut out = String::new();
    write_header(
        &mut out, &body, files, entries, utilities, dir_edges, manifests, opts,
    );
    out.push_str(&body);
    out
}

#[allow(clippy::too_many_arguments)]
fn write_header(
    out: &mut String,
    body: &str,
    files: &[FileIndex],
    entries: &[&FileIndex],
    utilities: &[&FileIndex],
    dir_edges: &HashMap<String, Vec<String>>,
    manifests: &[String],
    opts: &Options,
) {
    let ver = if opts.version.is_empty() {
        "v0"
    } else {
        opts.version.as_str()
    };
    let commit = if opts.commit.is_empty() {
        String::new()
    } else {
        format!("  commit={}", opts.commit)
    };
    let tknzr = if opts.tokenizer_id.is_empty() {
        "cl100k_base"
    } else {
        opts.tokenizer_id.as_str()
    };
    writeln!(
        out,
        "# tingle {}  gen={}{}  files={}  tokenizer={}",
        ver,
        opts.gen_date,
        commit,
        count_parsed(files),
        tknzr,
    )
    .unwrap();

    if !opts.no_legend {
        out.push_str(&build_legend(
            entries, utilities, dir_edges, manifests, files, opts,
        ));
        out.push('\n');
    }

    // Soft token warning — char/4 is a rough cl100k_base approximation.
    // Skipped when writing to a file (no preview to exceed).
    let approx_tokens = (body.len() + out.len()) / 4;
    if !opts.suppress_warning && approx_tokens > TOKEN_WARN_THRESHOLD {
        // Warning emits before the body, so it survives any truncation
        // by the agent's tool-result preview. Lists actions in order of
        // most-common-fix-first: agents keep independently inventing
        // the file-redirect workaround, so put it first.
        //
        // Don't hardcode `/tmp/...` — in container/sandbox setups the
        // tingle process and the agent process can sit in different
        // filesystem namespaces, and /tmp inside the container isn't
        // reachable by the outside agent. The agent picks a path it
        // can read back.
        // Both workarounds are lossless — file redirect keeps everything,
        // --scope keeps everything for a subtree.
        writeln!(
            out,
            "# warning: ~{}k tokens — exceeds many agent previews. Pipe to a file your agent can Read (e.g. `tingle ... > out.md`) or zoom in with --scope PATH.",
            approx_tokens / 1000
        )
        .unwrap();
    }

    out.push('\n');
}

/// Context-aware legend — only lists marker categories that will actually
/// appear in this run's output. Prevents the "legend over-promises" UX bug
/// where agents see `S=manifest` in the legend but no Manifests section
/// renders.
fn build_legend(
    entries: &[&FileIndex],
    utilities: &[&FileIndex],
    dir_edges: &HashMap<String, Vec<String>>,
    manifests: &[String],
    files: &[FileIndex],
    opts: &Options,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Section prefixes.
    let mut sections: Vec<&str> = Vec::new();
    if !manifests.is_empty() {
        sections.push("S=manifest");
    }
    if !entries.is_empty() {
        // Defines out=/in= here so agents don't have to guess the
        // semantics from context (feedback: legend was over-promising
        // section markers and under-explaining numeric ones).
        sections.push("EP=entry(out=imports-out,in=imports-in)");
    }
    if !utilities.is_empty() {
        sections.push("U=utility(in=fan-in)");
    }
    if !dir_edges.is_empty() {
        // `src -> dst` reads as "src imports from dst" — the arrow points
        // from dependent to dependency. Legend calls this out explicitly
        // so agents don't infer the opposite convention.
        sections.push("M=module-edge(src->dst=src-imports-dst)");
    }
    let files_rendered = files
        .iter()
        .any(|f| !f.lang.is_empty() || !f.tags.is_empty());
    if files_rendered {
        // `(N)` after the path is LOC for files tingle parses — surfaces
        // outsized files (common refactor targets) at a glance.
        let any_loc = files.iter().any(|f| f.loc > 0);
        if any_loc {
            sections.push("F=file(N=loc)");
        } else {
            sections.push("F=file");
        }
    }
    if !sections.is_empty() {
        parts.push(sections.join(" "));
    }

    // Tag categories — only if the Files section is rendered AND files in
    // it carry those tags.
    if files_rendered {
        let scope = opts.scope.trim_start_matches("./").trim_end_matches('/');
        let visible = files.iter().filter(|f| {
            (!f.lang.is_empty() || !f.tags.is_empty())
                && (scope.is_empty()
                    || f.path == scope
                    || f.path.starts_with(&format!("{}/", scope)))
        });
        let mut tag_parts: Vec<&str> = Vec::new();
        let files_vec: Vec<&FileIndex> = visible.collect();
        if files_vec.iter().any(|f| f.tags.iter().any(|t| t == "M")) {
            tag_parts.push("[M]=modified");
        }
        if files_vec
            .iter()
            .any(|f| f.tags.iter().any(|t| t == "untracked"))
        {
            tag_parts.push("[untracked]=new-unstaged");
        }
        if files_vec.iter().any(|f| f.tags.iter().any(|t| t == "test")) {
            tag_parts.push("[test]=test-file");
        }
        if !tag_parts.is_empty() {
            parts.push(tag_parts.join(" "));
        }
    }

    // [hub] marker — only emitted when at least one EP record will carry
    // it (i.e., a file appears in both EP and U).
    let hub_present = !entries.is_empty()
        && utilities
            .iter()
            .any(|u| entries.iter().any(|e| e.path == u.path));
    if hub_present {
        parts.push("[hub]=both-entry-and-utility".to_string());
    }

    // [orphan] marker — only emitted when at least one F record will
    // carry it. Same scope-aware visibility check the tag categories use.
    if files_rendered {
        let scope = opts.scope.trim_start_matches("./").trim_end_matches('/');
        let entry_paths: std::collections::HashSet<&str> =
            entries.iter().map(|e| e.path.as_str()).collect();
        let kotlin_peers = jvm::kotlin_packages_with_peers(files);
        let any_orphan = files.iter().any(|f| {
            (!f.lang.is_empty() || !f.tags.is_empty())
                && (scope.is_empty()
                    || f.path == scope
                    || f.path.starts_with(&format!("{}/", scope)))
                && f.in_deg == 0
                && !f.defs.is_empty()
                && !entry_paths.contains(f.path.as_str())
                && !f.tags.iter().any(|t| t == "test")
                && !kotlin_peers.contains(&f.package)
        });
        if any_orphan {
            // Honest framing — tingle reads static imports only, so files
            // wired through runtime registration (DI, manifests, reflection,
            // platform-callable Services/Activities/Workers) look orphan
            // even when they're not. The tag flags "no inbound import
            // edge"; the agent verifies whether that means dead code.
            parts.push("[orphan]=no-import-callers(may-be-runtime-registered)".to_string());
        }
    }

    // Def-kind markers — only if the F section will actually render defs.
    // Utilities no longer emit inline defs (they'd duplicate F section
    // content). `--compact` drops F-section defs too. In both cases the
    // def-kinds legend would advertise markers that never appear, which
    // is the exact UX bug this section was designed to prevent.
    let has_defs = opts.full && files_rendered && files.iter().any(|f| !f.defs.is_empty());
    if has_defs {
        let mut kinds: Vec<&str> = Vec::new();
        let iter_defs = || files.iter().flat_map(|f| f.defs.iter());
        let has_kind = |k: &str| iter_defs().any(|d| d.kind.as_str() == k);
        if has_kind("f") {
            kinds.push("f=func");
        }
        if has_kind("c") {
            kinds.push("c=class");
        }
        if has_kind("m") {
            kinds.push("m=method");
        }
        if has_kind("i") {
            kinds.push("i=interface");
        }
        if has_kind("t") {
            kinds.push("t=type");
        }
        if has_kind("e") {
            kinds.push("e=enum");
        }
        if !kinds.is_empty() {
            parts.push(format!("[path:line]=def {}", kinds.join(" ")));
        }
    }

    if parts.is_empty() {
        "# legend:".to_string()
    } else {
        format!("# legend: {}", parts.join("  "))
    }
}

fn build_body(
    files: &[FileIndex],
    entries: &[&FileIndex],
    utilities: &[&FileIndex],
    dir_edges: &HashMap<String, Vec<String>>,
    callers: &HashMap<String, Vec<String>>,
    manifests: &[String],
    opts: &Options,
) -> String {
    let mut b = String::new();

    // Manifests
    if !manifests.is_empty() {
        b.push_str("## Manifests\n");
        for m in manifests {
            b.push_str(m);
            b.push('\n');
        }
        b.push('\n');
    }

    // Entry points. EP records that ALSO qualify as utilities (file is
    // both heavily importing AND heavily imported) get an inline `[hub]`
    // tag. These are orchestrator/manager files whose role doesn't fit
    // cleanly as either entry or utility — surfacing the duality saves
    // the agent from having to compare numbers across sections.
    if !entries.is_empty() {
        let utility_paths: std::collections::HashSet<&str> =
            utilities.iter().map(|u| u.path.as_str()).collect();
        b.push_str("## Entry points\n");
        for f in entries {
            let name = first_def_name(f);
            let line = first_def_line(f);
            let hub = if utility_paths.contains(f.path.as_str()) {
                " [hub]"
            } else {
                ""
            };
            let loc_str = if f.loc > 0 {
                format!(" loc={}", f.loc)
            } else {
                String::new()
            };
            writeln!(
                b,
                "EP {}:{} {} (out={} in={}{}){}",
                f.path, line, name, f.out_deg, f.in_deg, loc_str, hub
            )
            .unwrap();
        }
        b.push('\n');
    }

    // Utilities — U records. Inline defs are NOT repeated here: they also
    // appear in the F section below, so the duplicate burns tokens with
    // zero signal gain. The U record carries path + in_deg + top callers,
    // which is what uniquely surfaces "load-bearing file."
    if !utilities.is_empty() {
        b.push_str("## Utilities\n");
        for f in utilities {
            let empty: Vec<String> = Vec::new();
            let cs = callers.get(&f.path).unwrap_or(&empty);
            let caller_str = if cs.is_empty() {
                String::new()
            } else {
                // Default shows 1 caller; --full opens up to 3.
                // Caller paths are architecture labels (the utility itself
                // is the anchor) — compact Gradle boilerplate for tokens.
                let cap = if opts.full { 3 } else { 1 };
                let max_show = cap.min(cs.len());
                let short: Vec<String> = cs[..max_show]
                    .iter()
                    .map(|c| compact_label_path(c))
                    .collect();
                let mut s = format!("  ← {}", short.join(" "));
                if cs.len() > max_show {
                    s.push_str(&format!(" (+{} more)", cs.len() - max_show));
                }
                s
            };
            let loc_str = if f.loc > 0 {
                format!(" loc={}", f.loc)
            } else {
                String::new()
            };
            writeln!(b, "U {} (in={}{}){}", f.path, f.in_deg, loc_str, caller_str).unwrap();
        }
        b.push('\n');
    }

    // Modules. Dirs here are architecture *labels*, never anchors — the
    // agent never Reads a directory — so strip Gradle source-root
    // boilerplate for token efficiency.
    //
    // Dedup by *compacted* form: when a module has both `src/main/java/...`
    // and `src/main/kotlin/...` source roots, the raw graph holds them as
    // separate src dirs that compact to the same label (e.g.,
    // `core/domain/usecase`). Without merging, the M section repeats the
    // same logical edge across multiple lines — a real bug flagged by two
    // agents independently.
    if !dir_edges.is_empty() {
        let mut merged: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (raw_src, raw_dsts) in dir_edges {
            let src_label = compact_label_path(raw_src);
            let entry = merged.entry(src_label).or_default();
            for d in raw_dsts {
                entry.insert(compact_label_path(d));
            }
        }
        // Drop self-edges that may appear after compaction (same module
        // referencing itself once source-set boundaries collapse).
        for (src, dsts) in merged.iter_mut() {
            dsts.remove(src);
        }
        merged.retain(|_, dsts| !dsts.is_empty());

        if !merged.is_empty() {
            b.push_str("## Modules\n");
            for (src, dsts) in &merged {
                let dst_str: Vec<&str> = dsts.iter().map(|s| s.as_str()).collect();
                writeln!(b, "M {} -> {}", src, dst_str.join(" ")).unwrap();
            }
            b.push('\n');
        }
    }

    // Files
    let scope = opts.scope.trim_start_matches("./").trim_end_matches('/');
    let mut visible: Vec<&FileIndex> = files
        .iter()
        .filter(|f| !f.lang.is_empty() || !f.tags.is_empty())
        .filter(|f| {
            scope.is_empty() || f.path == scope || f.path.starts_with(&format!("{}/", scope))
        })
        .collect();
    if visible.is_empty() {
        return b;
    }
    visible.sort_by(|a, b| a.path.cmp(&b.path));

    b.push_str("## Files\n");

    // Orphan = file with defs that nothing imports AND isn't an entry point
    // AND isn't a test. Usually dead code worth deleting; cheap to surface,
    // high signal for cleanup workflows.
    //
    // Kotlin exception: files in a package with ≥2 members are suppressed.
    // Same-package peers can call a declaration without an `import`, and
    // tingle's syntactic capture misses several routing paths (extension
    // fns, reflection, manifest/DI wiring). Flagging these as orphans was
    // the single biggest false-positive source on Android/Kotlin repos —
    // better to stay silent than make a wrong claim.
    let entry_paths: std::collections::HashSet<&str> =
        entries.iter().map(|e| e.path.as_str()).collect();
    let kotlin_peers = jvm::kotlin_packages_with_peers(files);
    let orphan_paths: std::collections::HashSet<&str> = visible
        .iter()
        .filter(|f| {
            f.in_deg == 0
                && !f.defs.is_empty()
                && !entry_paths.contains(f.path.as_str())
                && !f.tags.iter().any(|t| t == "test")
                && !kotlin_peers.contains(&f.package)
        })
        .map(|f| f.path.as_str())
        .collect();

    // Module-grouped F section: group by parent directory, emit `###` per
    // group, render children with basename only. Collapses repeated path
    // prefixes (e.g. `core/src/main/java/com/charliesbot/shared/core/...`)
    // that dominate byte cost on Android/Kotlin repos.
    let mut groups: BTreeMap<String, Vec<&FileIndex>> = BTreeMap::new();
    for f in &visible {
        groups.entry(parent_dir(&f.path)).or_default().push(f);
    }

    for (dir, children) in &groups {
        if dir.is_empty() {
            // Repo-root files: no header, render full path.
            for f in children {
                write_file_line(&mut b, f, &f.path, opts, &orphan_paths);
            }
        } else if children.len() == 1 {
            // Singleton group: the `### <dir>` header costs more than it
            // saves. Render the lone file with its full path, no header.
            let f = children[0];
            write_file_line(&mut b, f, &f.path, opts, &orphan_paths);
        } else {
            writeln!(b, "### {}", dir).unwrap();
            for f in children {
                let name = basename(&f.path);
                write_file_line(&mut b, f, name, opts, &orphan_paths);
            }
        }
    }

    b
}

fn write_file_line(
    b: &mut String,
    f: &FileIndex,
    display_name: &str,
    opts: &Options,
    orphan_paths: &std::collections::HashSet<&str>,
) {
    let mut tag_str = String::new();
    for t in &f.tags {
        tag_str.push('[');
        tag_str.push_str(t);
        tag_str.push(']');
    }
    if orphan_paths.contains(f.path.as_str()) {
        tag_str.push_str("[orphan]");
    }
    // LOC marker — surfaces outsized files at a glance (reviewer asked for
    // this explicitly: FeedRepositoryImpl at 378, FeedViewModel at 336).
    // Omitted when we didn't read the file (unsupported extension, loc=0).
    let loc_str = if f.loc > 0 {
        format!(" ({})", f.loc)
    } else {
        String::new()
    };
    // Cap the imports list with overflow notation. Mirrors the U-record
    // caller pattern (`(+N more)`). DI-heavy / aggregate files like
    // `AppModule.kt` carry 20+ imports inline — capping at 10 keeps the
    // F-line scannable without losing the "yes, this file aggregates a
    // lot" signal.
    const IMPORTS_CAP: usize = 10;
    let imps = if f.imports.is_empty() {
        String::new()
    } else if f.imports.len() <= IMPORTS_CAP {
        format!("  imp: {}", f.imports.join(" "))
    } else {
        format!(
            "  imp: {} (+{} more)",
            f.imports[..IMPORTS_CAP].join(" "),
            f.imports.len() - IMPORTS_CAP
        )
    };
    writeln!(b, "F {}{} {}{}", display_name, loc_str, tag_str, imps).unwrap();
    if opts.full {
        write_defs(b, &f.defs);
    }
}

fn write_defs(b: &mut String, defs: &[Symbol]) {
    for d in defs {
        writeln!(b, " {} {} {}", d.line, d.kind.as_str(), d.signature).unwrap();
        for m in &d.children {
            writeln!(b, "  {} {} {}", m.line, m.kind.as_str(), m.signature).unwrap();
        }
    }
}

/// Pick the def whose name matches the file's basename-without-extension
/// (Kotlin/Java/C# convention: file `SettingsViewModel.kt` declares class
/// `SettingsViewModel`). Fallback to the first def. This makes the EP
/// label match the file's actual identity instead of whatever happens to
/// declare first (a sibling data class, a private helper, etc.).
fn primary_def(f: &FileIndex) -> Option<&Symbol> {
    let stem = file_stem(&f.path);
    f.defs
        .iter()
        .find(|d| d.name == stem)
        .or_else(|| f.defs.first())
}

fn first_def_name(f: &FileIndex) -> String {
    match primary_def(f) {
        Some(d) => d.name.clone(),
        None => basename(&f.path).to_string(),
    }
}

fn first_def_line(f: &FileIndex) -> u32 {
    primary_def(f).map(|d| d.line).unwrap_or(1)
}

fn file_stem(p: &str) -> &str {
    let base = basename(p);
    match base.rfind('.') {
        Some(i) if i > 0 => &base[..i],
        _ => base,
    }
}

fn basename(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

fn parent_dir(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => p[..i].to_string(),
        None => String::new(),
    }
}

fn count_parsed(files: &[FileIndex]) -> usize {
    files.iter().filter(|f| !f.lang.is_empty()).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Symbol, SymbolKind};

    fn make_def(name: &str, line: u32, kind: SymbolKind) -> Symbol {
        Symbol {
            name: name.into(),
            kind,
            signature: format!("{} ()", name),
            line,
            children: Vec::new(),
        }
    }

    fn opts_minimal() -> Options {
        Options {
            version: "v0".into(),
            gen_date: "2026-04-19".into(),
            no_legend: true,
            ..Default::default()
        }
    }

    #[test]
    fn empty_repo_emits_header_and_no_files_section() {
        let opts = opts_minimal();
        let out = render(&[], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        assert!(out.starts_with("# tingle v0  gen=2026-04-19  files=0  tokenizer=cl100k_base\n"));
        // Nothing visible to file-filter → no `## Files` header at all.
        assert!(!out.contains("## Files"));
    }

    #[test]
    fn files_section_grouped_by_parent_dir() {
        let files = vec![
            FileIndex {
                path: "src/a.ts".into(),
                lang: "ts".into(),
                ..Default::default()
            },
            FileIndex {
                path: "src/b.ts".into(),
                lang: "ts".into(),
                ..Default::default()
            },
            FileIndex {
                path: "src/c.ts".into(),
                lang: "ts".into(),
                ..Default::default()
            },
            FileIndex {
                path: "src/sub/d.ts".into(),
                lang: "ts".into(),
                ..Default::default()
            },
        ];
        let opts = opts_minimal();
        let out = render(
            &files,
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &opts,
        );
        // Multi-file group: header + basenames.
        assert!(out.contains("### src\n"), "{}", out);
        assert!(out.contains("F a.ts "), "{}", out);
        assert!(out.contains("F b.ts "), "{}", out);
        assert!(out.contains("F c.ts "), "{}", out);
        assert!(!out.contains("F src/a.ts"), "{}", out);
        // Singleton group (src/sub has one file): no header, full path.
        // Header would cost more than it saves.
        assert!(!out.contains("### src/sub"), "{}", out);
        assert!(out.contains("F src/sub/d.ts "), "{}", out);
    }

    #[test]
    fn repo_root_files_render_with_full_path_no_header() {
        let f = FileIndex {
            path: "README.md".into(),
            tags: vec!["untracked".into()],
            ..Default::default()
        };
        let opts = opts_minimal();
        let out = render(&[f], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        assert!(out.contains("F README.md [untracked]"), "{}", out);
        assert!(!out.contains("###"), "{}", out);
    }

    #[test]
    fn scope_filters_files_section() {
        let a = FileIndex {
            path: "core/a.ts".into(),
            lang: "ts".into(),
            ..Default::default()
        };
        let b = FileIndex {
            path: "app/b.ts".into(),
            lang: "ts".into(),
            ..Default::default()
        };
        let opts = Options {
            version: "v0".into(),
            gen_date: "2026-04-19".into(),
            no_legend: true,
            scope: "core".into(),
            ..Default::default()
        };
        let out = render(
            &[a, b],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &opts,
        );
        // Singleton after scope filter → full path.
        assert!(out.contains("F core/a.ts "), "{}", out);
        assert!(!out.contains("F app/b.ts"), "{}", out);
    }

    #[test]
    fn default_drops_per_file_defs() {
        // Compact-by-default: F records render path + imports + tags only.
        let f = FileIndex {
            path: "src/a.ts".into(),
            lang: "ts".into(),
            defs: vec![make_def("foo", 5, SymbolKind::Func)],
            ..Default::default()
        };
        let opts = opts_minimal(); // full = false
        let out = render(&[f], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        assert!(out.contains("F src/a.ts "), "{}", out);
        assert!(
            !out.contains(" 5 f foo"),
            "default mode must not emit defs:\n{}",
            out
        );
    }

    #[test]
    fn full_flag_re_emits_per_file_defs() {
        let f = FileIndex {
            path: "src/a.ts".into(),
            lang: "ts".into(),
            defs: vec![make_def("foo", 5, SymbolKind::Func)],
            ..Default::default()
        };
        let opts = Options {
            full: true,
            ..opts_minimal()
        };
        let out = render(&[f], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        assert!(out.contains("F src/a.ts "), "{}", out);
        assert!(out.contains(" 5 f foo"), "--full must emit defs:\n{}", out);
    }

    #[test]
    fn u_records_drop_inline_defs() {
        let util = FileIndex {
            path: "src/util.ts".into(),
            lang: "ts".into(),
            in_deg: 5,
            defs: vec![make_def("helper", 10, SymbolKind::Func)],
            ..Default::default()
        };
        let opts = opts_minimal();
        let mut callers = HashMap::new();
        callers.insert("src/util.ts".into(), vec!["src/a.ts".into()]);
        let files = [util];
        let out = render(
            &files,
            &[],
            &[&files[0]],
            &HashMap::new(),
            &callers,
            &[],
            &opts,
        );
        // U line present.
        assert!(out.contains("U src/util.ts (in=5)"), "{}", out);
        // But no inline def under the U header.
        let u_section_start = out.find("## Utilities").unwrap();
        let u_section_end = out[u_section_start..]
            .find("##")
            .map(|i| u_section_start + i)
            .and_then(|i| out[i + 1..].find("##").map(|j| i + 1 + j))
            .unwrap_or(out.len());
        let u_section = &out[u_section_start..u_section_end];
        assert!(
            !u_section.contains(" 10 f helper"),
            "U section:\n{}",
            u_section
        );
    }

    #[test]
    fn legend_context_aware_drops_absent_categories() {
        let f = FileIndex {
            path: "a.ts".into(),
            lang: "ts".into(),
            ..Default::default()
        };
        let opts = Options {
            version: "v0".into(),
            gen_date: "2026-04-19".into(),
            ..Default::default()
        };
        let out = render(&[f], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        let legend = out
            .lines()
            .find(|l| l.starts_with("# legend:"))
            .expect("legend line present");
        // Only F is visible → no S/EP/U/M in legend, no tag-marker noise.
        assert!(legend.contains("F=file"), "{}", legend);
        assert!(!legend.contains("S=manifest"), "{}", legend);
        assert!(!legend.contains("EP=entry"), "{}", legend);
        assert!(!legend.contains("U=utility"), "{}", legend);
        assert!(!legend.contains("M=module-edge"), "{}", legend);
        assert!(!legend.contains("[M]=modified"), "{}", legend);
    }

    #[test]
    fn soft_token_warning_triggers_on_big_output() {
        // Build >80k chars of content (>20k tokens by char/4 heuristic).
        let mut files: Vec<FileIndex> = Vec::new();
        for i in 0..1200 {
            files.push(FileIndex {
                path: format!("dir{}/file{}.ts", i / 10, i),
                lang: "ts".into(),
                imports: vec![
                    "long-import-name-that-takes-space".into(),
                    "another-long-import".into(),
                    "and-one-more-verbose-import".into(),
                ],
                ..Default::default()
            });
        }
        let opts = opts_minimal();
        let out = render(
            &files,
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &opts,
        );
        assert!(
            out.contains("# warning:"),
            "expected soft warning line, got:\n{}",
            &out[..out.len().min(400)]
        );
    }

    #[test]
    fn ep_label_prefers_def_matching_file_basename() {
        // File is SettingsViewModel.kt but the FIRST def is a sibling data
        // class `SettingsUiState`. Without basename preference, the EP
        // record labels as `SettingsUiState` which is misleading.
        let f = FileIndex {
            path: "src/SettingsViewModel.kt".into(),
            lang: "kt".into(),
            out_deg: 11,
            in_deg: 1,
            defs: vec![
                make_def("SettingsUiState", 28, SymbolKind::Class),
                make_def("SettingsViewModel", 50, SymbolKind::Class),
            ],
            ..Default::default()
        };
        let opts = opts_minimal();
        let files = [f];
        let out = render(
            &files,
            &[&files[0]],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &opts,
        );
        assert!(
            out.contains("EP src/SettingsViewModel.kt:50 SettingsViewModel (out=11 in=1)"),
            "expected basename-matching def to be the EP label, got:\n{}",
            out
        );
    }

    #[test]
    fn ep_label_falls_back_to_first_def_when_no_match() {
        let f = FileIndex {
            path: "src/main.go".into(),
            lang: "go".into(),
            out_deg: 5,
            defs: vec![
                make_def("helper", 5, SymbolKind::Func),
                make_def("main", 10, SymbolKind::Func),
            ],
            ..Default::default()
        };
        let opts = opts_minimal();
        let files = [f];
        let out = render(
            &files,
            &[&files[0]],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &opts,
        );
        // file_stem("main.go") = "main" → matches the second def
        assert!(
            out.contains("EP src/main.go:10 main (out=5 in=0)"),
            "{}",
            out
        );
    }

    #[test]
    fn orphan_files_tagged_in_f_section() {
        let live = FileIndex {
            path: "src/used.ts".into(),
            lang: "ts".into(),
            in_deg: 5,
            defs: vec![make_def("foo", 1, SymbolKind::Func)],
            ..Default::default()
        };
        let dead = FileIndex {
            path: "src/dead.ts".into(),
            lang: "ts".into(),
            in_deg: 0,
            defs: vec![make_def("bar", 1, SymbolKind::Func)],
            ..Default::default()
        };
        let dead_test = FileIndex {
            path: "src/dead.test.ts".into(),
            lang: "ts".into(),
            in_deg: 0,
            tags: vec!["test".into()],
            defs: vec![make_def("baz", 1, SymbolKind::Func)],
            ..Default::default()
        };
        let no_defs = FileIndex {
            path: "src/empty.ts".into(),
            lang: "ts".into(),
            in_deg: 0,
            ..Default::default()
        };
        let opts = opts_minimal();
        let out = render(
            &[live, dead, dead_test, no_defs],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &opts,
        );
        assert!(out.contains("F dead.ts [orphan]"), "{}", out);
        assert!(!out.contains("F used.ts [orphan]"), "{}", out);
        // Tests are NOT orphans (their unused-ness is intentional).
        assert!(!out.contains("F dead.test.ts [orphan]"), "{}", out);
        // Files without defs aren't orphans (we have no code-presence to call dead).
        assert!(!out.contains("F empty.ts [orphan]"), "{}", out);
    }

    #[test]
    fn loc_renders_after_path_when_present() {
        let f = FileIndex {
            path: "src/big.ts".into(),
            lang: "ts".into(),
            loc: 378,
            defs: vec![make_def("foo", 1, SymbolKind::Func)],
            ..Default::default()
        };
        let opts = Options {
            no_legend: false,
            ..opts_minimal()
        };
        let out = render(&[f], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        assert!(out.contains("F src/big.ts (378)"), "{}", out);
        // Legend advertises `(N=loc)` so agents know the parens are a count.
        assert!(out.contains("F=file(N=loc)"), "{}", out);
    }

    #[test]
    fn loc_omitted_when_zero() {
        // Files we didn't read (unsupported ext) keep loc=0 — don't emit
        // `(0)` because it's misleading.
        let f = FileIndex {
            path: "src/a.ts".into(),
            lang: "ts".into(),
            defs: vec![make_def("foo", 1, SymbolKind::Func)],
            ..Default::default()
        };
        let opts = Options {
            no_legend: false,
            ..opts_minimal()
        };
        let out = render(&[f], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        assert!(!out.contains("(0)"), "{}", out);
        // Legend falls back to plain `F=file` when no file has a LOC.
        assert!(!out.contains("F=file(N=loc)"), "{}", out);
        assert!(out.contains("F=file"), "{}", out);
    }

    #[test]
    fn kotlin_package_peers_suppress_orphan_tag() {
        // Two Kotlin files share `com.ex.feature`. Neither imports the other,
        // so both have in_deg == 0 even though they may call each other via
        // same-package resolution (which tingle can't always see). Conservative
        // policy: don't tag either as orphan.
        let a = FileIndex {
            path: "app/src/main/java/com/ex/feature/ScreenA.kt".into(),
            ext: ".kt".into(),
            lang: "kt".into(),
            package: "com.ex.feature".into(),
            in_deg: 0,
            defs: vec![make_def("ScreenA", 1, SymbolKind::Class)],
            ..Default::default()
        };
        let b = FileIndex {
            path: "app/src/main/java/com/ex/feature/ScreenB.kt".into(),
            ext: ".kt".into(),
            lang: "kt".into(),
            package: "com.ex.feature".into(),
            in_deg: 0,
            defs: vec![make_def("ScreenB", 1, SymbolKind::Class)],
            ..Default::default()
        };
        // A lonely file in its own package is still orphan-eligible.
        let lonely = FileIndex {
            path: "app/src/main/java/com/ex/lonely/Solo.kt".into(),
            ext: ".kt".into(),
            lang: "kt".into(),
            package: "com.ex.lonely".into(),
            in_deg: 0,
            defs: vec![make_def("Solo", 1, SymbolKind::Class)],
            ..Default::default()
        };
        let opts = opts_minimal();
        let out = render(
            &[a, b, lonely],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &opts,
        );
        assert!(!out.contains("ScreenA.kt [orphan]"), "{}", out);
        assert!(!out.contains("ScreenB.kt [orphan]"), "{}", out);
        assert!(out.contains("Solo.kt [orphan]"), "{}", out);
    }

    #[test]
    fn imports_capped_with_overflow() {
        let mut imports = Vec::new();
        for i in 0..15 {
            imports.push(format!("dep{}", i));
        }
        let f = FileIndex {
            path: "src/aggregate.ts".into(),
            lang: "ts".into(),
            imports,
            ..Default::default()
        };
        let opts = opts_minimal();
        let out = render(&[f], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        assert!(
            out.contains("dep0 dep1 dep2 dep3 dep4 dep5 dep6 dep7 dep8 dep9 (+5 more)"),
            "{}",
            out
        );
        assert!(
            !out.contains("dep10"),
            "should be hidden under overflow:\n{}",
            out
        );
    }

    #[test]
    fn ep_record_gets_hub_annotation_when_also_utility() {
        let manager = FileIndex {
            path: "src/Manager.kt".into(),
            lang: "kt".into(),
            out_deg: 8,
            in_deg: 5,
            defs: vec![make_def("Manager", 1, SymbolKind::Class)],
            ..Default::default()
        };
        let pure_ep = FileIndex {
            path: "src/Main.kt".into(),
            lang: "kt".into(),
            out_deg: 10,
            in_deg: 0,
            defs: vec![make_def("Main", 1, SymbolKind::Class)],
            ..Default::default()
        };
        let opts = opts_minimal();
        let files = [manager, pure_ep];
        let out = render(
            &files,
            &[&files[0], &files[1]],
            &[&files[0]],
            &HashMap::new(),
            &HashMap::new(),
            &[],
            &opts,
        );
        assert!(
            out.contains("EP src/Manager.kt:1 Manager (out=8 in=5) [hub]"),
            "Manager EP record should be marked [hub]:\n{}",
            out
        );
        assert!(
            out.contains("EP src/Main.kt:1 Main (out=10 in=0)\n"),
            "Pure EP record (no utility overlap) should NOT be marked:\n{}",
            out
        );
    }

    #[test]
    fn m_records_dedup_across_source_sets() {
        // Two raw src dirs that compact to the same label (java + kotlin
        // source sets in the same Gradle module) → one merged M line.
        let mut edges: HashMap<String, Vec<String>> = HashMap::new();
        edges.insert(
            "core/src/main/java/com/x/shared/core/domain".into(),
            vec!["core/src/main/java/com/x/shared/core/models".into()],
        );
        edges.insert(
            "core/src/main/kotlin/com/x/shared/core/domain".into(),
            vec!["core/src/main/kotlin/com/x/shared/core/usecase".into()],
        );
        let opts = opts_minimal();
        let out = render(&[], &[], &[], &edges, &HashMap::new(), &[], &opts);
        // One M line for `core/domain` with both dsts merged.
        let m_lines: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("M core/domain"))
            .collect();
        assert_eq!(
            m_lines.len(),
            1,
            "expected one merged line, got: {:?}",
            m_lines
        );
        assert!(m_lines[0].contains("core/models"), "{}", m_lines[0]);
        assert!(m_lines[0].contains("core/usecase"), "{}", m_lines[0]);
    }

    #[test]
    fn modules_section_uses_compact_paths() {
        let mut edges: HashMap<String, Vec<String>> = HashMap::new();
        edges.insert(
            "core/src/main/java/com/x/shared/core/domain".into(),
            vec!["core/src/main/java/com/x/shared/core/models".into()],
        );
        let opts = opts_minimal();
        let out = render(&[], &[], &[], &edges, &HashMap::new(), &[], &opts);
        assert!(
            out.contains("M core/domain -> core/models"),
            "expected compact module paths:\n{}",
            out
        );
    }

    #[test]
    fn utility_callers_use_compact_paths() {
        let util = FileIndex {
            path: "core/src/main/java/com/x/shared/core/models/Foo.kt".into(),
            lang: "kt".into(),
            in_deg: 2,
            ..Default::default()
        };
        let mut callers = HashMap::new();
        callers.insert(
            util.path.clone(),
            vec!["app/src/main/java/com/x/one/services/A.kt".into()],
        );
        let files = [util];
        let opts = opts_minimal();
        let out = render(
            &files,
            &[],
            &[&files[0]],
            &HashMap::new(),
            &callers,
            &[],
            &opts,
        );
        assert!(
            out.contains("← app/services/A.kt"),
            "expected compact caller path:\n{}",
            out
        );
        // The U record's own path must stay full — it's an anchor.
        assert!(
            out.contains("U core/src/main/java/com/x/shared/core/models/Foo.kt"),
            "{}",
            out
        );
    }

    #[test]
    fn modules_sorted_alphabetically() {
        let mut edges: HashMap<String, Vec<String>> = HashMap::new();
        edges.insert("src/z".into(), vec!["src/a".into()]);
        edges.insert("src/a".into(), vec!["src/b".into(), "src/c".into()]);
        let opts = opts_minimal();
        let out = render(&[], &[], &[], &edges, &HashMap::new(), &[], &opts);
        let a_at = out.find("M src/a -> src/b src/c").unwrap();
        let z_at = out.find("M src/z -> src/a").unwrap();
        assert!(a_at < z_at);
    }
}
