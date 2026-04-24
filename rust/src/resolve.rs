//! Heuristic import resolution.
//!
//! - Relative path math + extension + index-file + `__init__.py` trials.
//! - Kotlin FQCN resolution and dotted-import collapse delegated to
//!   `lang::jvm` — the JVM-ecosystem-specific code lives in one file so
//!   it's easy to find and easy to change.
//!
//! External / unmappable imports that aren't collapsed stay raw.

use std::collections::{HashMap, HashSet};

use crate::lang::{jvm, vue};
use crate::model::FileIndex;

pub type Aliases = HashMap<String, String>;

/// Default extensions to try, in rough order of likelihood.
const CANDIDATE_EXTS: &[&str] = &[
    ".ts", ".tsx", ".js", ".jsx", ".mjs", ".vue", ".py", ".go", ".kt",
];

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
    let kotlin_index = jvm::build_kotlin_index(files);
    let vue_index = vue::build_component_index(files);

    for f in files.iter_mut() {
        let from = f.path.clone();
        // Single gate for the JVM-ecosystem code paths. See `lang::jvm`
        // for everything that switches on this.
        let is_kotlin = jvm::is_kotlin_ext(&f.ext);
        // AndroidManifest.xml carries class FQCNs in its `imports` list
        // (populated by `parse`). Route them through the same FQCN
        // resolver Kotlin uses — this is how Activities/Services/
        // Receivers/Application get counted as live without a code import.
        let is_manifest = jvm::is_android_manifest_path(&f.path);
        // DI-registration flag must be read from the *original* imports —
        // the FQCN-rewrite loop below strips the `org.koin.` / `dagger.`
        // prefixes the heuristic relies on.
        if is_kotlin {
            f.is_registration = jvm::is_registration_imports(&f.imports);
        }
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
            // `<module>/<ClassName>` tag for display — the full repo path
            // is much longer than both the FQCN and the display form.
            // Manifest files share this path: their `imports` are class
            // FQCNs extracted from `android:name=` attributes.
            if is_kotlin || is_manifest {
                if let Some(r) = jvm::resolve_kotlin_fqcn(imp, &kotlin_index) {
                    let display = jvm::kotlin_compact_display(&r);
                    resolved.push(r);
                    *imp = display;
                    continue;
                }
            }
            // Fallback: collapse noisy dotted imports. Kotlin only — other
            // languages preserve full import strings.
            if is_kotlin {
                if let Some(collapsed) = jvm::collapse_dotted(imp) {
                    *imp = collapsed;
                }
            }
        }

        // Kotlin same-package references: the import list doesn't capture
        // calls to top-level decls in the file's own package (Kotlin resolves
        // them without an import). Backfill those edges from the symbol refs
        // we extracted from the file body — this is the structural fix for
        // the "every screen in a feature looks orphan" failure mode on
        // Android/Kotlin repos.
        if is_kotlin {
            for r in &f.refs {
                if let Some(path) = jvm::resolve_same_package_ref(r, &f.package, &kotlin_index) {
                    if path != from {
                        resolved.push(path);
                    }
                }
            }
        }

        // Vue template refs: `<Foo />` in a `<template>` block resolves to
        // `components/Foo.vue` via Nuxt / unplugin-vue-components
        // auto-registration. Without this backfill, Vue projects that rely
        // on auto-import look entirely orphan — the reviewer-dropped
        // Slidev repo is the canonical failure mode.
        if vue::is_vue_ext(&f.ext) {
            for r in &f.refs {
                if let Some(path) = vue_index.get(r) {
                    if path != &from {
                        resolved.push(path.clone());
                    }
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
        assert_eq!(files[1].imports, vec!["core/Repo"]);
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
        assert_eq!(files[1].imports, vec!["core/Const"]);
        assert_eq!(
            files[1].resolved_imports,
            vec!["core/src/main/java/com/ex/shared/Const.kt"]
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
    fn kotlin_same_package_ref_creates_edge_without_import() {
        // Two files share `com.ex.app`. `Caller.kt` calls `helper()` which is
        // declared in `Helper.kt` — no import, because Kotlin same-package
        // resolution happens implicitly. The ref-based resolver backfills
        // the missing graph edge.
        let mut files = vec![
            {
                let mut f = kt(
                    "app/src/main/java/com/ex/app/Caller.kt",
                    "com.ex.app",
                    &["Caller"],
                );
                f.refs = vec!["helper".into()];
                f
            },
            kt(
                "app/src/main/java/com/ex/app/Helper.kt",
                "com.ex.app",
                &["helper"],
            ),
        ];
        all(&mut files, &HashMap::new());
        assert_eq!(
            files[0].resolved_imports,
            vec!["app/src/main/java/com/ex/app/Helper.kt"]
        );
        // Display imports stay empty — no import statement existed, and
        // synthesizing one would be dishonest.
        assert!(files[0].imports.is_empty());
    }

    #[test]
    fn kotlin_ref_ignores_self_reference() {
        // A file referencing its own declarations (recursive calls, internal
        // type usage) must not generate a self-edge.
        let mut files = vec![{
            let mut f = kt(
                "app/src/main/java/com/ex/Foo.kt",
                "com.ex",
                &["Foo", "helper"],
            );
            f.refs = vec!["helper".into(), "Foo".into()];
            f
        }];
        all(&mut files, &HashMap::new());
        assert!(files[0].resolved_imports.is_empty());
    }

    #[test]
    fn kotlin_ref_ignores_cross_package_match() {
        // `helper` exists in another package — a file that references
        // `helper` in ITS package should not accidentally match.
        let mut files = vec![
            {
                let mut f = kt("app/src/main/java/com/a/Caller.kt", "com.a", &["Caller"]);
                f.refs = vec!["helper".into()];
                f
            },
            kt("app/src/main/java/com/b/Helper.kt", "com.b", &["helper"]),
        ];
        all(&mut files, &HashMap::new());
        assert!(files[0].resolved_imports.is_empty());
    }

    #[test]
    fn kotlin_registration_flag_set_when_koin_imported() {
        let mut files = vec![{
            let mut f = kt(
                "app/src/main/java/com/ex/di/AppModule.kt",
                "com.ex.di",
                &["AppModule"],
            );
            f.imports = vec!["org.koin.dsl.module".into()];
            f
        }];
        all(&mut files, &HashMap::new());
        assert!(files[0].is_registration);
    }

    #[test]
    fn kotlin_registration_flag_unset_for_plain_kotlin() {
        let mut files = vec![{
            let mut f = kt("app/src/main/java/com/ex/Foo.kt", "com.ex", &["Foo"]);
            f.imports = vec!["kotlinx.coroutines.flow.Flow".into()];
            f
        }];
        all(&mut files, &HashMap::new());
        assert!(!files[0].is_registration);
    }

    #[test]
    fn android_manifest_resolves_class_refs_to_kotlin_files() {
        // AresApplication-style case: the app class is referenced only from
        // AndroidManifest.xml. Without manifest wiring, it would look orphan.
        // With the resolver change, the manifest contributes a real edge.
        let manifest = FileIndex {
            path: "app/src/main/AndroidManifest.xml".into(),
            lang: "androidManifest".into(),
            imports: vec!["com.ex.app.AresApplication".into()],
            ..Default::default()
        };
        let app_class = kt(
            "app/src/main/java/com/ex/app/AresApplication.kt",
            "com.ex.app",
            &["AresApplication"],
        );
        let mut files = vec![manifest, app_class];
        all(&mut files, &HashMap::new());
        assert_eq!(
            files[0].resolved_imports,
            vec!["app/src/main/java/com/ex/app/AresApplication.kt"]
        );
        // Display collapses to the compact `<module>/<ClassName>` form.
        assert_eq!(files[0].imports, vec!["app/AresApplication"]);
    }

    fn vue(path: &str) -> FileIndex {
        FileIndex {
            path: path.to_string(),
            ext: ".vue".into(),
            lang: "vue".into(),
            ..Default::default()
        }
    }

    #[test]
    fn vue_template_ref_resolves_to_component_file() {
        // Slidev-style: a `.vue` page references `<Badge />` with no import
        // because Nuxt/unplugin-vue-components auto-registers
        // `components/**`. The ref-based resolver has to backfill the edge.
        let mut files = vec![
            {
                let mut f = vue("src/pages/Home.vue");
                f.refs = vec!["Badge".into(), "Callout".into()];
                f
            },
            vue("src/components/Badge.vue"),
            vue("src/components/Callout.vue"),
        ];
        all(&mut files, &HashMap::new());
        assert!(files[0]
            .resolved_imports
            .contains(&"src/components/Badge.vue".into()));
        assert!(files[0]
            .resolved_imports
            .contains(&"src/components/Callout.vue".into()));
        // Template refs don't render as F-record imports (no source-level
        // `import` string existed).
        assert!(files[0].imports.is_empty());
    }

    #[test]
    fn vue_template_ref_ignores_unknown_and_self() {
        // `Unknown` isn't a repo component (framework built-in or typo) —
        // no edge. `Home` matches this file — no self-edge.
        let mut files = vec![{
            let mut f = vue("src/pages/Home.vue");
            f.refs = vec!["Unknown".into(), "Home".into()];
            f
        }];
        all(&mut files, &HashMap::new());
        assert!(files[0].resolved_imports.is_empty());
    }

    #[test]
    fn vue_explicit_import_resolves_with_vue_in_candidate_exts() {
        // Extensionless `./Badge` imports should still resolve when the
        // target is a `.vue` file — `.vue` is in CANDIDATE_EXTS.
        let mut files = vec![vue("src/pages/Home.vue"), vue("src/pages/Badge.vue")];
        files[0].imports = vec!["./Badge".into()];
        all(&mut files, &HashMap::new());
        assert_eq!(files[0].imports, vec!["src/pages/Badge.vue"]);
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
}
