//! Compact tag-prefixed rendering. Output is AI-first (token-efficient),
//! not human-first — we optimize for the rate in bits-per-agent-answer,
//! not for casual readability.
//!
//! Section order: Manifests, Entry points, Utilities, Modules, Files.
//! Empty sections are omitted. The legend line is context-aware — it only
//! mentions prefix/tag categories that actually appear in THIS run.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

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
    /// `--skeleton`: omit the `## Files` section entirely.
    pub skeleton: bool,
    /// `--compact`: drop per-file def listings in the `## Files` section
    /// (file paths + imports + tags only). Opt-in aggressive trim for
    /// agents that just want the architecture + file listing.
    pub compact: bool,
}

/// Token count above which the header includes a shrink-suggestion line.
/// Char/4 approximation of cl100k_base. 20k is a rough "fits in one
/// agent turn with room for reply" budget — most agent environments
/// cap a single tool result around 20-30k tokens; crossing 20k is the
/// right signal to suggest `--compact`/`--scope`/`--skeleton`.
const TOKEN_WARN_THRESHOLD: usize = 20_000;

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
    let approx_tokens = (body.len() + out.len()) / 4;
    if approx_tokens > TOKEN_WARN_THRESHOLD {
        writeln!(
            out,
            "# warning: ~{}k tokens — consider --compact, --skeleton, or --scope PATH",
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
        sections.push("EP=entry");
    }
    if !utilities.is_empty() {
        sections.push("U=utility");
    }
    if !dir_edges.is_empty() {
        sections.push("M=module-edge");
    }
    let files_rendered = !opts.skeleton
        && files
            .iter()
            .any(|f| !f.lang.is_empty() || !f.tags.is_empty());
    if files_rendered {
        sections.push("F=file");
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

    // Def-kind markers — only if the F section will actually render defs.
    // Utilities no longer emit inline defs (they'd duplicate F section
    // content). `--compact` drops F-section defs too. In both cases the
    // def-kinds legend would advertise markers that never appear, which
    // is the exact UX bug this section was designed to prevent.
    let has_defs =
        !opts.compact && files_rendered && files.iter().any(|f| !f.defs.is_empty());
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

    // Entry points
    if !entries.is_empty() {
        b.push_str("## Entry points\n");
        for f in entries {
            let name = first_def_name(f);
            let line = first_def_line(f);
            writeln!(
                b,
                "EP {}:{} {} (out={} in={})",
                f.path, line, name, f.out_deg, f.in_deg
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
                // --compact tightens to a single caller; default shows 3.
                let cap = if opts.compact { 1 } else { 3 };
                let max_show = cap.min(cs.len());
                let mut s = format!("  ← {}", cs[..max_show].join(" "));
                if cs.len() > max_show {
                    s.push_str(&format!(" (+{} more)", cs.len() - max_show));
                }
                s
            };
            writeln!(b, "U {} (in={}){}", f.path, f.in_deg, caller_str).unwrap();
        }
        b.push('\n');
    }

    // Modules
    if !dir_edges.is_empty() {
        b.push_str("## Modules\n");
        let mut srcs: Vec<&String> = dir_edges.keys().collect();
        srcs.sort();
        for src in srcs {
            writeln!(b, "M {} -> {}", src, dir_edges[src].join(" ")).unwrap();
        }
        b.push('\n');
    }

    // Files
    if opts.skeleton {
        return b;
    }
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
                write_file_line(&mut b, f, &f.path, opts);
            }
        } else if children.len() == 1 {
            // Singleton group: the `### <dir>` header costs more than it
            // saves. Render the lone file with its full path, no header.
            let f = children[0];
            write_file_line(&mut b, f, &f.path, opts);
        } else {
            writeln!(b, "### {}", dir).unwrap();
            for f in children {
                let name = basename(&f.path);
                write_file_line(&mut b, f, name, opts);
            }
        }
    }

    b
}

fn write_file_line(b: &mut String, f: &FileIndex, display_name: &str, opts: &Options) {
    let mut tag_str = String::new();
    for t in &f.tags {
        tag_str.push('[');
        tag_str.push_str(t);
        tag_str.push(']');
    }
    let imps = if f.imports.is_empty() {
        String::new()
    } else {
        format!("  imp: {}", f.imports.join(" "))
    };
    writeln!(b, "F {} {}{}", display_name, tag_str, imps).unwrap();
    if !opts.compact {
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

fn first_def_name(f: &FileIndex) -> String {
    match f.defs.first() {
        Some(d) => d.name.clone(),
        None => basename(&f.path).to_string(),
    }
}

fn first_def_line(f: &FileIndex) -> u32 {
    f.defs.first().map(|d| d.line).unwrap_or(1)
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
    fn skeleton_omits_files_section() {
        let f = FileIndex {
            path: "a.ts".into(),
            lang: "ts".into(),
            ..Default::default()
        };
        let opts = Options {
            version: "v0".into(),
            gen_date: "2026-04-19".into(),
            no_legend: true,
            skeleton: true,
            ..Default::default()
        };
        let out = render(&[f], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        assert!(!out.contains("## Files"));
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
    fn compact_drops_per_file_defs() {
        let f = FileIndex {
            path: "src/a.ts".into(),
            lang: "ts".into(),
            defs: vec![make_def("foo", 5, SymbolKind::Func)],
            ..Default::default()
        };
        let opts = Options {
            version: "v0".into(),
            gen_date: "2026-04-19".into(),
            no_legend: true,
            compact: true,
            ..Default::default()
        };
        let out = render(&[f], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        assert!(out.contains("F src/a.ts "), "{}", out);
        assert!(
            !out.contains(" 5 f foo"),
            "compact must not emit defs:\n{}",
            out
        );
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
