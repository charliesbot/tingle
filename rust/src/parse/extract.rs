//! Language-agnostic extractor: consumes tags.scm captures and produces
//! `Symbol` + import lists. Mirrors the Go `internal/parse.extractOne` logic.

use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;
use tree_sitter::{Node, Query, QueryCursor, StreamingIterator};

use crate::model::{Symbol, SymbolKind};

struct RawDef<'a> {
    query_kind: String,
    name_node: Option<Node<'a>>,
    outer_node: Option<Node<'a>>,
}

pub struct Extracted {
    pub defs: Vec<Symbol>,
    pub imports: Vec<String>,
    /// Dot-separated package name (Kotlin `package` header, etc.).
    pub package: String,
}

pub fn extract_one<'a>(query: &Query, root: Node<'a>, src: &[u8]) -> Extracted {
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, src);

    let mut classes: Vec<RawDef> = Vec::new();
    let mut methods: Vec<RawDef> = Vec::new();
    let mut funcs: Vec<RawDef> = Vec::new();
    let mut imports: Vec<String> = Vec::new();
    let mut seen_import: HashSet<String> = HashSet::new();
    let mut package = String::new();

    while let Some(m) = matches.next() {
        let mut def = RawDef {
            query_kind: String::new(),
            name_node: None,
            outer_node: None,
        };
        for cap in m.captures {
            let name = capture_names[cap.index as usize];
            if name == "name.reference.import" {
                if let Ok(text) = cap.node.utf8_text(src) {
                    let trimmed = trim_import_chars(text);
                    if !trimmed.is_empty() && !seen_import.contains(trimmed) {
                        seen_import.insert(trimmed.to_string());
                        imports.push(trimmed.to_string());
                    }
                }
            } else if name == "name.reference.package" {
                // First package capture wins (there should only be one).
                if package.is_empty() {
                    if let Ok(text) = cap.node.utf8_text(src) {
                        package = text.trim().to_string();
                    }
                }
            } else if let Some(rest) = name.strip_prefix("name.definition.") {
                def.name_node = Some(cap.node);
                // Retain the definition kind from the sibling "definition.*" capture;
                // if only name.definition.* fires without a definition.* capture, fall
                // back to `rest`. (Shouldn't happen with aider-style queries but stays
                // safe.)
                if def.query_kind.is_empty() {
                    def.query_kind = rest.to_string();
                }
            } else if let Some(rest) = name.strip_prefix("definition.") {
                def.outer_node = Some(cap.node);
                def.query_kind = rest.to_string();
            }
        }

        let (Some(_name_node), Some(_outer_node)) = (def.name_node, def.outer_node) else {
            continue;
        };
        match def.query_kind.as_str() {
            "method" => methods.push(def),
            "class" | "interface" | "enum" | "type" | "object" | "module" => classes.push(def),
            _ => funcs.push(def),
        }
    }

    let mut class_syms: Vec<Symbol> = Vec::with_capacity(classes.len());
    let mut attached = vec![false; methods.len()];
    for c in &classes {
        let mut cs = build_symbol(
            &c.query_kind,
            c.name_node.unwrap(),
            c.outer_node.unwrap(),
            src,
            SymbolKind::Class,
        );
        for (i, m) in methods.iter().enumerate() {
            if contains(c.outer_node.unwrap(), m.outer_node.unwrap()) {
                cs.children.push(build_symbol(
                    &m.query_kind,
                    m.name_node.unwrap(),
                    m.outer_node.unwrap(),
                    src,
                    SymbolKind::Method,
                ));
                attached[i] = true;
            }
        }
        class_syms.push(cs);
    }

    let mut func_syms: Vec<Symbol> = Vec::with_capacity(funcs.len() + methods.len());
    for f in &funcs {
        func_syms.push(build_symbol(
            &f.query_kind,
            f.name_node.unwrap(),
            f.outer_node.unwrap(),
            src,
            SymbolKind::Func,
        ));
    }
    for (i, m) in methods.iter().enumerate() {
        if !attached[i] {
            func_syms.push(build_symbol(
                &m.query_kind,
                m.name_node.unwrap(),
                m.outer_node.unwrap(),
                src,
                SymbolKind::Func,
            ));
        }
    }

    let mut all = class_syms;
    all.extend(func_syms);
    all.sort_by_key(|s| s.line);
    Extracted {
        defs: all,
        imports,
        package,
    }
}

fn build_symbol(
    query_kind: &str,
    name_node: Node,
    outer_node: Node,
    src: &[u8],
    fallback: SymbolKind,
) -> Symbol {
    let name = name_node.utf8_text(src).unwrap_or_default().to_string();
    let sig = render_signature(&name, name_node, outer_node, src);
    Symbol {
        name,
        kind: kind_from_query(query_kind, fallback),
        signature: sig,
        line: decl_start_row(outer_node) + 1,
        children: Vec::new(),
    }
}

/// Returns the start row of the "logical" declaration.
///
/// Two Kotlin workarounds for the kotlin-ng grammar's annotation handling,
/// both keep the reported line matching Go's gotreesitter output (and aider's
/// convention: the declaration starts at its first annotation):
///
/// 1. Nested `annotated_expression` wrapping `infix_expression` — the
///    misparse of `@A @B private fun Foo() { ... }`. Walks up to the
///    outermost enclosing `annotated_expression`.
/// 2. Preceding sibling `annotated_expression` nodes in front of a
///    `function_declaration` / `class_declaration` / `object_declaration`.
///    Each `@Anno(...)` call-style annotation lands as a sibling rather
///    than being folded into the declaration's modifiers.
fn decl_start_row(mut node: Node) -> u32 {
    if node.kind() == "infix_expression" {
        while let Some(parent) = node.parent() {
            if parent.kind() == "annotated_expression" {
                node = parent;
            } else {
                break;
            }
        }
    }
    if matches!(
        node.kind(),
        "function_declaration" | "class_declaration" | "object_declaration"
    ) && !has_annotation_modifier(node)
    {
        // Only walk back when the declaration's own `modifiers` doesn't
        // include annotation nodes — otherwise the annotations that
        // semantically belong to THIS decl are already covered, and walking
        // back would cross into a preceding (possibly misparsed) decl's
        // annotations, which would report the wrong line.
        while let Some(prev) = node.prev_sibling() {
            if prev.kind() == "annotated_expression" {
                node = prev;
            } else {
                break;
            }
        }
    }
    node.start_position().row as u32
}

fn has_annotation_modifier(node: Node) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut c2 = child.walk();
            for grand in child.children(&mut c2) {
                if grand.kind() == "annotation" {
                    return true;
                }
            }
        }
    }
    false
}

fn kind_from_query(q: &str, fallback: SymbolKind) -> SymbolKind {
    match q {
        "function" => SymbolKind::Func,
        "class" | "object" | "module" => SymbolKind::Class,
        "interface" => SymbolKind::Interface,
        "method" => SymbolKind::Method,
        "type" => SymbolKind::Type,
        "enum" => SymbolKind::Enum,
        _ => fallback,
    }
}

/// Mirrors Go `renderSignature`: returns `"name (params) -> return"`
/// best-effort, single-line.
fn render_signature(name: &str, name_node: Node, outer_node: Node, src: &[u8]) -> String {
    let start = name_node.end_byte();
    let end = outer_node.end_byte();
    if start >= src.len() || end <= start {
        return name.to_string();
    }
    const MAX_TAIL: usize = 400;
    let end = (start + MAX_TAIL).min(end);
    let mut tail = String::from_utf8_lossy(&src[start..end]).to_string();
    for stop in ["{", "=>", " where ", ";", "\n\n"] {
        if let Some(i) = tail.find(stop) {
            tail.truncate(i);
        }
    }
    tail = COLLAPSE_WS.replace_all(&tail, " ").into_owned();
    tail = tail.trim().to_string();
    if let Some(rest) = tail.strip_prefix("= ") {
        tail = rest.to_string();
    }
    if let Some(idx) = tail.rfind(')') {
        if idx < tail.len() - 1 {
            let after = tail[idx + 1..].trim_start();
            if let Some(rest) = after.strip_prefix(':') {
                tail = format!("{}) -> {}", &tail[..idx], rest.trim());
            }
        }
    }
    if tail.is_empty() {
        return name.to_string();
    }
    const MAX_SIG: usize = 180;
    if tail.chars().count() > MAX_SIG {
        let truncated: String = tail.chars().take(MAX_SIG).collect();
        tail = format!("{}…", truncated);
    }
    format!("{} {}", name, tail)
}

static COLLAPSE_WS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());

fn contains(outer: Node, inner: Node) -> bool {
    inner.start_byte() >= outer.start_byte() && inner.end_byte() <= outer.end_byte()
}

fn trim_import_chars(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '<' | '>'))
}
