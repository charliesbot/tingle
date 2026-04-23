//! Ranking: entry points, utilities, module-edge graph.
//!
//! Mirrors `internal/rank/rank.go`. Scoring blends filename conventions,
//! shebang detection, manifest-declared entries, (out − in) degree, and a
//! root-export bonus. Utility rank = in-degree.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::model::FileIndex;

pub struct GraphOutput {
    pub dir_edges: HashMap<String, Vec<String>>,
    pub callers: HashMap<String, Vec<String>>,
}

/// Build graph edges, populate `out_deg` / `in_deg`, return dir→dir edges
/// and file→caller lists.
pub fn graph(files: &mut [FileIndex]) -> GraphOutput {
    let by_path: HashMap<String, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.clone(), i))
        .collect();

    let mut raw_edges: BTreeMap<String, BTreeMap<String, ()>> = BTreeMap::new();
    let mut callers: HashMap<String, Vec<String>> = HashMap::new();

    // Two-pass: first figure out (src_path, import_target, contributes_edge)
    // triplets, then apply mutations, since we can't borrow files mutably
    // while iterating indices.
    let mut in_deg_bumps: HashMap<usize, u32> = HashMap::new();
    let mut out_deg_bumps: HashMap<usize, u32> = HashMap::new();

    for (src_idx, src_file) in files.iter().enumerate() {
        let src_path = src_file.path.clone();
        let src_dir = parent_dir(&src_path).to_string();
        // Graph uses `resolved_imports` (repo-relative paths) — `imports` may
        // carry compact display strings (e.g. Kotlin FQCNs) that intentionally
        // don't appear in `by_path`.
        let imports = src_file.resolved_imports.clone();
        for imp in &imports {
            if let Some(&tgt_idx) = by_path.get(imp) {
                *in_deg_bumps.entry(tgt_idx).or_default() += 1;
                callers
                    .entry(imp.clone())
                    .or_default()
                    .push(src_path.clone());
                let dst = parent_dir(imp).to_string();
                if dst != src_dir {
                    raw_edges
                        .entry(src_dir.clone())
                        .or_default()
                        .insert(dst, ());
                    *out_deg_bumps.entry(src_idx).or_default() += 1;
                }
            }
        }
    }

    for (i, n) in in_deg_bumps {
        files[i].in_deg += n;
    }
    for (i, n) in out_deg_bumps {
        files[i].out_deg += n;
    }

    let dir_edges: HashMap<String, Vec<String>> = raw_edges
        .into_iter()
        .map(|(src, dsts)| (src, dsts.into_keys().collect()))
        .collect();

    for cs in callers.values_mut() {
        cs.sort();
        cs.dedup();
    }

    GraphOutput { dir_edges, callers }
}

pub struct EntryPointsOpts<'a> {
    pub repo: &'a Path,
    pub manifest_ep: &'a [String],
    pub max_eps: usize,
}

/// Return files ranked by the entry-point heuristic, capped at `max_eps`
/// with score > 0. Test-tagged files are excluded — they're probes, not
/// entries (feedback from multiple real-use agents: test files kept
/// outranking production ViewModels/Services with only slightly higher
/// out_deg).
pub fn entry_points<'a>(files: &'a [FileIndex], opts: EntryPointsOpts) -> Vec<&'a FileIndex> {
    let manifest_set: HashSet<&str> = opts.manifest_ep.iter().map(|s| s.as_str()).collect();

    let mut scored: Vec<(i32, &FileIndex)> = Vec::new();
    for f in files {
        if f.lang.is_empty() || f.defs.is_empty() {
            continue;
        }
        if f.tags.iter().any(|t| t == "test") {
            continue;
        }
        let s = score_one(f, opts.repo, &manifest_set);
        if s > 0 {
            scored.push((s, f));
        }
    }
    // Stable sort desc by score.
    scored.sort_by_key(|x| std::cmp::Reverse(x.0));

    let cap = if opts.max_eps == 0 { 15 } else { opts.max_eps };
    let n = cap.min(scored.len());
    scored.into_iter().take(n).map(|(_, f)| f).collect()
}

/// Files with ≥2 inbound edges from *non-registration* callers, sorted by
/// total in_deg descending (stable).
///
/// DI/registration files (Koin/Hilt modules) are routing, not logical
/// dependency: one Koin module binding 11 classes inflates each binding's
/// in_deg by 1 and drags 11 DTOs into the Utilities section. We keep the
/// raw `in_deg` for display honesty but filter by non-registration callers
/// so the section reflects code that's actually reused.
pub fn utilities<'a>(
    files: &'a [FileIndex],
    callers: &HashMap<String, Vec<String>>,
) -> Vec<&'a FileIndex> {
    let by_path: HashMap<&str, &FileIndex> = files.iter().map(|f| (f.path.as_str(), f)).collect();
    let mut out: Vec<&FileIndex> = files
        .iter()
        .filter(|f| {
            let Some(cs) = callers.get(f.path.as_str()) else {
                return false;
            };
            let non_reg = cs
                .iter()
                .filter(|c| {
                    by_path
                        .get(c.as_str())
                        .map_or(true, |cf| !cf.is_registration)
                })
                .count();
            non_reg >= 2
        })
        .collect();
    out.sort_by_key(|f| std::cmp::Reverse(f.in_deg));
    out
}

// --- scoring helpers ---

const CONVENTION_ENTRY: &[&str] = &[
    "main.go",
    "index.ts",
    "index.tsx",
    "index.js",
    "server.ts",
    "server.js",
    "app.ts",
    "app.tsx",
    "cli.ts",
    "manage.py",
    "__main__.py",
];

const PACKAGE_ROOT_PREFIXES: &[&str] = &["cmd/", "src/", "pkg/", "internal/"];

fn score_one(f: &FileIndex, repo: &Path, manifest_set: &HashSet<&str>) -> i32 {
    let mut score: i32 = 0;
    let base = basename(&f.path);
    if CONVENTION_ENTRY.contains(&base) {
        score += 10;
    }
    if base.starts_with("App.") {
        score += 8;
    }
    if manifest_set.contains(f.path.as_str()) {
        score += 10;
    }
    if has_shebang(&repo.join(&f.path)) {
        score += 10;
    }
    for prefix in PACKAGE_ROOT_PREFIXES {
        if f.path.starts_with(prefix) {
            let rest = &f.path[prefix.len()..];
            if rest.matches('/').count() <= 1 {
                score += 5;
                break;
            }
        }
    }
    score += f.out_deg as i32 - f.in_deg as i32;
    score
}

fn basename(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

/// Mirrors Go `filepath.Dir`: returns `"."` when the path has no slash.
fn parent_dir(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[..i],
        None => ".",
    }
}

fn has_shebang(full: &Path) -> bool {
    let Ok(file) = File::open(full) else {
        return false;
    };
    let mut r = BufReader::new(file);
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return false;
    }
    line.starts_with("#!")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Symbol, SymbolKind};

    fn fi(path: &str, lang: &str) -> FileIndex {
        FileIndex {
            path: path.to_string(),
            lang: lang.to_string(),
            defs: vec![Symbol {
                name: "x".into(),
                kind: SymbolKind::Func,
                signature: "x".into(),
                line: 1,
                children: vec![],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn graph_builds_dir_edges_and_callers() {
        let mut files = vec![
            {
                let mut f = fi("src/a.ts", "ts");
                f.resolved_imports = vec!["src/util/helper.ts".into()];
                f
            },
            fi("src/util/helper.ts", "ts"),
        ];
        let g = graph(&mut files);
        assert_eq!(files[0].out_deg, 1);
        assert_eq!(files[1].in_deg, 1);
        assert_eq!(g.dir_edges["src"], vec!["src/util"]);
        assert_eq!(g.callers["src/util/helper.ts"], vec!["src/a.ts"]);
    }

    #[test]
    fn same_dir_imports_bump_indeg_but_no_edge() {
        let mut files = vec![
            {
                let mut f = fi("src/a.ts", "ts");
                f.resolved_imports = vec!["src/b.ts".into()];
                f
            },
            fi("src/b.ts", "ts"),
        ];
        let g = graph(&mut files);
        assert_eq!(files[0].out_deg, 0);
        assert_eq!(files[1].in_deg, 1);
        assert!(g.dir_edges.is_empty());
    }

    #[test]
    fn utility_rank_by_indeg() {
        let mut a = fi("src/a.ts", "ts");
        a.in_deg = 5;
        let mut b = fi("src/b.ts", "ts");
        b.in_deg = 1;
        let mut c = fi("src/c.ts", "ts");
        c.in_deg = 2;
        let files = vec![a, b, c];
        let mut callers: HashMap<String, Vec<String>> = HashMap::new();
        callers.insert(
            "src/a.ts".into(),
            vec![
                "x1.ts".into(),
                "x2.ts".into(),
                "x3.ts".into(),
                "x4.ts".into(),
                "x5.ts".into(),
            ],
        );
        callers.insert("src/b.ts".into(), vec!["x1.ts".into()]);
        callers.insert("src/c.ts".into(), vec!["x1.ts".into(), "x2.ts".into()]);
        let u = utilities(&files, &callers);
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].path, "src/a.ts");
        assert_eq!(u[1].path, "src/c.ts");
    }

    #[test]
    fn utility_excludes_files_with_only_registration_callers() {
        // Classic Android/Kotlin pattern: Koin `AppModule.kt` binds 11 services;
        // each binding gets +1 in_deg from the module. Without discount, all 11
        // appear as utilities. With discount, only those with real callers stay.
        let mut di = fi("app/di/AppModule.kt", "kt");
        di.is_registration = true;
        let mut tag = fi("app/services/TagHandler.kt", "kt");
        tag.in_deg = 1; // only the DI module uses it
        let mut real = fi("app/services/RealService.kt", "kt");
        real.in_deg = 3; // DI module + 2 actual callers
        let files = vec![di, tag, real];
        let mut callers: HashMap<String, Vec<String>> = HashMap::new();
        callers.insert(
            "app/services/TagHandler.kt".into(),
            vec!["app/di/AppModule.kt".into()],
        );
        callers.insert(
            "app/services/RealService.kt".into(),
            vec![
                "app/di/AppModule.kt".into(),
                "app/ui/Screen.kt".into(),
                "app/worker/Job.kt".into(),
            ],
        );
        let u = utilities(&files, &callers);
        let paths: Vec<&str> = u.iter().map(|f| f.path.as_str()).collect();
        assert!(
            !paths.contains(&"app/services/TagHandler.kt"),
            "{:?}",
            paths
        );
        assert!(
            paths.contains(&"app/services/RealService.kt"),
            "{:?}",
            paths
        );
    }

    #[test]
    fn entry_points_filter_test_tagged_files() {
        let repo = Path::new("/nonexistent");
        let mut real = fi("app/src/main/VM.kt", "kt");
        real.out_deg = 10;
        let mut test = fi("app/src/test/VMTest.kt", "kt");
        test.out_deg = 15;
        test.tags = vec!["test".into()];
        let files = vec![real, test];
        let eps = entry_points(
            &files,
            EntryPointsOpts {
                repo,
                manifest_ep: &[],
                max_eps: 15,
            },
        );
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].path, "app/src/main/VM.kt");
    }

    #[test]
    fn entry_points_rank_by_score() {
        let repo = Path::new("/nonexistent");
        let mut main = fi("cmd/server/main.go", "go");
        main.out_deg = 5;
        let mut idx = fi("src/index.ts", "ts");
        idx.out_deg = 3;
        let mut util = fi("src/util.ts", "ts");
        util.in_deg = 10;
        let files = vec![main, idx, util];
        let eps = entry_points(
            &files,
            EntryPointsOpts {
                repo,
                manifest_ep: &[],
                max_eps: 15,
            },
        );
        let names: Vec<&str> = eps.iter().map(|f| f.path.as_str()).collect();
        assert!(names.contains(&"cmd/server/main.go"), "{:?}", names);
        assert!(names.contains(&"src/index.ts"), "{:?}", names);
        // util has high in_deg and isn't an entry convention → negative score → excluded
        assert!(!names.contains(&"src/util.ts"), "{:?}", names);
    }
}
