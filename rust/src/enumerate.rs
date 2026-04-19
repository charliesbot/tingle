//! Produce the initial list of files with activity tags.
//!
//! Primary path: `git ls-files -com --exclude-standard -z` — inherits .gitignore.
//! Fallback: walkdir with a baked-in ignore list for repos without .git.
//! Either path applies `.tingleignore` patterns on top.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use thiserror::Error;
use walkdir::WalkDir;

use crate::model::FileIndex;

#[derive(Debug, Error)]
pub enum EnumerateError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("git ls-files failed: {0}")]
    Git(String),
}

/// Default ignores for the no-git fallback. Kept intentionally small; users
/// add repo-specific excludes via `.tingleignore`.
const DEFAULT_IGNORES: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    ".venv",
    "venv",
    "target",
    ".next",
    "out",
    "coverage",
    ".git",
];

/// Scan a repo and return the list of files with activity tags applied.
/// `FileIndex.defs` / `.imports` stay empty — parse fills them.
pub fn repo(repo_path: &Path) -> Result<Vec<FileIndex>, EnumerateError> {
    let repo_abs = repo_path.canonicalize()?;

    let mut paths = if has_git(&repo_abs) {
        git_ls_files(&repo_abs, &["-com", "--exclude-standard"])?
    } else {
        walk_dir(&repo_abs)?
    };

    // `git ls-files -com` emits the union of -c, -o, -m sets and does NOT
    // dedupe. A tracked-and-modified file appears in both -c and -m, which
    // renders as a duplicate F record downstream. Dedupe while preserving
    // first-seen order.
    {
        let mut seen = HashSet::new();
        paths.retain(|p| seen.insert(p.clone()));
    }

    if let Some(ig) = load_tingleignore(&repo_abs) {
        paths.retain(|p| !matches_gitignore(&ig, p));
    }

    // Run git status queries unconditionally — matches Go parity: the parent
    // repo's .git may be above the given path, so git can still produce
    // modified/untracked status even when `has_git(&repo_abs)` is false.
    let modified: HashSet<String> = git_ls_files(&repo_abs, &["-m"])
        .unwrap_or_default()
        .into_iter()
        .collect();
    let untracked: HashSet<String> = git_ls_files(&repo_abs, &["-o", "--exclude-standard"])
        .unwrap_or_default()
        .into_iter()
        .collect();

    Ok(paths
        .into_iter()
        .map(|p| {
            let mut tags = Vec::new();
            if is_test_path(&p) {
                tags.push("test".to_string());
            }
            if modified.contains(&p) {
                tags.push("M".to_string());
            }
            if untracked.contains(&p) {
                tags.push("untracked".to_string());
            }
            FileIndex {
                ext: lower_ext(&p),
                path: p,
                tags,
                ..Default::default()
            }
        })
        .collect())
}

fn has_git(repo: &Path) -> bool {
    repo.join(".git").exists()
}

fn git_ls_files(repo: &Path, extra_args: &[&str]) -> Result<Vec<String>, EnumerateError> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).arg("ls-files").arg("-z");
    for a in extra_args {
        cmd.arg(a);
    }
    let out = cmd
        .output()
        .map_err(|e| EnumerateError::Git(e.to_string()))?;
    if !out.status.success() {
        return Err(EnumerateError::Git(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    let mut paths = Vec::new();
    for chunk in out.stdout.split(|b| *b == 0) {
        if chunk.is_empty() {
            continue;
        }
        paths.push(String::from_utf8_lossy(chunk).into_owned());
    }
    Ok(paths)
}

fn walk_dir(repo: &Path) -> Result<Vec<String>, EnumerateError> {
    let mut out = Vec::new();
    let walker = WalkDir::new(repo).into_iter().filter_entry(|e| {
        let Ok(rel) = e.path().strip_prefix(repo) else {
            return true;
        };
        if rel.as_os_str().is_empty() {
            return true;
        }
        for ig in DEFAULT_IGNORES {
            if rel.components().next().map(|c| c.as_os_str()) == Some(std::ffi::OsStr::new(ig)) {
                return false;
            }
        }
        true
    });
    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(repo) else {
            continue;
        };
        out.push(normalize_path(rel));
    }
    Ok(out)
}

fn load_tingleignore(repo: &Path) -> Option<Gitignore> {
    let path = repo.join(".tingleignore");
    if !path.exists() {
        return None;
    }
    let mut builder = GitignoreBuilder::new(repo);
    if builder.add(&path).is_some() {
        return None;
    }
    builder.build().ok()
}

fn matches_gitignore(ig: &Gitignore, rel_path: &str) -> bool {
    // `matched_path_or_any_parents` walks ancestors; behavior mirrors
    // sabhiram/go-gitignore's MatchesPath.
    ig.matched_path_or_any_parents(rel_path, false).is_ignore()
}

fn is_test_path(p: &str) -> bool {
    let lp = p.to_ascii_lowercase();
    lp.contains(".test.")
        || lp.contains(".spec.")
        || lp.contains("__tests__/")
        || lp.ends_with("_test.go")
        || lp.starts_with("tests/")
        || lp.contains("/tests/")
        // Android Gradle layout: `src/test/java/...` and `src/androidtest/java/...`.
        || lp.contains("/src/test/")
        || lp.contains("/src/androidtest/")
        || lp.contains("/src/testdebug/")
        || lp.contains("/src/testrelease/")
}

fn lower_ext(p: &str) -> String {
    // Match Go's `filepath.Ext`: starts from the last '.' after the last '/'.
    let base = match p.rsplit_once('/') {
        Some((_, b)) => b,
        None => p,
    };
    match base.rfind('.') {
        Some(i) => base[i..].to_ascii_lowercase(),
        None => String::new(),
    }
}

fn normalize_path(p: &Path) -> String {
    // Normalize to forward slashes to match git's output.
    let mut buf = PathBuf::new();
    for c in p.components() {
        buf.push(c);
    }
    buf.to_string_lossy().replace('\\', "/")
}
