//! Heuristic import resolution.
//!
//! Mirrors `internal/resolve/resolve.go`: alias substitution, then relative
//! path math + extension + index-file + `__init__.py` trials. External and
//! unmappable imports stay raw.

use std::collections::{HashMap, HashSet};

use crate::model::FileIndex;

pub type Aliases = HashMap<String, String>;

/// Default extensions to try, in rough order of likelihood.
const CANDIDATE_EXTS: &[&str] = &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".py", ".go", ".kt"];

/// Rewrite `f.imports` in place: resolvable ones become repo-relative paths,
/// unresolvable ones stay raw. Idempotent on already-resolved paths.
pub fn all(files: &mut [FileIndex], aliases: &Aliases) {
    let have: HashSet<String> = files.iter().map(|f| f.path.clone()).collect();
    for f in files.iter_mut() {
        let from = f.path.clone();
        for imp in f.imports.iter_mut() {
            if let Some(resolved) = resolve_one(&from, imp, &have, aliases) {
                *imp = resolved;
            }
        }
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

    fn fi(path: &str) -> FileIndex {
        FileIndex {
            path: path.to_string(),
            ..Default::default()
        }
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
        // After alias rewrite, "src/shared" doesn't start with "." so isn't
        // treated as relative → stays raw. Aliases resolve only when the
        // rewrite produces a relative form — that's Go's behavior too.
        // Documented limitation per design-doc.md.
        assert_eq!(files[0].imports, vec!["@/shared"]);
    }

    #[test]
    fn external_imports_stay_raw() {
        let mut files = vec![fi("src/a.ts")];
        files[0].imports = vec!["react".into(), "@okta/sdk".into()];
        all(&mut files, &HashMap::new());
        assert_eq!(files[0].imports, vec!["react", "@okta/sdk"]);
    }

    #[test]
    fn clean_join_keeps_escaping_dotdot() {
        // Matches Go: filepath.Clean("src/../../up") == "../up"
        assert_eq!(clean_join("src", "../../up"), "../up");
        assert_eq!(clean_join(".", "../../foo"), "../../foo");
        assert_eq!(clean_join("a/b", "../c"), "a/c");
        assert_eq!(clean_join("a/b", "../../../c"), "../c");
    }
}
