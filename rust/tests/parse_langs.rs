//! Per-language extraction tests. Mirrors `internal/parse/parse_test.go` but
//! tightened: Rust uses canonical C tree-sitter, so the two gotreesitter
//! grammar gaps (Kotlin `object_declaration`, Python f-string-followed-by-def)
//! close — `UserModule` and `read_lines` are now required captures.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use tingle::model::FileIndex;
use tingle::parse::{self, Stats};

fn fixture_dir() -> PathBuf {
    // repo-root/testdata/fixtures/langs — tests run with CWD = rust/
    Path::new("../testdata/fixtures/langs")
        .canonicalize()
        .unwrap()
}

fn run(file: &str, ext: &str) -> FileIndex {
    // Use a per-test Stats so parallel test execution doesn't race on
    // PACKAGE_STATS counters.
    let stats = Stats::default();
    let f = FileIndex {
        path: file.to_string(),
        ext: ext.to_string(),
        ..Default::default()
    };
    let mut files = vec![f];
    parse::all(&fixture_dir(), &mut files, &stats);

    assert_eq!(
        0,
        stats.parse_errors.load(Ordering::Relaxed),
        "parse errors on {}",
        file
    );
    files.remove(0)
}

fn def_names(f: &FileIndex) -> Vec<String> {
    let mut out = Vec::new();
    for d in &f.defs {
        out.push(d.name.clone());
        for c in &d.children {
            out.push(c.name.clone());
        }
    }
    out
}

fn contains_def(f: &FileIndex, name: &str) -> bool {
    def_names(f).iter().any(|n| n == name)
}

#[test]
fn typescript_utils() {
    let f = run("utils.ts", ".ts");
    let expected = [
        "getInputLines",
        "getParagraphs",
        "readFile",
        "AuthService",
        "Session",
        "AuthServiceImpl",
        "login",
        "logout",
    ];
    for want in expected {
        assert!(
            contains_def(&f, want),
            "missing {}, got: {:?}",
            want,
            def_names(&f)
        );
    }
    assert!(f.defs.len() >= 6, "defs: {:?}", def_names(&f));
}

#[test]
fn go_main() {
    let f = run("main.go", ".go");
    for want in ["main", "Listen", "Server"] {
        assert!(
            contains_def(&f, want),
            "missing {}, got: {:?}",
            want,
            def_names(&f)
        );
    }
    for want in ["fmt", "os", "strings"] {
        assert!(
            f.imports.iter().any(|i| i == want),
            "missing import {}, got: {:?}",
            want,
            f.imports
        );
    }
}

#[test]
fn kotlin_repo_includes_object_declaration() {
    // Canonical C tree-sitter captures `object_declaration` (gotreesitter did
    // not). `UserModule` + its `provide` method are required.
    let f = run("Repo.kt", ".kt");
    for want in [
        "UserRepository",
        "UserRepositoryImpl",
        "UserModule",
        "topLevelHelper",
        "getAll",
        "insert",
        "provide",
    ] {
        assert!(
            contains_def(&f, want),
            "missing {}, got: {:?}",
            want,
            def_names(&f)
        );
    }
    for want in [
        "kotlinx.coroutines.flow.Flow",
        "kotlinx.coroutines.flow.flow",
    ] {
        assert!(
            f.imports.iter().any(|i| i == want),
            "missing import {}",
            want
        );
    }
}

#[test]
fn cpp_reader() {
    let f = run("reader.h", ".h");
    for want in ["FileReader", "Config", "processFile"] {
        assert!(
            contains_def(&f, want),
            "missing {}, got: {:?}",
            want,
            def_names(&f)
        );
    }
    for want in ["string", "vector"] {
        assert!(f.imports.iter().any(|i| i == want), "missing {}", want);
    }
}

#[test]
fn vue_sfc_extracts_script_imports_and_template_refs() {
    // Page.vue: explicit `import Badge from './Badge.vue'` in `<script>`
    // and a `<Callout />` ref in `<template>` with no import (auto-
    // registered, Nuxt/unplugin-vue-components style).
    let f = run("Page.vue", ".vue");
    assert_eq!(f.lang, "vue");
    // Script block delivered to the TS grammar — `./Badge.vue` lands in
    // the import list verbatim.
    assert!(
        f.imports.iter().any(|i| i == "./Badge.vue"),
        "missing ./Badge.vue import, got: {:?}",
        f.imports
    );
    // Template refs (pascal-case tags) populate `refs`, not `imports`.
    assert!(
        f.refs.iter().any(|r| r == "Badge"),
        "missing Badge template ref, got: {:?}",
        f.refs
    );
    assert!(
        f.refs.iter().any(|r| r == "Callout"),
        "missing Callout template ref, got: {:?}",
        f.refs
    );
    assert!(f.loc > 0, "loc not populated");
}

#[test]
fn python_util_includes_read_lines() {
    // Canonical C tree-sitter captures top-level defs after an f-string method
    // body (gotreesitter did not). `read_lines` is required.
    let f = run("util.py", ".py");
    for want in ["Service", "greet", "read_lines", "fetch_data"] {
        assert!(
            contains_def(&f, want),
            "missing {}, got: {:?}",
            want,
            def_names(&f)
        );
    }
    for want in ["os", "typing"] {
        assert!(f.imports.iter().any(|i| i == want), "missing {}", want);
    }
}
