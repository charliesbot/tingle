//! Heuristic import resolution.
//!
//! - Relative path math + extension + index-file + `__init__.py` trials.
//! - Kotlin FQCN resolution via a `package → dir → class → file` index built
//!   from parsed files' `package` headers and `defs`.
//! - Unresolved dotted imports >2 segments get collapsed to the first two
//!   segments (cuts `androidx.compose.foundation.background` noise).
//!
//! External / unmappable imports that aren't collapsed stay raw.

use std::collections::{HashMap, HashSet};

use crate::model::FileIndex;

pub type Aliases = HashMap<String, String>;

/// Default extensions to try, in rough order of likelihood.
const CANDIDATE_EXTS: &[&str] = &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".py", ".go", ".kt"];

/// Rewrite `f.imports` in place and populate `f.resolved_imports`.
///
/// `imports` holds the **display** string — what renders in the F record.
/// `resolved_imports` holds the **graph** edges — the repo-relative paths
/// used by `rank`. For most languages these are the same set; Kotlin
/// differs because resolved paths (`core/src/main/java/...`) are much
/// fatter than the original FQCN (`com.foo.bar.Baz`), so we resolve for
/// the graph but keep a compact FQCN-style string for display.
pub fn all(files: &mut [FileIndex], aliases: &Aliases) {
    let have: HashSet<String> = files.iter().map(|f| f.path.clone()).collect();
    let kotlin_index = build_kotlin_index(files);

    for f in files.iter_mut() {
        let from = f.path.clone();
        let is_kotlin = matches!(f.ext.as_str(), ".kt" | ".kts");
        // Dotted-import collapse is Kotlin-only. Other dotted-style import
        // languages (Python `django.db.models`) carry real signal in the
        // middle segments that collapse would destroy.
        let collapse_dotted_fallback = is_kotlin;
        let mut resolved: Vec<String> = Vec::new();

        for imp in f.imports.iter_mut() {
            // Already repo-internal (points to a known file)? record as edge.
            if have.contains(imp) {
                resolved.push(imp.clone());
                continue;
            }
            // Relative import?
            if let Some(r) = resolve_one(&from, imp, &have, aliases) {
                resolved.push(r.clone());
                *imp = r;
                continue;
            }
            // Kotlin FQCN? Resolve for the graph, then render a compact
            // `<module>:<ClassName>` tag for display — the full repo path
            // is much longer than both the FQCN and the display form.
            if is_kotlin {
                if let Some(r) = resolve_kotlin_fqcn(imp, &kotlin_index) {
                    let display = kotlin_compact_display(&r);
                    resolved.push(r);
                    *imp = display;
                    continue;
                }
            }
            // Fallback: collapse noisy dotted imports. Kotlin only — other
            // languages preserve full import strings.
            if collapse_dotted_fallback {
                if let Some(collapsed) = collapse_dotted(imp) {
                    *imp = collapsed;
                }
            }
        }

        // Dedupe both lists (collapse may create repeats).
        let mut seen_display = HashSet::new();
        f.imports.retain(|s| seen_display.insert(s.clone()));
        let mut seen_resolved = HashSet::new();
        resolved.retain(|s| seen_resolved.insert(s.clone()));
        f.resolved_imports = resolved;
    }
}

fn resolve_one(from: &str, imp: &str, have: &HashSet<String>, aliases: &Aliases) -> Option<String> {
    // Alias substitution first. Exact match or prefix/ match.
    let mut rewritten = imp.to_string();
    for (prefix, target) in aliases {
        if rewritten == *prefix || rewritten.starts_with(&format!("{}/", prefix)) {
            let suffix = &rewritten[prefix.len()..];
            rewritten = format!("{}{}", target, suffix);
            break;
        }
    }

    if !rewritten.starts_with('.') {
        return None;
    }

    let base_dir = parent_dir(from);
    let target = clean_join(base_dir, &rewritten);

    if have.contains(&target) {
        return Some(target);
    }
    for e in CANDIDATE_EXTS {
        let cand = format!("{}{}", target, e);
        if have.contains(&cand) {
            return Some(cand);
        }
    }
    for e in CANDIDATE_EXTS {
        let cand = if target.is_empty() {
            format!("index{}", e)
        } else {
            format!("{}/index{}", target, e)
        };
        if have.contains(&cand) {
            return Some(cand);
        }
    }
    let py_init = if target.is_empty() {
        "__init__.py".to_string()
    } else {
        format!("{}/__init__.py", target)
    };
    if have.contains(&py_init) {
        return Some(py_init);
    }
    None
}

/// Mirrors Go `filepath.Dir`: returns `"."` when the path has no slash.
fn parent_dir(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[..i],
        None => ".",
    }
}

/// Mirrors Go `filepath.Clean(filepath.Join(base, rel))` for forward-slash
/// repo-relative paths. Handles `.` and `..` components.
///
/// Semantics match Go's `filepath.Clean`:
///   - `.` segments are dropped
///   - `..` pops a preceding normal segment; when parts is empty or the
///     top is already `..`, the `..` is kept (so `..` components that
///     escape above the base are preserved)
fn clean_join(base: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in base.split('/').chain(rel.split('/')) {
        match seg {
            "" | "." => continue,
            ".." => match parts.last() {
                Some(&last) if last != ".." => {
                    parts.pop();
                }
                _ => parts.push(".."),
            },
            s => parts.push(s),
        }
    }
    parts.join("/")
}

// ---- Kotlin FQCN resolution ----

/// Index keyed by Kotlin package FQCN → (class_name → repo-relative path).
///
/// Example: `com.foo.bar` → { "Baz" → "src/main/kotlin/com/foo/bar/Baz.kt" }.
///
/// Multiple source roots (e.g. `app/src/main/java/...` and
/// `wear/src/main/java/...` both declaring `package com.x.y`) deduplicate
/// on class name; first file wins (stable by input file order).
#[derive(Default)]
struct KotlinIndex {
    /// `package` → { `class_name` → `file_path` }.
    ///
    /// When the same package is declared in multiple source roots (common in
    /// Android: `app/src/main/java/...` and `app/src/androidTest/java/...`
    /// both declare `package com.x`), first-file-wins. File order is
    /// deterministic per run (git ls-files sort order).
    by_pkg: HashMap<String, HashMap<String, String>>,
}

fn build_kotlin_index(files: &[FileIndex]) -> KotlinIndex {
    let mut idx = KotlinIndex::default();
    for f in files {
        if !matches!(f.ext.as_str(), ".kt" | ".kts") {
            continue;
        }
        if f.package.is_empty() {
            continue;
        }
        let entry = idx.by_pkg.entry(f.package.clone()).or_default();

        // Map every top-level def's name to this file. Kotlin allows
        // multiple declarations per file, so a single file may contribute
        // several keys.
        for d in &f.defs {
            entry
                .entry(d.name.clone())
                .or_insert_with(|| f.path.clone());
        }
        // Also map the filename-without-extension as a fallback — matches
        // the common "one public class per file named after it" convention.
        // `or_insert_with` keeps def-based entries (which have stronger
        // evidence) from being overwritten.
        let base = f.path.rsplit_once('/').map(|(_, b)| b).unwrap_or(&f.path);
        let stem = base
            .strip_suffix(".kt")
            .or(base.strip_suffix(".kts"))
            .unwrap_or(base);
        entry
            .entry(stem.to_string())
            .or_insert_with(|| f.path.clone());
    }
    idx
}

/// Resolve a Kotlin FQCN import like `com.foo.bar.Baz` or
/// `com.foo.bar.Baz.CONST` to the repo-relative file that declares it.
fn resolve_kotlin_fqcn(imp: &str, idx: &KotlinIndex) -> Option<String> {
    let segs: Vec<&str> = imp.split('.').collect();
    if segs.len() < 2 {
        return None;
    }
    // Try each split point from longest package prefix down to length-2.
    // For `a.b.c.D` we try:
    //   package="a.b.c" class="D"   (class import)
    //   package="a.b"   class="c"   (member import: a.b.c.D where c is the class)
    //   package="a"     class="b"   (rare — top-level package)
    for n in (1..segs.len()).rev() {
        let pkg = segs[..n].join(".");
        let class = segs[n];
        if let Some(inner) = idx.by_pkg.get(&pkg) {
            if let Some(path) = inner.get(class) {
                return Some(path.clone());
            }
        }
    }
    None
}

/// Render a resolved Kotlin import path as `<module>:<ClassName>`.
///
/// Takes a repo-relative path like `core/src/main/java/com/ex/shared/Repo.kt`
/// and returns `core:Repo`. For nested feature modules like
/// `features/settings/app/src/main/java/.../Foo.kt`, returns
/// `features/settings/app:Foo` (first path element up to `src/` boundary).
fn kotlin_compact_display(resolved_path: &str) -> String {
    let parts: Vec<&str> = resolved_path.split('/').collect();
    let filename = parts.last().copied().unwrap_or("");
    let class_name = filename
        .strip_suffix(".kt")
        .or_else(|| filename.strip_suffix(".kts"))
        .unwrap_or(filename);

    // Module prefix = everything before the first path element that looks
    // like a source-root marker. Covers both Gradle (`core/src/main/java`)
    // and Kotlin Multiplatform (`shared/commonMain/kotlin`) layouts.
    const SOURCE_ROOTS: &[&str] = &[
        "src",
        "commonMain",
        "androidMain",
        "iosMain",
        "jvmMain",
        "jsMain",
        "nativeMain",
        "kotlin",
        "java",
    ];
    if parts.len() < 2 {
        return class_name.to_string();
    }
    let boundary = parts
        .iter()
        .position(|p| SOURCE_ROOTS.contains(p))
        .unwrap_or(1);
    if boundary == 0 {
        class_name.to_string()
    } else {
        format!("{}:{}", parts[..boundary].join("/"), class_name)
    }
}

// ---- Dotted-import collapse (noise reduction) ----

/// If an unresolved import has the shape `a.b.c.d[...]` (≥3 dot-separated
/// segments, each a plain identifier), collapse to `a.b`. Preserves shorter
/// names (`react`, `os`, `fmt`) and scoped names (`@okta/sdk`) verbatim.
fn collapse_dotted(imp: &str) -> Option<String> {
    // Skip paths we already resolved (forward-slash) or scoped packages.
    if imp.contains('/') || imp.starts_with('@') {
        return None;
    }
    let segs: Vec<&str> = imp.split('.').collect();
    if segs.len() < 3 {
        return None;
    }
    // Every segment must be an identifier-ish — skip things like version
    // markers that happen to contain dots.
    for s in &segs {
        if s.is_empty()
            || !s
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return None;
        }
    }
    Some(format!("{}.{}", segs[0], segs[1]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Symbol, SymbolKind};

    fn fi(path: &str) -> FileIndex {
        FileIndex {
            path: path.to_string(),
            ..Default::default()
        }
    }

    fn kt(path: &str, pkg: &str, defs: &[&str]) -> FileIndex {
        let mut f = fi(path);
        f.ext = ".kt".into();
        f.lang = "kt".into();
        f.package = pkg.into();
        f.defs = defs
            .iter()
            .map(|n| Symbol {
                name: (*n).into(),
                kind: SymbolKind::Class,
                signature: (*n).into(),
                line: 1,
                children: vec![],
            })
            .collect();
        f
    }

    #[test]
    fn resolves_relative_same_dir_with_extension() {
        let mut files = vec![fi("src/a.ts"), fi("src/b.ts")];
        files[0].imports = vec!["./b".into()];
        all(&mut files, &HashMap::new());
        assert_eq!(files[0].imports, vec!["src/b.ts"]);
    }

    #[test]
    fn resolves_relative_parent_dir() {
        let mut files = vec![fi("src/nested/a.ts"), fi("src/shared.ts")];
        files[0].imports = vec!["../shared".into()];
        all(&mut files, &HashMap::new());
        assert_eq!(files[0].imports, vec!["src/shared.ts"]);
    }

    #[test]
    fn resolves_index_file() {
        let mut files = vec![fi("src/a.ts"), fi("src/util/index.ts")];
        files[0].imports = vec!["./util".into()];
        all(&mut files, &HashMap::new());
        assert_eq!(files[0].imports, vec!["src/util/index.ts"]);
    }

    #[test]
    fn resolves_python_init() {
        let mut files = vec![fi("app/main.py"), fi("app/pkg/__init__.py")];
        files[0].imports = vec!["./pkg".into()];
        all(&mut files, &HashMap::new());
        assert_eq!(files[0].imports, vec!["app/pkg/__init__.py"]);
    }

    #[test]
    fn alias_substitution_before_path_math() {
        let mut files = vec![fi("src/a.ts"), fi("src/shared.ts")];
        files[0].imports = vec!["@/shared".into()];
        let aliases: Aliases = [("@".into(), "src".into())].into_iter().collect();
        all(&mut files, &aliases);
        // After alias rewrite, "src/shared" doesn't start with "." so
        // neither path math nor FQCN applies. But it's 2 dot-segments
        // of junk — stays raw since collapse requires ≥3 segments.
        assert_eq!(files[0].imports, vec!["@/shared"]);
    }

    #[test]
    fn external_imports_stay_raw_when_short() {
        let mut files = vec![fi("src/a.ts")];
        files[0].imports = vec!["react".into(), "@okta/sdk".into()];
        all(&mut files, &HashMap::new());
        assert_eq!(files[0].imports, vec!["react", "@okta/sdk"]);
    }

    #[test]
    fn clean_join_keeps_escaping_dotdot() {
        assert_eq!(clean_join("src", "../../up"), "../up");
        assert_eq!(clean_join(".", "../../foo"), "../../foo");
        assert_eq!(clean_join("a/b", "../c"), "a/c");
        assert_eq!(clean_join("a/b", "../../../c"), "../c");
    }

    #[test]
    fn kotlin_fqcn_resolves_and_renders_compact_display() {
        let mut files = vec![
            kt(
                "core/src/main/java/com/ex/shared/Repo.kt",
                "com.ex.shared",
                &["Repo"],
            ),
            kt(
                "app/src/main/java/com/ex/app/App.kt",
                "com.ex.app",
                &["App"],
            ),
        ];
        files[1].imports = vec!["com.ex.shared.Repo".into()];
        all(&mut files, &HashMap::new());
        assert_eq!(files[1].imports, vec!["core:Repo"]);
        assert_eq!(
            files[1].resolved_imports,
            vec!["core/src/main/java/com/ex/shared/Repo.kt"]
        );
    }

    #[test]
    fn kotlin_fqcn_member_import_uses_class_not_member_in_display() {
        let mut files = vec![
            kt(
                "core/src/main/java/com/ex/shared/Const.kt",
                "com.ex.shared",
                &["Const"],
            ),
            kt(
                "app/src/main/java/com/ex/app/App.kt",
                "com.ex.app",
                &["App"],
            ),
        ];
        // Member-style: `com.ex.shared.Const.LOG_TAG` → display is the file's
        // class, not the member.
        files[1].imports = vec!["com.ex.shared.Const.LOG_TAG".into()];
        all(&mut files, &HashMap::new());
        assert_eq!(files[1].imports, vec!["core:Const"]);
        assert_eq!(
            files[1].resolved_imports,
            vec!["core/src/main/java/com/ex/shared/Const.kt"]
        );
    }

    #[test]
    fn kotlin_compact_display_handles_nested_modules() {
        assert_eq!(
            kotlin_compact_display("core/src/main/java/com/ex/Repo.kt"),
            "core:Repo"
        );
        assert_eq!(
            kotlin_compact_display("features/settings/app/src/main/java/com/ex/Foo.kt"),
            "features/settings/app:Foo"
        );
        assert_eq!(kotlin_compact_display("NoSrcPath.kt"), "NoSrcPath");
    }

    #[test]
    fn kotlin_compact_display_handles_multiplatform_layout() {
        // KMP: shared/commonMain/kotlin/...
        assert_eq!(
            kotlin_compact_display("shared/commonMain/kotlin/com/ex/Foo.kt"),
            "shared:Foo"
        );
        assert_eq!(
            kotlin_compact_display("shared/androidMain/kotlin/com/ex/Bar.kt"),
            "shared:Bar"
        );
    }

    #[test]
    fn python_dotted_imports_not_collapsed() {
        // `collapse_dotted` must NOT apply to Python — `django.db.models`
        // carries real module signal in the middle segments.
        let mut files = vec![FileIndex {
            path: "app/x.py".into(),
            ext: ".py".into(),
            lang: "py".into(),
            ..Default::default()
        }];
        files[0].imports = vec!["django.db.models".into(), "os.path".into()];
        all(&mut files, &HashMap::new());
        assert_eq!(files[0].imports, vec!["django.db.models", "os.path"]);
    }

    #[test]
    fn kotlin_unresolved_external_collapses() {
        let mut files = vec![kt(
            "app/src/main/java/com/ex/app/App.kt",
            "com.ex.app",
            &["App"],
        )];
        files[0].imports = vec![
            "androidx.compose.foundation.background".into(),
            "androidx.compose.foundation.layout.Column".into(),
            "kotlinx.coroutines.flow.Flow".into(),
        ];
        all(&mut files, &HashMap::new());
        // Two androidx.compose.* entries collapse + dedupe to one.
        assert_eq!(
            files[0].imports,
            vec![
                "androidx.compose".to_string(),
                "kotlinx.coroutines".to_string()
            ]
        );
    }

    #[test]
    fn collapse_leaves_two_segment_imports_alone() {
        assert_eq!(collapse_dotted("django.db"), None);
        assert_eq!(collapse_dotted("react"), None);
        assert_eq!(collapse_dotted("@okta/sdk"), None);
        assert_eq!(
            collapse_dotted("django.db.models"),
            Some("django.db".into())
        );
    }
}
