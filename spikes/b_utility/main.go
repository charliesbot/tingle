// Spike B — thin tingle prototype.
//
// Emits the compact tag-prefixed format (§Output format in docs/design-doc.md)
// from real tree-sitter parses. Used as contestant (2) in Spike B to validate
// that the proposed format beats `discover` + agent exploration and
// `repomix --compress` on (task quality + tokens).
//
// Build:  go build -ldflags="-s -w" -o spikes/b_utility/tingle ./spikes/b_utility
// Run:    ./spikes/b_utility/tingle <repo-path>
//
// Scope limits (acceptable for a spike; will need attention for v1):
//   - def extraction uses a hand-rolled per-language node-type switch rather
//     than tags.scm queries. Real v1 should use tags.scm for language-agnostic
//     extraction.
//   - heuristic import resolution only: relative path math + extension guessing.
//   - no --alias, no --stdin, no --max-depth, no secret redaction, no warnings.
package main

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"

	sitter "github.com/smacker/go-tree-sitter"
	"github.com/smacker/go-tree-sitter/cpp"
	"github.com/smacker/go-tree-sitter/javascript"
	"github.com/smacker/go-tree-sitter/kotlin"
	"github.com/smacker/go-tree-sitter/typescript/tsx"
	"github.com/smacker/go-tree-sitter/typescript/typescript"
)

// ---------- types ----------

type Def struct {
	Line      int
	Kind      string // "f", "c", "m"
	Signature string // single-line
	Children  []Def  // methods on a class
}

type File struct {
	Path    string
	Ext     string
	Lang    string
	Tags    []string // "M", "test", "untracked"
	Imports []string // raw strings; resolved to repo-paths when heuristic succeeds
	Defs    []Def
	OutDeg  int
	InDeg   int
}

type Edge struct {
	From string // source dir
	To   []string
}

// ---------- language dispatch ----------

type langSpec struct {
	name    string
	grammar *sitter.Language
	// top-level AST node kinds considered defs
	defKinds map[string]string // node_type → ("f"/"c"/"m")
	// methods to walk inside a class body
	classBodyKinds    []string // node types whose children may hold methods
	methodKinds       map[string]string
	importKinds       []string // node types for imports
	importStringField string   // child field name holding the import path
}

var langs = map[string]*langSpec{
	".ts": {
		name:    "ts",
		grammar: typescript.GetLanguage(),
		defKinds: map[string]string{
			"function_declaration":  "f",
			"class_declaration":     "c",
			"interface_declaration": "c",
			"type_alias_declaration": "c",
			"enum_declaration":      "c",
		},
		classBodyKinds: []string{"class_body", "interface_body"},
		methodKinds: map[string]string{
			"method_definition":            "m",
			"method_signature":             "m",
			"public_field_definition":      "m",
			"abstract_method_signature":    "m",
		},
		importKinds:       []string{"import_statement"},
		importStringField: "source",
	},
	".tsx": {
		name:    "tsx",
		grammar: tsx.GetLanguage(),
		defKinds: map[string]string{
			"function_declaration":  "f",
			"class_declaration":     "c",
			"interface_declaration": "c",
			"type_alias_declaration": "c",
			"enum_declaration":      "c",
		},
		classBodyKinds: []string{"class_body", "interface_body"},
		methodKinds: map[string]string{
			"method_definition": "m",
		},
		importKinds:       []string{"import_statement"},
		importStringField: "source",
	},
	".js":  jsSpec(),
	".jsx": jsSpec(),
	".mjs": jsSpec(),
	".kt": {
		name:    "kt",
		grammar: kotlin.GetLanguage(),
		defKinds: map[string]string{
			"function_declaration": "f",
			"class_declaration":    "c",
			"object_declaration":   "c",
		},
		classBodyKinds: []string{"class_body"},
		methodKinds: map[string]string{
			"function_declaration": "m",
		},
		importKinds:       []string{"import_header"},
		importStringField: "",
	},
	".kts": {
		name:    "kts",
		grammar: kotlin.GetLanguage(),
		defKinds: map[string]string{
			"function_declaration": "f",
			"class_declaration":    "c",
		},
		classBodyKinds:    []string{"class_body"},
		methodKinds:       map[string]string{"function_declaration": "m"},
		importKinds:       []string{"import_header"},
		importStringField: "",
	},
	".cc":  cppSpec(),
	".cpp": cppSpec(),
	".cxx": cppSpec(),
	".h":   cppSpec(),
	".hpp": cppSpec(),
	".hxx": cppSpec(),
}

func jsSpec() *langSpec {
	return &langSpec{
		name:    "js",
		grammar: javascript.GetLanguage(),
		defKinds: map[string]string{
			"function_declaration": "f",
			"class_declaration":    "c",
		},
		classBodyKinds: []string{"class_body"},
		methodKinds: map[string]string{
			"method_definition": "m",
		},
		importKinds:       []string{"import_statement"},
		importStringField: "source",
	}
}

func cppSpec() *langSpec {
	return &langSpec{
		name:    "cpp",
		grammar: cpp.GetLanguage(),
		defKinds: map[string]string{
			"function_definition": "f",
			"class_specifier":     "c",
			"struct_specifier":    "c",
		},
		classBodyKinds: []string{"field_declaration_list"},
		methodKinds: map[string]string{
			"function_definition": "m",
		},
		importKinds:       []string{"preproc_include"},
		importStringField: "",
	}
}

// ---------- enumeration ----------

func enumerateRepo(repo string) ([]*File, error) {
	cmd := exec.Command("git", "-C", repo, "ls-files", "-com", "--exclude-standard")
	out, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("git ls-files: %w", err)
	}
	lines := strings.Split(strings.TrimSpace(string(out)), "\n")

	modified := statusSet(repo, "-m")
	untracked := statusSet(repo, "-o", "--exclude-standard")

	files := make([]*File, 0, len(lines))
	for _, p := range lines {
		if p == "" {
			continue
		}
		ext := strings.ToLower(filepath.Ext(p))
		f := &File{Path: p, Ext: ext}
		if spec, ok := langs[ext]; ok {
			f.Lang = spec.name
		}
		if isTestPath(p) {
			f.Tags = append(f.Tags, "test")
		}
		if modified[p] {
			f.Tags = append(f.Tags, "M")
		}
		if untracked[p] {
			f.Tags = append(f.Tags, "untracked")
		}
		files = append(files, f)
	}
	return files, nil
}

func statusSet(repo string, args ...string) map[string]bool {
	cmd := exec.Command("git", append([]string{"-C", repo, "ls-files"}, args...)...)
	out, err := cmd.Output()
	if err != nil {
		return nil
	}
	set := make(map[string]bool)
	for _, p := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if p != "" {
			set[p] = true
		}
	}
	return set
}

func isTestPath(p string) bool {
	lp := strings.ToLower(p)
	return strings.Contains(lp, ".test.") ||
		strings.Contains(lp, ".spec.") ||
		strings.Contains(lp, "__tests__/") ||
		strings.HasSuffix(lp, "_test.go") ||
		strings.HasPrefix(lp, "tests/") ||
		strings.Contains(lp, "/tests/")
}

// ---------- parse + extract ----------

func parseAll(repo string, files []*File) {
	workers := runtime.NumCPU()
	sem := make(chan struct{}, workers)
	var wg sync.WaitGroup
	var mu sync.Mutex
	for _, f := range files {
		if f.Lang == "" {
			continue
		}
		wg.Add(1)
		sem <- struct{}{}
		go func(f *File) {
			defer wg.Done()
			defer func() { <-sem }()
			data, err := os.ReadFile(filepath.Join(repo, f.Path))
			if err != nil {
				return
			}
			spec := langs[f.Ext]
			parser := sitter.NewParser()
			parser.SetLanguage(spec.grammar)
			tree, err := parser.ParseCtx(context.Background(), nil, data)
			if err != nil || tree == nil {
				return
			}
			defer tree.Close()
			defer parser.Close()

			defs, imports := extract(tree.RootNode(), data, spec)
			mu.Lock()
			f.Defs = defs
			f.Imports = imports
			mu.Unlock()
		}(f)
	}
	wg.Wait()
}

func extract(root *sitter.Node, data []byte, spec *langSpec) ([]Def, []string) {
	var defs []Def
	var imports []string
	n := int(root.NamedChildCount())
	for i := 0; i < n; i++ {
		c := root.NamedChild(i)
		kind := c.Type()

		// import?
		matched := false
		for _, ik := range spec.importKinds {
			if kind == ik {
				imports = append(imports, extractImport(c, data, spec))
				matched = true
				break
			}
		}
		if matched {
			continue
		}

		// direct def (function_declaration, class_declaration, etc.)
		if defKind, ok := spec.defKinds[kind]; ok {
			defs = append(defs, buildDef(c, data, spec, defKind))
			continue
		}
		// arrow/function assigned to const/let at top level: const foo = () => {}
		if fdef, ok := tryExtractFunctionFromDecl(c, data); ok {
			defs = append(defs, fdef)
			continue
		}
		// export_statement wrapping any of the above
		if kind == "export_statement" {
			if dn := firstNamedChild(c); dn != nil {
				if defKind, ok := spec.defKinds[dn.Type()]; ok {
					defs = append(defs, buildDef(dn, data, spec, defKind))
					continue
				}
				if fdef, ok := tryExtractFunctionFromDecl(dn, data); ok {
					defs = append(defs, fdef)
					continue
				}
			}
		}
	}
	return defs, imports
}

// tryExtractFunctionFromDecl handles `const foo = () => {}` and
// `var foo = function() {}` patterns — arrow / function expressions assigned
// to a name. Returns (def, true) on match.
func tryExtractFunctionFromDecl(node *sitter.Node, data []byte) (Def, bool) {
	kind := node.Type()
	if kind != "lexical_declaration" && kind != "variable_declaration" {
		return Def{}, false
	}
	n := int(node.NamedChildCount())
	for i := 0; i < n; i++ {
		d := node.NamedChild(i)
		if d.Type() != "variable_declarator" {
			continue
		}
		nameNode := d.ChildByFieldName("name")
		valueNode := d.ChildByFieldName("value")
		if nameNode == nil || valueNode == nil {
			continue
		}
		vt := valueNode.Type()
		if vt != "arrow_function" && vt != "function_expression" && vt != "function" {
			continue
		}
		name := nameNode.Content(data)
		sig := compactSignature(valueNode, data)
		return Def{
			Line:      int(d.StartPoint().Row) + 1,
			Kind:      "f",
			Signature: strings.TrimSpace(name + " " + sig),
		}, true
	}
	return Def{}, false
}

func firstNamedChild(n *sitter.Node) *sitter.Node {
	if n.NamedChildCount() == 0 {
		return nil
	}
	return n.NamedChild(0)
}

func buildDef(node *sitter.Node, data []byte, spec *langSpec, kind string) Def {
	line := int(node.StartPoint().Row) + 1
	name := findName(node, data)
	sig := compactSignature(node, data)
	def := Def{Line: line, Kind: kind, Signature: fmt.Sprintf("%s %s", name, sig)}

	// walk class body for methods
	for _, bodyKind := range spec.classBodyKinds {
		body := findChild(node, bodyKind)
		if body == nil {
			continue
		}
		m := int(body.NamedChildCount())
		for j := 0; j < m; j++ {
			mc := body.NamedChild(j)
			if methodKind, ok := spec.methodKinds[mc.Type()]; ok {
				mname := findName(mc, data)
				msig := compactSignature(mc, data)
				def.Children = append(def.Children, Def{
					Line:      int(mc.StartPoint().Row) + 1,
					Kind:      methodKind,
					Signature: fmt.Sprintf("%s %s", mname, msig),
				})
			}
		}
	}
	return def
}

func findChild(node *sitter.Node, kind string) *sitter.Node {
	n := int(node.NamedChildCount())
	for i := 0; i < n; i++ {
		c := node.NamedChild(i)
		if c.Type() == kind {
			return c
		}
	}
	return nil
}

// findName pulls the "identifier"-ish child from a def node. Crude but works
// across most languages.
func findName(node *sitter.Node, data []byte) string {
	// try common field names
	for _, fieldName := range []string{"name", "declarator"} {
		if c := node.ChildByFieldName(fieldName); c != nil {
			// for declarators in C/C++, recurse
			if c.Type() == "function_declarator" || c.Type() == "pointer_declarator" {
				if inner := c.ChildByFieldName("declarator"); inner != nil {
					return inner.Content(data)
				}
			}
			return c.Content(data)
		}
	}
	// fallback: first identifier-ish named child
	n := int(node.NamedChildCount())
	for i := 0; i < n; i++ {
		c := node.NamedChild(i)
		t := c.Type()
		if strings.Contains(t, "identifier") || t == "type_identifier" {
			return c.Content(data)
		}
	}
	return "?"
}

// compactSignature returns a tight, single-line signature for a def.
// For functions, params + return. For classes, just the name (children handle
// methods). Best-effort.
func compactSignature(node *sitter.Node, data []byte) string {
	// try to pull params + return type
	var parts []string
	if params := node.ChildByFieldName("parameters"); params != nil {
		parts = append(parts, oneLine(params.Content(data)))
	}
	if params := node.ChildByFieldName("value_parameters"); params != nil {
		parts = append(parts, oneLine(params.Content(data)))
	}
	if ret := node.ChildByFieldName("return_type"); ret != nil {
		parts = append(parts, "-> "+oneLine(strings.TrimPrefix(ret.Content(data), ":")))
	}
	if typ := node.ChildByFieldName("type"); typ != nil && len(parts) > 0 {
		parts = append(parts, "-> "+oneLine(typ.Content(data)))
	}
	if len(parts) == 0 {
		return ""
	}
	return strings.Join(parts, " ")
}

func oneLine(s string) string {
	s = strings.TrimSpace(s)
	s = strings.ReplaceAll(s, "\n", " ")
	s = strings.ReplaceAll(s, "\t", " ")
	for strings.Contains(s, "  ") {
		s = strings.ReplaceAll(s, "  ", " ")
	}
	return s
}

func extractImport(node *sitter.Node, data []byte, spec *langSpec) string {
	if spec.importStringField != "" {
		if s := node.ChildByFieldName(spec.importStringField); s != nil {
			raw := strings.Trim(s.Content(data), "\"'`")
			return raw
		}
	}
	// generic: find the first string-literal or identifier under the import
	return strings.TrimSpace(stripQuotes(node.Content(data)))
}

func stripQuotes(s string) string {
	s = strings.TrimPrefix(s, "import ")
	s = strings.TrimPrefix(s, "#include ")
	s = strings.Trim(s, `"'<>`)
	return s
}

// ---------- resolution (heuristic path math) ----------

func resolveImports(repo string, files []*File) {
	have := map[string]bool{}
	for _, f := range files {
		have[f.Path] = true
	}
	exts := []string{".ts", ".tsx", ".js", ".jsx", ".mjs", ".kt", ".py", ".go", ".cc", ".cpp", ".h", ".hpp"}
	for _, f := range files {
		for i, imp := range f.Imports {
			if !strings.HasPrefix(imp, ".") {
				continue // external or absolute; leave raw
			}
			dir := filepath.Dir(f.Path)
			target := filepath.Clean(filepath.Join(dir, imp))
			// try: target exactly, then with each ext, then as index
			if have[target] {
				f.Imports[i] = target
				continue
			}
			for _, e := range exts {
				if have[target+e] {
					f.Imports[i] = target + e
					break
				}
			}
			for _, e := range exts {
				cand := filepath.Join(target, "index"+e)
				if have[cand] {
					f.Imports[i] = cand
					break
				}
			}
		}
	}
}

// ---------- graph + ranking ----------

// buildGraph returns (dirEdges, callersByFile).
// callersByFile maps a file path to the list of files that import it.
func buildGraph(files []*File) (map[string][]string, map[string][]string) {
	dirEdges := map[string]map[string]bool{}
	for _, f := range files {
		src := filepath.Dir(f.Path)
		for _, imp := range f.Imports {
			if !strings.Contains(imp, "/") {
				continue
			}
			if strings.HasPrefix(imp, "@") || strings.Contains(imp, "://") {
				continue
			}
			dst := filepath.Dir(imp)
			if dst == "" || dst == src {
				continue
			}
			if dirEdges[src] == nil {
				dirEdges[src] = map[string]bool{}
			}
			dirEdges[src][dst] = true
			f.OutDeg++
		}
	}
	fileByPath := map[string]*File{}
	for _, f := range files {
		fileByPath[f.Path] = f
	}
	callersByFile := map[string][]string{}
	for _, f := range files {
		for _, imp := range f.Imports {
			if dst, ok := fileByPath[imp]; ok {
				dst.InDeg++
				callersByFile[imp] = append(callersByFile[imp], f.Path)
			}
		}
	}
	m := map[string][]string{}
	for src, dsts := range dirEdges {
		var out []string
		for d := range dsts {
			out = append(out, d)
		}
		sort.Strings(out)
		m[src] = out
	}
	for p := range callersByFile {
		sort.Strings(callersByFile[p])
	}
	return m, callersByFile
}

var entryFilenames = map[string]bool{
	"main.go":      true,
	"index.ts":     true,
	"index.tsx":    true,
	"index.js":     true,
	"server.ts":    true,
	"server.js":    true,
	"app.ts":       true,
	"app.tsx":      true,
	"cli.ts":       true,
	"manage.py":    true,
	"__main__.py":  true,
}

func scoreEntry(f *File) int {
	score := 0
	base := filepath.Base(f.Path)
	if entryFilenames[base] {
		score += 10
	}
	if strings.HasPrefix(base, "App.") {
		score += 8
	}
	if strings.HasPrefix(f.Path, "cmd/") {
		score += 5
	}
	score += f.OutDeg - f.InDeg
	return score
}

// ---------- render ----------

func render(files []*File, graph map[string][]string, callers map[string][]string) string {
	var b strings.Builder
	b.WriteString("# tingle v0-spike  tokenizer=cl100k_base\n")
	b.WriteString("# legend: EP=entry U=utility M=module-edge F=file  [M]=modified [untracked]=new-unstaged [test]=test-file  [path:line]=def  f=func c=class m=method\n")
	b.WriteString("# usage: EP answers \"where does execution start\"; U answers \"which files are load-bearing utilities\" with exports+callers inline; M shows dir→dir imports; F has per-file signatures with line anchors (use Read(path, line=N) to jump)\n\n")

	// Score for ordering
	type scored struct {
		f     *File
		score int
	}
	var scoredFiles []scored
	for _, f := range files {
		if f.Lang == "" {
			continue
		}
		scoredFiles = append(scoredFiles, scored{f: f, score: scoreEntry(f)})
	}

	// Entry points: top score, capped to 15
	sort.Slice(scoredFiles, func(i, j int) bool { return scoredFiles[i].score > scoredFiles[j].score })
	b.WriteString("## Entry points\n")
	eps := 0
	for _, s := range scoredFiles {
		if eps >= 15 || s.score <= 0 {
			break
		}
		name := firstTopLevelName(s.f)
		if name == "" {
			continue // skip files with no recognizable def — not useful as an EP
		}
		line := 1
		if len(s.f.Defs) > 0 {
			line = s.f.Defs[0].Line
		}
		fmt.Fprintf(&b, "EP %s:%d %s (out=%d in=%d)\n", s.f.Path, line, name, s.f.OutDeg, s.f.InDeg)
		eps++
	}
	b.WriteString("\n")

	// Utilities: every file with in-degree >= 2. Show exports + top callers inline
	// so the agent can answer "what does util X export and who uses it" without
	// jumping to the F section.
	sort.Slice(scoredFiles, func(i, j int) bool { return scoredFiles[i].f.InDeg > scoredFiles[j].f.InDeg })
	b.WriteString("## Utilities\n")
	for _, s := range scoredFiles {
		if s.f.InDeg < 2 {
			break
		}
		cs := callers[s.f.Path]
		callerStr := ""
		if len(cs) > 0 {
			maxShow := 3
			if len(cs) < maxShow {
				maxShow = len(cs)
			}
			callerStr = "  ← " + strings.Join(cs[:maxShow], " ")
			if len(cs) > maxShow {
				callerStr += fmt.Sprintf(" (+%d more)", len(cs)-maxShow)
			}
		}
		fmt.Fprintf(&b, "U %s (in=%d)%s\n", s.f.Path, s.f.InDeg, callerStr)
		for _, d := range s.f.Defs {
			fmt.Fprintf(&b, " %d %s %s\n", d.Line, d.Kind, d.Signature)
			for _, m := range d.Children {
				fmt.Fprintf(&b, "  %d %s %s\n", m.Line, m.Kind, m.Signature)
			}
		}
	}
	b.WriteString("\n")

	// Module graph
	if len(graph) > 0 {
		b.WriteString("## Modules\n")
		var srcs []string
		for s := range graph {
			srcs = append(srcs, s)
		}
		sort.Strings(srcs)
		for _, src := range srcs {
			fmt.Fprintf(&b, "M %s -> %s\n", src, strings.Join(graph[src], " "))
		}
		b.WriteString("\n")
	}

	// Files (compact — skips unparsed + untagged)
	b.WriteString("## Files\n")
	sort.Slice(files, func(i, j int) bool { return files[i].Path < files[j].Path })
	for _, f := range files {
		if f.Lang == "" && len(f.Tags) == 0 {
			continue
		}
		tagStr := ""
		for _, t := range f.Tags {
			tagStr += "[" + t + "]"
		}
		imps := ""
		if len(f.Imports) > 0 {
			imps = "  imp: " + strings.Join(f.Imports, " ")
		}
		fmt.Fprintf(&b, "F %s %s%s\n", f.Path, tagStr, imps)
		for _, d := range f.Defs {
			fmt.Fprintf(&b, " %d %s %s\n", d.Line, d.Kind, d.Signature)
			for _, m := range d.Children {
				fmt.Fprintf(&b, "  %d %s %s\n", m.Line, m.Kind, m.Signature)
			}
		}
	}

	return b.String()
}

func firstTopLevelName(f *File) string {
	if len(f.Defs) == 0 {
		return ""
	}
	s := f.Defs[0].Signature
	// strip trailing "(...)..." to get just the name
	if i := strings.IndexAny(s, "( "); i > 0 {
		return s[:i]
	}
	return s
}

// ---------- main ----------

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: tingle <repo-path>")
		os.Exit(2)
	}
	repo, err := filepath.Abs(os.Args[1])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}

	files, err := enumerateRepo(repo)
	if err != nil {
		fmt.Fprintln(os.Stderr, "enumerate:", err)
		os.Exit(1)
	}
	parseAll(repo, files)
	resolveImports(repo, files)
	graph, callers := buildGraph(files)
	out := render(files, graph, callers)
	fmt.Print(out)
}
