//! Shared types for the tingle pipeline.
//!
//! Pipeline: enumerate → parse → resolve → rank → render.
//! Each stage reads and augments `FileIndex` values through a `Graph`.

use std::collections::HashMap;

/// Rendered kind code (one char keeps the output compact).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Func,
    Class,
    Method,
    Type,
    Interface,
    Enum,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Func => "f",
            SymbolKind::Class => "c",
            SymbolKind::Method => "m",
            SymbolKind::Type => "t",
            SymbolKind::Interface => "i",
            SymbolKind::Enum => "e",
        }
    }
}

/// Top-level definition (or a method nested under a class).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// Single-line signature (e.g. `bootstrap (x: string) -> Promise<void>`).
    pub signature: String,
    /// 1-indexed line number.
    pub line: u32,
    /// Methods under a class; empty for standalone defs.
    pub children: Vec<Symbol>,
}

/// Everything tingle knows about one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileIndex {
    pub path: String,
    pub ext: String,
    /// `"ts"`, `"kt"`, `"go"`, `""` for unsupported.
    pub lang: String,

    // enumerate step
    /// Insertion order: `"test"`, `"M"`, `"untracked"`.
    pub tags: Vec<String>,

    // parse step
    pub defs: Vec<Symbol>,
    /// What renders in the `F` record's `imp:` list. Repo-relative path
    /// for languages where file paths ARE shorter than their native
    /// reference (TS, JS, Python, Go); the original FQCN or a collapsed
    /// prefix for Kotlin, where file paths (`core/src/main/java/...`)
    /// are longer than the canonical reference (`com.foo.Bar`).
    pub imports: Vec<String>,
    /// Resolved repo-relative paths used by `rank` for the graph
    /// (in_deg / out_deg / dir edges / caller lists). One-for-one with
    /// `imports` in positional semantics is NOT required — this is just
    /// the set of resolved edges from this file, duplicates dropped.
    pub resolved_imports: Vec<String>,
    /// Dot-separated package name for languages with explicit package
    /// headers (Kotlin). Empty for languages without one.
    pub package: String,

    // rank step
    pub out_deg: u32,
    pub in_deg: u32,
}

/// Working in-memory representation of a parsed repo.
#[derive(Debug, Default)]
pub struct Graph {
    pub files: HashMap<String, FileIndex>,
}
