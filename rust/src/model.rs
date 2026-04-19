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
    /// Repo-relative when heuristic-resolvable; else raw.
    pub imports: Vec<String>,

    // rank step
    pub out_deg: u32,
    pub in_deg: u32,
}

/// Working in-memory representation of a parsed repo.
#[derive(Debug, Default)]
pub struct Graph {
    pub files: HashMap<String, FileIndex>,
}
