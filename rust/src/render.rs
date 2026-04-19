//! Compact tag-prefixed rendering. Matches `internal/render/render.go`
//! byte-for-byte — the parity gate (make parity) enforces it.
//!
//! Section order: Manifests, Entry points, Utilities, Modules, Files.
//! Empty sections are omitted.

use std::collections::HashMap;
use std::fmt::Write;

use crate::model::{FileIndex, Symbol};

#[derive(Default, Clone)]
pub struct Options {
    pub version: String,
    pub commit: String,
    pub tokenizer_id: String,
    pub no_legend: bool,
    pub tokens_approx: u32,
    /// ISO date (UTC) used in the `gen=...` header. Stable across runs for
    /// the parity gate — set to today's date at invocation time.
    pub gen_date: String,
}

const LEGEND_LINE: &str = "# legend: S=manifest EP=entry U=utility M=module-edge F=file  [M]=modified [untracked]=new-unstaged [test]=test-file  [path:line]=def  f=func c=class m=method i=interface t=type e=enum";

pub fn render(
    files: &[FileIndex],
    entries: &[&FileIndex],
    utilities: &[&FileIndex],
    dir_edges: &HashMap<String, Vec<String>>,
    callers: &HashMap<String, Vec<String>>,
    manifests: &[String],
    opts: &Options,
) -> String {
    let mut b = String::new();

    // Header
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
    let tok = if opts.tokens_approx > 0 {
        format!("  tokens~{}", human_token(opts.tokens_approx))
    } else {
        String::new()
    };
    let tknzr = if opts.tokenizer_id.is_empty() {
        "cl100k_base"
    } else {
        opts.tokenizer_id.as_str()
    };
    writeln!(
        b,
        "# tingle {}  gen={}{}  files={}{}  tokenizer={}",
        ver,
        opts.gen_date,
        commit,
        count_parsed(files),
        tok,
        tknzr,
    )
    .unwrap();

    if !opts.no_legend {
        b.push_str(LEGEND_LINE);
        b.push('\n');
    }
    b.push('\n');

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

    // Utilities
    if !utilities.is_empty() {
        b.push_str("## Utilities\n");
        for f in utilities {
            let empty: Vec<String> = Vec::new();
            let cs = callers.get(&f.path).unwrap_or(&empty);
            let caller_str = if cs.is_empty() {
                String::new()
            } else {
                let max_show = 3usize.min(cs.len());
                let mut s = format!("  ← {}", cs[..max_show].join(" "));
                if cs.len() > max_show {
                    s.push_str(&format!(" (+{} more)", cs.len() - max_show));
                }
                s
            };
            writeln!(b, "U {} (in={}){}", f.path, f.in_deg, caller_str).unwrap();
            write_defs(&mut b, &f.defs);
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
    b.push_str("## Files\n");
    let mut sorted: Vec<&FileIndex> = files
        .iter()
        .filter(|f| !f.lang.is_empty() || !f.tags.is_empty())
        .collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    for f in sorted {
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
        // Match Go's `"F %s %s%s\n"` — preserves a trailing space when
        // tag_str is empty.
        writeln!(b, "F {} {}{}", f.path, tag_str, imps).unwrap();
        write_defs(&mut b, &f.defs);
    }

    b
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

fn count_parsed(files: &[FileIndex]) -> usize {
    files.iter().filter(|f| !f.lang.is_empty()).count()
}

fn human_token(n: u32) -> String {
    if n < 1000 {
        return n.to_string();
    }
    format!("{:.1}k", n as f64 / 1000.0)
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

    #[test]
    fn empty_repo_emits_header_plus_files_section() {
        let opts = Options {
            version: "v0-test".into(),
            gen_date: "2026-04-18".into(),
            tokenizer_id: "cl100k_base".into(),
            ..Default::default()
        };
        let out = render(&[], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        assert!(
            out.starts_with("# tingle v0-test  gen=2026-04-18  files=0  tokenizer=cl100k_base\n")
        );
        assert!(out.contains("## Files\n"));
    }

    #[test]
    fn file_with_no_tags_renders_trailing_space() {
        let f = FileIndex {
            path: "a.ts".into(),
            lang: "ts".into(),
            ..Default::default()
        };
        let opts = Options {
            version: "v0".into(),
            gen_date: "2026-04-18".into(),
            no_legend: true,
            ..Default::default()
        };
        let out = render(&[f], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        // F line has trailing space because tag_str is empty (Go parity).
        assert!(out.contains("F a.ts \n"), "output:\n{}", out);
    }

    #[test]
    fn file_with_tags_and_imports_renders_expected() {
        let f = FileIndex {
            path: "src/a.ts".into(),
            lang: "ts".into(),
            tags: vec!["M".into(), "test".into()],
            imports: vec!["react".into(), "./b".into()],
            defs: vec![{
                let mut s = make_def("bootstrap", 12, SymbolKind::Func);
                s.signature = "bootstrap () -> void".into();
                s
            }],
            ..Default::default()
        };

        let opts = Options {
            version: "v0".into(),
            gen_date: "2026-04-18".into(),
            no_legend: true,
            ..Default::default()
        };
        let out = render(&[f], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        assert!(
            out.contains("F src/a.ts [M][test]  imp: react ./b\n"),
            "{}",
            out
        );
        assert!(out.contains(" 12 f bootstrap () -> void\n"), "{}", out);
    }

    #[test]
    fn class_children_indent_two_spaces() {
        let mut cls = make_def("App", 5, SymbolKind::Class);
        cls.signature = "App".into();
        cls.children = vec![{
            let mut m = make_def("start", 10, SymbolKind::Method);
            m.signature = "start () -> void".into();
            m
        }];
        let f = FileIndex {
            path: "src/a.ts".into(),
            lang: "ts".into(),
            defs: vec![cls],
            ..Default::default()
        };

        let opts = Options {
            version: "v0".into(),
            gen_date: "2026-04-18".into(),
            no_legend: true,
            ..Default::default()
        };
        let out = render(&[f], &[], &[], &HashMap::new(), &HashMap::new(), &[], &opts);
        assert!(out.contains(" 5 c App\n"), "{}", out);
        assert!(out.contains("  10 m start () -> void\n"), "{}", out);
    }

    #[test]
    fn modules_sorted_alphabetically() {
        let mut edges: HashMap<String, Vec<String>> = HashMap::new();
        edges.insert("src/z".into(), vec!["src/a".into()]);
        edges.insert("src/a".into(), vec!["src/b".into(), "src/c".into()]);
        let opts = Options {
            version: "v0".into(),
            gen_date: "2026-04-18".into(),
            no_legend: true,
            ..Default::default()
        };
        let out = render(&[], &[], &[], &edges, &HashMap::new(), &[], &opts);
        let a_at = out.find("M src/a -> src/b src/c").unwrap();
        let z_at = out.find("M src/z -> src/a").unwrap();
        assert!(a_at < z_at);
    }
}
