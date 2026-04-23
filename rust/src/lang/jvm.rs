//! JVM-ecosystem special-casing: Kotlin + Gradle + KMP + Android.
//!
//! Everything here is a *controlled hack* — knowledge that doesn't
//! generalize across tingle's other supported languages but earns its keep
//! on Kotlin/Gradle/Android repos (the only ecosystem where the defaults
//! produced enough output noise to warrant per-ecosystem code). Centralized
//! so a future maintainer can grep "what does tingle do for JVM repos?"
//! and find one file.
//!
//! Surface, by call site:
//!   - `is_kotlin_ext`         — gate Kotlin-only code paths in `resolve`
//!   - `KotlinIndex` + `build_kotlin_index` / `resolve_kotlin_fqcn` /
//!     `kotlin_compact_display` — FQCN resolution + display compaction
//!   - `resolve_same_package_ref` / `kotlin_packages_with_peers` —
//!     same-package usage resolution + orphan-policy helper
//!   - `is_registration_imports` — Koin/Hilt/Dagger DI detection for the
//!     utility-scoring discount
//!   - `collapse_dotted`       — fallback for unresolved Kotlin FQCN imports
//!   - `compact_label_path`    — Gradle source-root stripping for labels
//!     (M edges, U caller lists)
//!   - `is_android_manifest_path` / `extract_android_manifest_refs` —
//!     surface `android:name=` class refs so Manifest-wired classes
//!     (Application/Activity/Service/Receiver/Provider) aren't orphan
//!   - `is_android_test_path`  — Android Gradle test-set directories
//!
//! When tingle gains validated coverage of more JVM-ish layouts (multi-module
//! Maven, Bazel-Java, etc.), extend the patterns *here* rather than scattering
//! checks across the core modules.

use std::collections::HashMap;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::model::FileIndex;

/// True for Kotlin source extensions (`.kt`, `.kts`). Centralized so the
/// Kotlin-only code paths in `resolve` flow through one predicate.
pub fn is_kotlin_ext(ext: &str) -> bool {
    matches!(ext, ".kt" | ".kts")
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
pub struct KotlinIndex {
    /// `package` → { `class_name` → `file_path` }.
    ///
    /// When the same package is declared in multiple source roots (common in
    /// Android: `app/src/main/java/...` and `app/src/androidTest/java/...`
    /// both declare `package com.x`), first-file-wins. File order is
    /// deterministic per run (git ls-files sort order).
    by_pkg: HashMap<String, HashMap<String, String>>,
}

pub fn build_kotlin_index(files: &[FileIndex]) -> KotlinIndex {
    let mut idx = KotlinIndex::default();
    for f in files {
        if !is_kotlin_ext(&f.ext) {
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

/// Resolve an unqualified symbol name against the file's own package.
/// Same-package references don't require an `import` in Kotlin, so the
/// import list doesn't capture them — this backfills the missing edges.
///
/// Returns the repo-relative file path of the declaring file, or `None`
/// if the package has no file declaring `name`. Empty `package` never
/// resolves (top-level / default package files aren't indexed).
pub fn resolve_same_package_ref(name: &str, package: &str, idx: &KotlinIndex) -> Option<String> {
    if package.is_empty() {
        return None;
    }
    idx.by_pkg.get(package)?.get(name).cloned()
}

/// Count distinct files declaring `package`, excluding `self_path`.
/// Used by the orphan-policy check: a Kotlin file with package peers
/// can't be proven unused by syntactic analysis (the peers may call it
/// without an import), so the orphan tag is suppressed.
pub fn package_peer_count(package: &str, self_path: &str, idx: &KotlinIndex) -> usize {
    if package.is_empty() {
        return 0;
    }
    let Some(m) = idx.by_pkg.get(package) else {
        return 0;
    };
    let mut paths: std::collections::HashSet<&str> = m.values().map(String::as_str).collect();
    paths.remove(self_path);
    paths.len()
}

/// Kotlin package names that contain ≥2 distinct Kotlin files. Used by the
/// orphan-policy check: if a file lives in such a package, at least one
/// sibling exists that could reference it without an import, so we can't
/// assert orphan on syntactic grounds alone.
pub fn kotlin_packages_with_peers(files: &[FileIndex]) -> std::collections::HashSet<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut seen_paths: HashMap<&str, std::collections::HashSet<&str>> = HashMap::new();
    for f in files {
        if !is_kotlin_ext(&f.ext) || f.package.is_empty() {
            continue;
        }
        let paths = seen_paths.entry(f.package.as_str()).or_default();
        if paths.insert(f.path.as_str()) {
            *counts.entry(f.package.as_str()).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(k, v)| if v >= 2 { Some(k.to_string()) } else { None })
        .collect()
}

/// True if this file's import list shows signs of being a Koin/Hilt/Dagger
/// DI-registration module. Call BEFORE `resolve::all` mutates `imports` —
/// Kotlin FQCNs get rewritten to compact display strings that would strip
/// the `org.koin.` / `dagger.` prefixes this check relies on.
///
/// Heuristic, not semantic: we can't tell a DI-wiring file from a file
/// that just happens to import Koin's `KoinComponent` for injection.
/// That's fine — the consumer (`rank::utilities`) uses this as a *soft*
/// signal to discount inbound edges, not to hide the file.
pub fn is_registration_imports(imports: &[String]) -> bool {
    imports
        .iter()
        .any(|i| i.starts_with("org.koin.") || i.starts_with("dagger."))
}

/// Resolve a Kotlin FQCN import like `com.foo.bar.Baz` or
/// `com.foo.bar.Baz.CONST` to the repo-relative file that declares it.
pub fn resolve_kotlin_fqcn(imp: &str, idx: &KotlinIndex) -> Option<String> {
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

/// Render a resolved Kotlin import path as `<module>/<ClassName>`.
///
/// Takes a repo-relative path like `core/src/main/java/com/ex/shared/Repo.kt`
/// and returns `core/Repo`. For nested feature modules like
/// `features/settings/app/src/main/java/.../Foo.kt`, returns
/// `features/settings/app/Foo` (first path element up to `src/` boundary).
///
/// (Slash, not colon: agent feedback noted that `core:data/repository`-style
/// labels read as Gradle-`:module:`-notation but aren't, causing mental
/// remapping. Slashes throughout are honest about what we know — these are
/// repo-relative virtual paths, not Gradle module declarations.)
pub fn kotlin_compact_display(resolved_path: &str) -> String {
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
        format!("{}/{}", parts[..boundary].join("/"), class_name)
    }
}

// ---- Dotted-import collapse (noise reduction) ----

/// If an unresolved import has the shape `a.b.c.d[...]` (≥3 dot-separated
/// segments, each a plain identifier), collapse to `a.b`. Preserves shorter
/// names (`react`, `os`, `fmt`) and scoped names (`@okta/sdk`) verbatim.
///
/// **Kotlin-only** — caller (resolve.rs) gates by extension. Other
/// dotted-import languages (Python `django.db.models`) carry real signal in
/// the middle segments that collapse would destroy.
pub fn collapse_dotted(imp: &str) -> Option<String> {
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

// ---- Gradle source-root label compaction ----

/// Compact a repo path for **label** use (M records, U caller lists) where
/// the path is informational, not an anchor for downstream Read. Strips
/// Gradle source-root boilerplate — `src/main/<lang>/com/<org>/<proj>` —
/// producing `<module>/<tail>` form (slashes throughout — see Note below).
///
/// Anchors (F records, EP records, U record paths) must NOT use this —
/// the agent needs the full path to Read the file.
///
/// Examples:
///   `core/src/main/java/com/charliesbot/shared/core/constants` → `core/constants`
///   `app/src/main/kotlin/com/x/one/data/Foo.kt` → `app/data/Foo.kt`
///   `features/dashboard/app/src/main/java/com/x/one/features/dashboard/VM.kt`
///     → `features/dashboard/app/features/dashboard/VM.kt`
///   `src/components/Form.tsx` → unchanged (no Gradle-style boilerplate)
///
/// Note on the `:` → `/` switch: an earlier version used `<module>:<tail>`
/// to highlight the module boundary visually. Two separate Android-Kotlin
/// agents read those labels as Gradle `:module:submodule` notation and
/// noted the format mismatch with their actual `settings.gradle.kts`
/// declarations. Slashes throughout are honest about what we know:
/// repo-relative virtual paths, no Gradle-module pretension.
pub fn compact_label_path(p: &str) -> String {
    let parts: Vec<&str> = p.split('/').collect();
    let Some(src_i) = parts.iter().position(|s| *s == "src") else {
        return p.to_string();
    };
    if src_i == 0 {
        return p.to_string();
    }
    let skip = src_i + 1 + 5;
    if skip > parts.len() {
        return p.to_string();
    }
    let module = &parts[..src_i];
    let mut tail = &parts[skip..];
    // Multi-segment tail dedup: find the longest PREFIX of `module` that
    // matches the head of `tail`, then drop those head segments. Common
    // patterns:
    //   module `complications` + tail `complications/Foo.kt` → drop 1
    //   module `core` + tail `core/utils/DateUtils.kt` → drop 1
    //   module `features/dashboard/app` + tail
    //     `features/dashboard/TodayViewModel.kt` → drop 2 (module's shared
    //     `features/dashboard` prefix mirrors the Kotlin package path)
    let max_n = module.len().min(tail.len());
    for n in (1..=max_n).rev() {
        if module[..n] == tail[..n] {
            tail = &tail[n..];
            break;
        }
    }
    let module_str = module.join("/");
    if tail.is_empty() {
        module_str
    } else {
        format!("{}/{}", module_str, tail.join("/"))
    }
}

// ---- AndroidManifest.xml class reference extraction ----

/// True for the Android manifest filename we special-case.
pub fn is_android_manifest_path(path: &str) -> bool {
    path.ends_with("/AndroidManifest.xml") || path == "AndroidManifest.xml"
}

/// Extract the `package` attribute from the `<manifest>` element.
/// Returns "" when absent (modern AGP uses `namespace` in build.gradle
/// instead; that path isn't handled here yet).
pub fn extract_manifest_package(xml: &str) -> String {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"<manifest[^>]*\bpackage\s*=\s*"([^"]+)""#).unwrap());
    RE.captures(xml)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

/// Extract class FQCNs referenced via `android:name="..."` attributes on
/// `<activity>`, `<service>`, `<receiver>`, `<provider>`, and
/// `<application>` elements. Resolves leading-dot shorthand against the
/// manifest's `package` attribute.
///
/// Filters aggressively to class-like values: the last FQCN segment must
/// start with an uppercase letter and contain at least one lowercase
/// letter (rejects permission constants like `INTERNET` or
/// `ACCESS_COARSE_LOCATION`). Framework-namespace references
/// (`android.*`) are also skipped — they'll never resolve to a repo file.
pub fn extract_android_manifest_refs(xml: &str) -> Vec<String> {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"android:name\s*=\s*"([^"]+)""#).unwrap());
    let pkg = extract_manifest_package(xml);
    let mut out = Vec::new();
    for cap in RE.captures_iter(xml) {
        let raw = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let fqcn = if let Some(rest) = raw.strip_prefix('.') {
            if pkg.is_empty() {
                continue;
            }
            format!("{}.{}", pkg, rest)
        } else if raw.contains('.') {
            if raw.starts_with("android.") {
                continue;
            }
            raw.to_string()
        } else {
            continue;
        };
        let last = fqcn.rsplit('.').next().unwrap_or("");
        let is_class_like = last.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && last.chars().any(|c| c.is_ascii_lowercase());
        if is_class_like {
            out.push(fqcn);
        }
    }
    out.sort();
    out.dedup();
    out
}

// ---- Android test-set directories ----

/// True if `lower_path` lives under one of Android Gradle's test source
/// sets. `lower_path` MUST already be lowercased (caller's invariant —
/// this matches the surrounding `is_test_path` conventions in `enumerate`).
///
/// Patterns covered: `src/test`, `src/androidTest`, `src/testDebug`,
/// `src/testRelease` — the standard Gradle JVM/Android test source-set
/// names. Generic conventions (`__tests__/`, `.test.`, etc.) stay in
/// `enumerate::is_test_path` since they apply across ecosystems.
pub fn is_android_test_path(lower_path: &str) -> bool {
    lower_path.contains("/src/test/")
        || lower_path.contains("/src/androidtest/")
        || lower_path.contains("/src/testdebug/")
        || lower_path.contains("/src/testrelease/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_kotlin_ext_recognizes_kt_and_kts() {
        assert!(is_kotlin_ext(".kt"));
        assert!(is_kotlin_ext(".kts"));
        assert!(!is_kotlin_ext(".java"));
        assert!(!is_kotlin_ext(".ts"));
        assert!(!is_kotlin_ext(""));
    }

    #[test]
    fn kotlin_compact_display_handles_nested_modules() {
        assert_eq!(
            kotlin_compact_display("core/src/main/java/com/ex/Repo.kt"),
            "core/Repo"
        );
        assert_eq!(
            kotlin_compact_display("features/settings/app/src/main/java/com/ex/Foo.kt"),
            "features/settings/app/Foo"
        );
        assert_eq!(kotlin_compact_display("NoSrcPath.kt"), "NoSrcPath");
    }

    #[test]
    fn kotlin_compact_display_handles_multiplatform_layout() {
        // KMP: shared/commonMain/kotlin/...
        assert_eq!(
            kotlin_compact_display("shared/commonMain/kotlin/com/ex/Foo.kt"),
            "shared/Foo"
        );
        assert_eq!(
            kotlin_compact_display("shared/androidMain/kotlin/com/ex/Bar.kt"),
            "shared/Bar"
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

    #[test]
    fn compact_label_path_examples() {
        assert_eq!(
            compact_label_path("core/src/main/java/com/x/shared/core/constants"),
            "core/constants",
            "dedup module-tail duplicate"
        );
        assert_eq!(
            compact_label_path("core/src/main/kotlin/com/x/shared/core/domain/usecase"),
            "core/domain/usecase",
        );
        assert_eq!(
            compact_label_path("complications/src/main/java/com/x/onewearos/complications/MainComplicationService.kt"),
            "complications/MainComplicationService.kt"
        );
        assert_eq!(
            compact_label_path("features/dashboard/app/src/main/java/com/x/one/features/dashboard/TodayViewModel.kt"),
            "features/dashboard/app/TodayViewModel.kt",
            "nested module: tail repeats `features/dashboard` from the module — multi-segment dedup strips it"
        );
        // Single-segment dedup still works.
        assert_eq!(
            compact_label_path("complications/src/main/java/com/x/onewearos/complications/sub/Foo.kt"),
            "complications/sub/Foo.kt",
            "module `complications` + tail starting `complications/sub/Foo.kt` → strip the leading `complications`"
        );
        // No dedup when tail diverges from module path.
        assert_eq!(
            compact_label_path("core/src/main/java/com/x/shared/core/utils/DateUtils.kt"),
            "core/utils/DateUtils.kt",
            "module `core` + tail starting `core/utils/...` → strip the leading `core`"
        );
        // Non-Gradle paths pass through unchanged.
        assert_eq!(
            compact_label_path("src/components/Form.tsx"),
            "src/components/Form.tsx"
        );
        assert_eq!(compact_label_path("README.md"), "README.md");
        // Path that ends in module boilerplate returns the module alone.
        assert_eq!(
            compact_label_path("core/src/main/java/com/x/shared/core"),
            "core"
        );
    }

    #[test]
    fn android_manifest_refs_resolve_leading_dot_against_package() {
        let xml = r#"<?xml version="1.0"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="com.ex.app">
    <uses-permission android:name="android.permission.INTERNET"/>
    <application android:name=".AresApplication">
        <activity android:name=".MainActivity"/>
        <service android:name="com.ex.app.sync.SyncService"/>
        <receiver android:name="com.ex.ext.Handler"/>
    </application>
</manifest>"#;
        let refs = extract_android_manifest_refs(xml);
        // Leading-dot shorthand gets the manifest's package prefix.
        assert!(refs.contains(&"com.ex.app.AresApplication".to_string()));
        assert!(refs.contains(&"com.ex.app.MainActivity".to_string()));
        // Already-qualified stays intact.
        assert!(refs.contains(&"com.ex.app.sync.SyncService".to_string()));
        assert!(refs.contains(&"com.ex.ext.Handler".to_string()));
        // Permission names (UPPER_CASE last segment) are NOT classes.
        assert!(!refs.iter().any(|s| s.contains("INTERNET")));
        // Framework namespace is skipped.
        assert!(!refs.iter().any(|s| s.starts_with("android.")));
    }

    #[test]
    fn android_manifest_refs_skip_when_no_package() {
        // Modern AGP projects often drop `package=` in favor of
        // `namespace` in build.gradle. Without the package attr, leading-
        // dot refs can't be resolved — skip rather than guess.
        let xml = r#"<manifest>
    <application android:name=".App"/>
</manifest>"#;
        let refs = extract_android_manifest_refs(xml);
        assert!(refs.is_empty(), "got {:?}", refs);
    }

    #[test]
    fn is_android_test_path_recognizes_gradle_dirs() {
        assert!(is_android_test_path("app/src/test/java/com/x/foo.kt"));
        assert!(is_android_test_path(
            "app/src/androidtest/java/com/x/foo.kt"
        ));
        assert!(is_android_test_path("app/src/testdebug/java/com/x/foo.kt"));
        assert!(is_android_test_path(
            "app/src/testrelease/java/com/x/foo.kt"
        ));
        // Generic patterns are NOT in scope here — those live in
        // `enumerate::is_test_path`. This predicate covers Android only.
        assert!(!is_android_test_path("app/src/main/java/com/x/foo.kt"));
        assert!(!is_android_test_path("__tests__/foo.ts"));
    }
}
