//! Vue / Nuxt ecosystem special-casing.
//!
//! Vue SFC files bundle three languages in one: `<script>` holds JS/TS,
//! `<template>` holds HTML-ish markup referencing components, `<style>`
//! holds CSS. Tingle hands the script block to the existing TS grammar
//! and scans the template here for `<PascalCase>` component refs.
//!
//! Auto-register convention: `<Foo />` in a template resolves to
//! `components/Foo.vue` without an explicit `import` — Nuxt registers
//! the whole `components/` tree, and userland Vue apps commonly wire the
//! same through `unplugin-vue-components`. The component index
//! (name → path) backfills those graph edges the same way
//! `jvm::resolve_same_package_ref` handles Kotlin package peers.
//!
//! No `tree-sitter-vue` dependency. The upstream grammar
//! (`ikatyang/tree-sitter-vue`) was archived years ago and the Rust
//! crate never left 0.0.x — section splitting is a small regex pass,
//! and `<script>` contents parse cleanly with the TS grammar we already
//! ship.

use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::model::FileIndex;

/// True for Vue SFC extensions.
pub fn is_vue_ext(ext: &str) -> bool {
    ext == ".vue"
}

/// True for markdown extensions that may carry inline `<PascalCase />`
/// component references — Slidev / VuePress / VitePress / Nuxt content
/// for `.md`, MDX-style component embedding for `.mdx`. React MDX is out
/// of scope (the resolver only matches against the Vue component index).
pub fn is_markdown_ext(ext: &str) -> bool {
    matches!(ext, ".md" | ".mdx")
}

/// Sections extracted from a Vue SFC. Missing sections come back empty.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SfcSections {
    /// Contents of every `<script>` / `<script setup>` block, concatenated
    /// with a newline between them. Vue 3 allows one `<script>` + one
    /// `<script setup>` in the same file — both contribute imports, so
    /// we parse the union.
    pub script: String,
    /// `"ts"` when any script block declares `lang="ts"`; `"js"` otherwise
    /// (including the no-script case, where the value is unused).
    pub script_lang: String,
    /// Contents of the first `<template>` block.
    pub template: String,
}

/// Split a Vue SFC into its `<script>` + `<template>` sections. The
/// `<style>` block is dropped — tingle doesn't parse styles for any
/// language. Content outside the recognised blocks is discarded.
///
/// Regex-based, not grammar-backed: valid SFCs can't nest a `<script>`
/// inside another `<script>`, so a DOTALL match on
/// `<script[^>]*>...</script>` is unambiguous for real files.
pub fn split_sfc(src: &str) -> SfcSections {
    static SCRIPT_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)<script([^>]*)>(.*?)</script>").unwrap());
    static TEMPLATE_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)<template([^>]*)>(.*?)</template>").unwrap());
    static LANG_TS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"lang\s*=\s*['"]ts['"]"#).unwrap());

    let mut script = String::new();
    let mut script_lang = String::new();
    for cap in SCRIPT_RE.captures_iter(src) {
        let attrs = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let body = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        if LANG_TS_RE.is_match(attrs) {
            script_lang = "ts".into();
        }
        if !script.is_empty() {
            script.push('\n');
        }
        script.push_str(body);
    }
    if script_lang.is_empty() && !script.is_empty() {
        script_lang = "js".into();
    }

    let template = TEMPLATE_RE
        .captures(src)
        .and_then(|c| c.get(2))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();

    SfcSections {
        script,
        script_lang,
        template,
    }
}

/// Extract PascalCase component references from a Vue template block.
///
/// Matches opening tags shaped like `<PascalCase`. Standard HTML elements
/// start lowercase and are skipped; Vue built-ins that happen to start
/// uppercase (`<Transition>`, `<KeepAlive>`) pass through — they won't
/// match any file in the repo, so the resolver silently drops them.
pub fn extract_template_refs(template: &str) -> Vec<String> {
    static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<([A-Z][A-Za-z0-9]*)").unwrap());
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for cap in TAG_RE.captures_iter(template) {
        if let Some(m) = cap.get(1) {
            let name = m.as_str();
            if seen.insert(name.to_string()) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// Extract PascalCase component references from a Markdown / MDX file.
///
/// Strips fenced code blocks, inline code spans, and HTML comments
/// before scanning, so embedded examples (a `<Badge />` shown verbatim
/// in docs, a fenced code sample, or a commented-out tag) don't fire
/// false positives. The remaining content is fed to
/// `extract_template_refs`.
pub fn extract_markdown_component_refs(src: &str) -> Vec<String> {
    static FENCE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)```.*?```").unwrap());
    static INLINE_CODE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`[^`\n]*`").unwrap());
    static COMMENT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
    let stripped = FENCE_RE.replace_all(src, "");
    let stripped = INLINE_CODE_RE.replace_all(&stripped, "");
    let stripped = COMMENT_RE.replace_all(&stripped, "");
    extract_template_refs(&stripped)
}

/// Derive a Vue component name from a `.vue` file path. Returns `None`
/// if the filename stem doesn't start with an uppercase letter.
///
/// Vue's convention is PascalCase filenames → PascalCase component
/// registration. Kebab-case filenames (`nav-bar.vue` → `<NavBar />`)
/// aren't handled here — they need PascalCase conversion plus matching
/// in `extract_template_refs`. Out of scope for v1; adding later is
/// additive.
pub fn component_name_from_path(path: &str) -> Option<String> {
    let base = path.rsplit_once('/').map(|(_, b)| b).unwrap_or(path);
    let stem = base.strip_suffix(".vue")?;
    if stem.chars().next()?.is_ascii_uppercase() {
        Some(stem.to_string())
    } else {
        None
    }
}

/// Build a `component_name → repo_path` index across all `.vue` files.
/// Collisions (two `Badge.vue` files) resolve first-file-wins, matching
/// the Kotlin index's stability convention. File order is deterministic
/// per run (git ls-files sort order).
pub fn build_component_index(files: &[FileIndex]) -> HashMap<String, String> {
    let mut idx = HashMap::new();
    for f in files {
        if !is_vue_ext(&f.ext) {
            continue;
        }
        if let Some(name) = component_name_from_path(&f.path) {
            idx.entry(name).or_insert_with(|| f.path.clone());
        }
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_vue_ext_recognizes_vue_only() {
        assert!(is_vue_ext(".vue"));
        assert!(!is_vue_ext(".ts"));
        assert!(!is_vue_ext(""));
    }

    #[test]
    fn is_markdown_ext_recognizes_md_and_mdx() {
        assert!(is_markdown_ext(".md"));
        assert!(is_markdown_ext(".mdx"));
        assert!(!is_markdown_ext(".markdown"));
        assert!(!is_markdown_ext(".vue"));
        assert!(!is_markdown_ext(""));
    }

    #[test]
    fn extract_markdown_refs_strips_fenced_code() {
        let src = r#"# Title

<Badge label="hi" />

```vue
<NotARealRef />
```

<Callout>see above</Callout>
"#;
        let refs = extract_markdown_component_refs(src);
        assert!(refs.contains(&"Badge".to_string()));
        assert!(refs.contains(&"Callout".to_string()));
        assert!(
            !refs.contains(&"NotARealRef".to_string()),
            "fenced code leaked: {:?}",
            refs
        );
    }

    #[test]
    fn extract_markdown_refs_strips_inline_code() {
        let src = "Use `<Badge />` like this: <Callout />.";
        let refs = extract_markdown_component_refs(src);
        assert!(refs.contains(&"Callout".to_string()));
        assert!(
            !refs.contains(&"Badge".to_string()),
            "inline code leaked: {:?}",
            refs
        );
    }

    #[test]
    fn extract_markdown_refs_strips_html_comments() {
        let src = "<!-- <Hidden /> --> <Visible />";
        let refs = extract_markdown_component_refs(src);
        assert_eq!(refs, vec!["Visible"]);
    }

    #[test]
    fn extract_markdown_refs_handles_slidev_separators() {
        // Slidev separates slides with `---` lines. Frontmatter has no
        // tags. Component refs across slides should all surface.
        let src = r#"---
layout: cover
---

# Slide 1
<Badge />

---
layout: section
---

# Slide 2
<Callout />
"#;
        let refs = extract_markdown_component_refs(src);
        assert!(refs.contains(&"Badge".to_string()));
        assert!(refs.contains(&"Callout".to_string()));
    }

    #[test]
    fn split_sfc_extracts_script_and_template() {
        let src = r#"<template>
  <Badge label="hi" />
  <Callout />
</template>

<script setup lang="ts">
import Badge from './Badge.vue'
import Callout from './Callout.vue'
</script>

<style scoped>
.foo { color: red; }
</style>"#;
        let s = split_sfc(src);
        assert!(s.script.contains("import Badge"));
        assert!(s.script.contains("import Callout"));
        assert_eq!(s.script_lang, "ts");
        assert!(s.template.contains("<Badge"));
        assert!(s.template.contains("<Callout"));
        // `<style>` contents must not leak into the script or template.
        assert!(!s.script.contains("color: red"));
        assert!(!s.template.contains("color: red"));
    }

    #[test]
    fn split_sfc_concats_two_script_blocks() {
        let src = r#"<script>
export default { name: 'Foo' }
</script>
<script setup>
import Bar from './Bar.vue'
</script>"#;
        let s = split_sfc(src);
        assert!(s.script.contains("export default"));
        assert!(s.script.contains("import Bar"));
        assert_eq!(s.script_lang, "js");
    }

    #[test]
    fn split_sfc_script_lang_defaults_to_js() {
        let src = r#"<script>
import Foo from './Foo.vue'
</script>"#;
        assert_eq!(split_sfc(src).script_lang, "js");
    }

    #[test]
    fn split_sfc_handles_single_quoted_lang_ts() {
        let src = r#"<script setup lang='ts'>import X from './X.vue'</script>"#;
        assert_eq!(split_sfc(src).script_lang, "ts");
    }

    #[test]
    fn split_sfc_empty_when_no_blocks() {
        let s = split_sfc("<div>plain html</div>");
        assert!(s.script.is_empty());
        assert!(s.template.is_empty());
        assert_eq!(s.script_lang, "");
    }

    #[test]
    fn extract_template_refs_pascal_case_only() {
        let t = r#"<div>
  <p>plain</p>
  <Badge />
  <Callout variant="warning">text</Callout>
  <Badge />
  <button class="x">btn</button>
</div>"#;
        let refs = extract_template_refs(t);
        assert_eq!(refs, vec!["Badge", "Callout"]);
    }

    #[test]
    fn extract_template_refs_handles_self_closing_and_nested() {
        let t = r#"<Spacer height="2" />
<Callout :type="'warning'">
  <template #icon><Icon name="alert" /></template>
</Callout>"#;
        let refs = extract_template_refs(t);
        assert!(refs.contains(&"Spacer".to_string()));
        assert!(refs.contains(&"Callout".to_string()));
        assert!(refs.contains(&"Icon".to_string()));
    }

    #[test]
    fn component_name_from_path_requires_pascal_case_stem() {
        assert_eq!(
            component_name_from_path("src/components/Badge.vue"),
            Some("Badge".to_string())
        );
        assert_eq!(component_name_from_path("Foo.vue"), Some("Foo".to_string()));
        // kebab-case filenames: out of scope in v1.
        assert_eq!(component_name_from_path("components/nav-bar.vue"), None);
        // Non-.vue paths.
        assert_eq!(component_name_from_path("foo.ts"), None);
    }

    #[test]
    fn build_component_index_first_file_wins() {
        let files = vec![
            FileIndex {
                path: "src/components/Badge.vue".into(),
                ext: ".vue".into(),
                ..Default::default()
            },
            FileIndex {
                path: "src/dup/Badge.vue".into(),
                ext: ".vue".into(),
                ..Default::default()
            },
            FileIndex {
                path: "src/utils.ts".into(),
                ext: ".ts".into(),
                ..Default::default()
            },
        ];
        let idx = build_component_index(&files);
        assert_eq!(
            idx.get("Badge"),
            Some(&"src/components/Badge.vue".to_string())
        );
        assert_eq!(idx.len(), 1);
    }
}
