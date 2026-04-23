//! Tree-sitter dispatch + extraction.
//!
//! Runs aider-style tags.scm queries against source files to extract
//! definitions and imports. Language-agnostic extractor: per-language work
//! lives in the .scm query files under queries/.

mod extract;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use once_cell::sync::Lazy;
use rayon::prelude::*;
use tree_sitter::{Language, Parser, Query};

use crate::model::FileIndex;

#[derive(Default)]
pub struct Stats {
    pub parsed_ok: AtomicU64,
    pub read_errors: AtomicU64,
    pub parse_errors: AtomicU64,
}

pub static PACKAGE_STATS: Stats = Stats {
    parsed_ok: AtomicU64::new(0),
    read_errors: AtomicU64::new(0),
    parse_errors: AtomicU64::new(0),
};

pub fn new_run() {
    PACKAGE_STATS.parsed_ok.store(0, Ordering::Relaxed);
    PACKAGE_STATS.read_errors.store(0, Ordering::Relaxed);
    PACKAGE_STATS.parse_errors.store(0, Ordering::Relaxed);
}

struct LangDef {
    ext: &'static str,
    name: &'static str,
    language_fn: fn() -> Language,
    query_src: &'static str,
}

const TS_QUERY: &str = include_str!("queries/typescript-tags.scm");
const TSX_QUERY: &str = include_str!("queries/tsx-tags.scm");
const JS_QUERY: &str = include_str!("queries/javascript-tags.scm");
const PY_QUERY: &str = include_str!("queries/python-tags.scm");
const GO_QUERY: &str = include_str!("queries/go-tags.scm");
const KT_QUERY: &str = include_str!("queries/kotlin-tags.scm");
const CPP_QUERY: &str = include_str!("queries/cpp-tags.scm");

fn lang_ts() -> Language {
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
}
fn lang_tsx() -> Language {
    tree_sitter_typescript::LANGUAGE_TSX.into()
}
fn lang_js() -> Language {
    tree_sitter_javascript::LANGUAGE.into()
}
fn lang_py() -> Language {
    tree_sitter_python::LANGUAGE.into()
}
fn lang_go() -> Language {
    tree_sitter_go::LANGUAGE.into()
}
fn lang_kt() -> Language {
    tree_sitter_kotlin_ng::LANGUAGE.into()
}
fn lang_cpp() -> Language {
    tree_sitter_cpp::LANGUAGE.into()
}

/// All registered languages. Extensions include leading dot to match
/// `FileIndex.ext`. Adding a language = drop in a grammar crate, a query
/// file, and one entry here.
const LANG_DEFS: &[LangDef] = &[
    LangDef {
        ext: ".ts",
        name: "ts",
        language_fn: lang_ts,
        query_src: TS_QUERY,
    },
    LangDef {
        ext: ".tsx",
        name: "tsx",
        language_fn: lang_tsx,
        query_src: TSX_QUERY,
    },
    LangDef {
        ext: ".js",
        name: "js",
        language_fn: lang_js,
        query_src: JS_QUERY,
    },
    LangDef {
        ext: ".jsx",
        name: "jsx",
        language_fn: lang_js,
        query_src: JS_QUERY,
    },
    LangDef {
        ext: ".mjs",
        name: "mjs",
        language_fn: lang_js,
        query_src: JS_QUERY,
    },
    LangDef {
        ext: ".py",
        name: "py",
        language_fn: lang_py,
        query_src: PY_QUERY,
    },
    LangDef {
        ext: ".go",
        name: "go",
        language_fn: lang_go,
        query_src: GO_QUERY,
    },
    LangDef {
        ext: ".kt",
        name: "kt",
        language_fn: lang_kt,
        query_src: KT_QUERY,
    },
    LangDef {
        ext: ".kts",
        name: "kts",
        language_fn: lang_kt,
        query_src: KT_QUERY,
    },
    LangDef {
        ext: ".cc",
        name: "cpp",
        language_fn: lang_cpp,
        query_src: CPP_QUERY,
    },
    LangDef {
        ext: ".cpp",
        name: "cpp",
        language_fn: lang_cpp,
        query_src: CPP_QUERY,
    },
    LangDef {
        ext: ".cxx",
        name: "cpp",
        language_fn: lang_cpp,
        query_src: CPP_QUERY,
    },
    LangDef {
        ext: ".h",
        name: "cpp",
        language_fn: lang_cpp,
        query_src: CPP_QUERY,
    },
    LangDef {
        ext: ".hpp",
        name: "cpp",
        language_fn: lang_cpp,
        query_src: CPP_QUERY,
    },
    LangDef {
        ext: ".hxx",
        name: "cpp",
        language_fn: lang_cpp,
        query_src: CPP_QUERY,
    },
];

fn lang_for(ext: &str) -> Option<&'static LangDef> {
    LANG_DEFS.iter().find(|d| d.ext == ext)
}

/// Count lines as `newlines + (1 if file has content and doesn't end in \n)`.
/// Matches POSIX `wc -l` semantics closely enough for F-record display.
fn count_lines(data: &[u8]) -> u32 {
    if data.is_empty() {
        return 0;
    }
    let mut n: u32 = 0;
    for &b in data {
        if b == b'\n' {
            n += 1;
        }
    }
    if data.last() != Some(&b'\n') {
        n += 1;
    }
    n
}

struct CompiledLang {
    language: Language,
    query: Query,
}

/// Per-language compiled Language + Query. Shared across parse workers.
/// Panics on broken queries — that's a build-time bug.
///
/// Cache is keyed by the static `&LangDef` pointer itself, so adding a
/// language to `LANG_DEFS` doesn't require a parallel table update.
fn compiled_for(def: &'static LangDef) -> &'static CompiledLang {
    use std::collections::HashMap;
    use std::sync::RwLock;

    // Cache keyed by the static `LangDef` address (as usize, since raw
    // pointers aren't Send/Sync). Equal keys imply the same grammar.
    static CACHE: Lazy<RwLock<HashMap<usize, &'static CompiledLang>>> =
        Lazy::new(|| RwLock::new(HashMap::new()));

    let key = def as *const LangDef as usize;
    {
        let cache = CACHE.read().unwrap();
        if let Some(c) = cache.get(&key) {
            return c;
        }
    }
    // Under races, two threads can both construct + leak; one loser leak
    // (bounded at once per language) is accepted.
    let language = (def.language_fn)();
    let query = Query::new(&language, def.query_src).unwrap_or_else(|e| {
        panic!(
            "tingle/parse: invalid query for {} ({}): {}",
            def.name, def.ext, e
        )
    });
    let leaked: &'static CompiledLang = Box::leak(Box::new(CompiledLang { language, query }));
    CACHE.write().unwrap().insert(key, leaked);
    leaked
}

/// Parse every file in `files` with a registered language. Files with unknown
/// extensions are left untouched. Populates `lang`, `defs`, `imports`.
///
/// Bumps counters on the supplied `stats`. Tests should pass a fresh
/// `Stats::default()` so parallel test execution doesn't race on
/// `PACKAGE_STATS`.
pub fn all(repo: &Path, files: &mut [FileIndex], stats: &Stats) {
    files.par_iter_mut().for_each(|f| {
        // AndroidManifest.xml gets first-class treatment — it declares
        // runtime entry points (Activities/Services/etc.) that the
        // import-based graph can't see otherwise. Not tree-sitter-driven
        // (XML regex is fine and keeps us off another grammar dep).
        if crate::lang::jvm::is_android_manifest_path(&f.path) {
            let full = repo.join(&f.path);
            let Ok(data) = std::fs::read(&full) else {
                stats.read_errors.fetch_add(1, Ordering::Relaxed);
                return;
            };
            f.loc = count_lines(&data);
            f.lang = "androidManifest".to_string();
            let xml = std::str::from_utf8(&data).unwrap_or("");
            f.imports = crate::lang::jvm::extract_android_manifest_refs(xml);
            stats.parsed_ok.fetch_add(1, Ordering::Relaxed);
            return;
        }

        let Some(def) = lang_for(&f.ext) else { return };
        f.lang = def.name.to_string();
        let compiled = compiled_for(def);

        let full = repo.join(&f.path);
        let data = match std::fs::read(&full) {
            Ok(d) => d,
            Err(_) => {
                stats.read_errors.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };

        let mut parser = Parser::new();
        if parser.set_language(&compiled.language).is_err() {
            stats.parse_errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let Some(tree) = parser.parse(&data, None) else {
            stats.parse_errors.fetch_add(1, Ordering::Relaxed);
            return;
        };

        f.loc = count_lines(&data);

        let extracted = extract::extract_one(&compiled.query, tree.root_node(), &data);
        f.defs = extracted.defs;
        f.imports = extracted.imports;
        f.package = extracted.package;
        f.refs = extracted.refs;
        stats.parsed_ok.fetch_add(1, Ordering::Relaxed);
    });
}
