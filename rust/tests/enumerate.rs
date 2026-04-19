//! Integration tests for `enumerate::repo`. Validates activity tags,
//! `.tingleignore` semantics, and no-git fallback against fresh tempdirs.

use std::fs;
use std::process::Command;

use tempfile::TempDir;
use tingle::enumerate;

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let full = root.join(rel);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(full, body).unwrap();
}

fn git(repo: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&status.stderr)
    );
}

fn paths_of(files: &[tingle::model::FileIndex]) -> Vec<&str> {
    files.iter().map(|f| f.path.as_str()).collect()
}

fn find<'a>(files: &'a [tingle::model::FileIndex], p: &str) -> &'a tingle::model::FileIndex {
    files.iter().find(|f| f.path == p).expect(p)
}

#[test]
fn no_git_fallback_applies_default_ignores() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(root, "src/a.ts", "");
    write(root, "node_modules/foo/index.js", "");
    write(root, "dist/bundle.js", "");
    write(root, "README.md", "");

    let files = enumerate::repo(root).unwrap();
    let paths = paths_of(&files);
    assert!(paths.contains(&"src/a.ts"), "got: {:?}", paths);
    assert!(paths.contains(&"README.md"), "got: {:?}", paths);
    assert!(!paths.iter().any(|p| p.starts_with("node_modules/")));
    assert!(!paths.iter().any(|p| p.starts_with("dist/")));
}

#[test]
fn tingleignore_trims_paths() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(root, "a.ts", "");
    write(root, "docs/keep.md", "");
    write(root, "docs/skip.md", "");
    write(root, ".tingleignore", "docs/skip.md\n");

    let files = enumerate::repo(root).unwrap();
    let paths: Vec<&str> = paths_of(&files);
    assert!(paths.contains(&"a.ts"));
    assert!(paths.contains(&"docs/keep.md"));
    assert!(!paths.contains(&"docs/skip.md"), "got: {:?}", paths);
}

#[test]
fn test_tag_heuristics() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(root, "src/foo.test.ts", "");
    write(root, "src/bar.spec.tsx", "");
    write(root, "src/__tests__/baz.ts", "");
    write(root, "cmd/main_test.go", "");
    write(root, "tests/integration.py", "");
    write(root, "src/tests/unit.py", "");
    // Android Gradle convention: `<module>/src/test/java/...` and
    // `<module>/src/androidTest/java/...`
    write(root, "app/src/test/java/com/ex/FooTest.kt", "");
    write(root, "app/src/androidTest/java/com/ex/BarTest.kt", "");
    write(root, "src/app.ts", "");

    let files = enumerate::repo(root).unwrap();
    assert!(find(&files, "src/foo.test.ts")
        .tags
        .contains(&"test".into()));
    assert!(find(&files, "src/bar.spec.tsx")
        .tags
        .contains(&"test".into()));
    assert!(find(&files, "src/__tests__/baz.ts")
        .tags
        .contains(&"test".into()));
    assert!(find(&files, "cmd/main_test.go")
        .tags
        .contains(&"test".into()));
    assert!(find(&files, "tests/integration.py")
        .tags
        .contains(&"test".into()));
    assert!(find(&files, "src/tests/unit.py")
        .tags
        .contains(&"test".into()));
    assert!(find(&files, "app/src/test/java/com/ex/FooTest.kt")
        .tags
        .contains(&"test".into()));
    assert!(find(&files, "app/src/androidTest/java/com/ex/BarTest.kt")
        .tags
        .contains(&"test".into()));
    assert!(!find(&files, "src/app.ts").tags.contains(&"test".into()));
}

#[test]
fn git_backed_tags_modified_and_untracked() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(root, "a.ts", "// v1\n");
    write(root, "b.ts", "// v1\n");
    write(root, "c.ts", "// v1\n");
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "a.ts", "b.ts"]);
    git(root, &["commit", "-qm", "init"]);
    // Modify a.ts, leave b.ts, c.ts untracked.
    fs::write(root.join("a.ts"), "// v2\n").unwrap();

    let files = enumerate::repo(root).unwrap();
    let a = find(&files, "a.ts");
    let b = find(&files, "b.ts");
    let c = find(&files, "c.ts");
    assert!(a.tags.contains(&"M".into()), "a tags: {:?}", a.tags);
    assert!(!b.tags.contains(&"M".into()));
    assert!(!b.tags.contains(&"untracked".into()));
    assert!(c.tags.contains(&"untracked".into()), "c tags: {:?}", c.tags);
}

#[test]
fn modified_tracked_file_appears_once() {
    // `git ls-files -com` emits the union without deduping; a tracked file
    // that's been modified appears in both the -c and -m sets. Regression
    // test for the duplicate-F-record bug.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(root, "a.ts", "// v1\n");
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "a.ts"]);
    git(root, &["commit", "-qm", "init"]);
    fs::write(root.join("a.ts"), "// v2\n").unwrap();

    let files = enumerate::repo(root).unwrap();
    let a_count = files.iter().filter(|f| f.path == "a.ts").count();
    assert_eq!(a_count, 1, "a.ts appeared {} times", a_count);
}

#[test]
fn ext_lowercased_with_dot() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(root, "a.TS", "");
    write(root, "README", "");

    let files = enumerate::repo(root).unwrap();
    assert_eq!(find(&files, "a.TS").ext, ".ts");
    assert_eq!(find(&files, "README").ext, "");
}
