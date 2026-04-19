// Spike B contestant (0) — regex-only tingle prototype.
//
// Same output format as the tree-sitter prototype (spikes/b_utility), but all
// extraction is done with hand-rolled regex. Purpose: measure how much of the
// tree-sitter prototype's signal we lose by dropping tree-sitter entirely.
//
// Build: go build -ldflags="-s -w" -o spikes/b_regex/tingle-regex ./spikes/b_regex
// Run:   ./spikes/b_regex/tingle-regex <repo-path>
package main

import (
	"bufio"
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

// ---------- shared types (mirror the tree-sitter prototype) ----------

type Def struct {
	Line      int
	Kind      string // "f", "c", "m"
	Signature string
	Children  []Def
}

type File struct {
	Path    string
	Ext     string
	Lang    string
	Tags    []string
	Imports []string
	Defs    []Def
	OutDeg  int
	InDeg   int
}

// ---------- regex patterns per language ----------

type patterns struct {
	// imports: one regex, capture group 1 = raw import string
	imp *regexp.Regexp
	// def patterns: each returns (line, kind, name, signatureTail)
	defs []defPattern
}

type defPattern struct {
	kind string // "f" / "c" / "m"
	re   *regexp.Regexp
	// capture groups: 1=name, optional 2=paramsOrBody
	nameGroup int
	tailGroup int // 0 if no tail
}

var langPatterns = map[string]*patterns{
	".ts":  tsPatterns(),
	".tsx": tsPatterns(),
	".js":  jsPatterns(),
	".jsx": jsPatterns(),
	".mjs": jsPatterns(),
	".kt":  kotlinPatterns(),
	".kts": kotlinPatterns(),
	".cc":  cppPatterns(),
	".cpp": cppPatterns(),
	".cxx": cppPatterns(),
	".h":   cppPatterns(),
	".hpp": cppPatterns(),
	".hxx": cppPatterns(),
	".py":  pyPatterns(),
	".go":  goPatterns(),
	".mdx": mdxPatterns(),
}

func tsPatterns() *patterns {
	return &patterns{
		imp: regexp.MustCompile(`^\s*import\s+(?:[^'"]+\s+from\s+)?['"]([^'"]+)['"]`),
		defs: []defPattern{
			// export function / export async function / function
			{"f", regexp.MustCompile(`^\s*(?:export\s+(?:default\s+)?)?(?:async\s+)?function\s+(\w+)\s*(\([^)]*\)(?:\s*:\s*[^{]+)?)`), 1, 2},
			// const name = async (args): T => ...   also const name = () => ...
			{"f", regexp.MustCompile(`^\s*(?:export\s+)?(?:const|let|var)\s+(\w+)\s*(?::\s*[^=]+)?=\s*(?:async\s+)?(\([^)]*\)(?:\s*:\s*[^=]+?)?)\s*=>`), 1, 2},
			// class
			{"c", regexp.MustCompile(`^\s*(?:export\s+(?:default\s+)?)?(?:abstract\s+)?class\s+(\w+)`), 1, 0},
			// interface
			{"c", regexp.MustCompile(`^\s*(?:export\s+)?interface\s+(\w+)`), 1, 0},
			// type alias
			{"c", regexp.MustCompile(`^\s*(?:export\s+)?type\s+(\w+)`), 1, 0},
			// enum
			{"c", regexp.MustCompile(`^\s*(?:export\s+)?enum\s+(\w+)`), 1, 0},
		},
	}
}

func jsPatterns() *patterns {
	return &patterns{
		imp: regexp.MustCompile(`^\s*(?:import\s+(?:[^'"]+\s+from\s+)?['"]([^'"]+)['"]|(?:const|let|var)\s+\w+\s*=\s*require\(\s*['"]([^'"]+)['"])`),
		defs: []defPattern{
			{"f", regexp.MustCompile(`^\s*(?:export\s+(?:default\s+)?)?(?:async\s+)?function\s+(\w+)\s*(\([^)]*\))`), 1, 2},
			{"f", regexp.MustCompile(`^\s*(?:export\s+)?(?:const|let|var)\s+(\w+)\s*=\s*(?:async\s+)?(\([^)]*\))\s*=>`), 1, 2},
			{"c", regexp.MustCompile(`^\s*(?:export\s+(?:default\s+)?)?class\s+(\w+)`), 1, 0},
		},
	}
}

func kotlinPatterns() *patterns {
	return &patterns{
		imp: regexp.MustCompile(`^\s*import\s+([\w.]+(?:\.\*)?)`),
		defs: []defPattern{
			{"f", regexp.MustCompile(`^\s*(?:(?:public|private|internal|protected|open|abstract|override|suspend|inline|operator|infix|tailrec)\s+)*fun\s+(?:<[^>]+>\s+)?(?:\w+\.)?(\w+)\s*(\([^)]*\)(?:\s*:\s*[\w<>?,\s]+)?)`), 1, 2},
			{"c", regexp.MustCompile(`^\s*(?:(?:public|private|internal|protected|open|abstract|sealed|data|enum|inner)\s+)*class\s+(\w+)`), 1, 0},
			{"c", regexp.MustCompile(`^\s*(?:(?:public|private|internal)\s+)?object\s+(\w+)`), 1, 0},
			{"c", regexp.MustCompile(`^\s*(?:(?:public|private|internal)\s+)?interface\s+(\w+)`), 1, 0},
		},
	}
}

func cppPatterns() *patterns {
	return &patterns{
		imp: regexp.MustCompile(`^\s*#include\s+[<"]([^>"]+)[>"]`),
		defs: []defPattern{
			// crude: ReturnType name(params) {   at top level (no leading whitespace)
			{"f", regexp.MustCompile(`^(?:[\w:*&<>,\s]+\s+)(\w+)\s*(\([^)]*\))\s*(?:const\s*)?(?:noexcept\s*)?\{`), 1, 2},
			{"c", regexp.MustCompile(`^\s*(?:class|struct)\s+(\w+)`), 1, 0},
		},
	}
}

func pyPatterns() *patterns {
	return &patterns{
		imp: regexp.MustCompile(`^\s*(?:from\s+([\w.]+)\s+import|import\s+([\w.]+))`),
		defs: []defPattern{
			{"f", regexp.MustCompile(`^\s*(?:async\s+)?def\s+(\w+)\s*(\([^)]*\)(?:\s*->\s*[^:]+)?)`), 1, 2},
			{"c", regexp.MustCompile(`^\s*class\s+(\w+)(?:\s*\([^)]*\))?`), 1, 0},
		},
	}
}

func goPatterns() *patterns {
	return &patterns{
		imp: regexp.MustCompile(`^\s*(?:import\s+["]([^"]+)["]|^\s+["]([^"]+)["])`), // single or inside import block
		defs: []defPattern{
			{"f", regexp.MustCompile(`^\s*func\s+(?:\(\w+\s+\*?\w+\)\s+)?(\w+)\s*(\([^)]*\)(?:\s*[^{]+)?)`), 1, 2},
			{"c", regexp.MustCompile(`^\s*type\s+(\w+)\s+(?:struct|interface|func)`), 1, 0},
		},
	}
}

func mdxPatterns() *patterns {
	return &patterns{
		imp: regexp.MustCompile(`^\s*import\s+(?:[^'"]+\s+from\s+)?['"]([^'"]+)['"]`),
		defs: []defPattern{
			{"f", regexp.MustCompile(`^\s*export\s+(?:const|let|function|async\s+function)\s+(\w+)`), 1, 0},
		},
	}
}

// ---------- extraction ----------

func extract(data []byte, pat *patterns) ([]Def, []string) {
	var defs []Def
	var imports []string
	sc := bufio.NewScanner(bytes.NewReader(data))
	sc.Buffer(make([]byte, 1024*1024), 1024*1024)
	lineno := 0
	for sc.Scan() {
		lineno++
		line := sc.Text()
		// skip obvious comment lines
		trimmed := strings.TrimLeft(line, " \t")
		if strings.HasPrefix(trimmed, "//") || strings.HasPrefix(trimmed, "#") && !strings.HasPrefix(trimmed, "#include") {
			continue
		}
		if strings.HasPrefix(trimmed, "*") || strings.HasPrefix(trimmed, "/*") {
			continue
		}

		// imports
		if pat.imp != nil {
			if m := pat.imp.FindStringSubmatch(line); m != nil {
				for _, g := range m[1:] {
					if g != "" {
						imports = append(imports, g)
						break
					}
				}
				continue
			}
		}

		// defs
		for _, dp := range pat.defs {
			m := dp.re.FindStringSubmatch(line)
			if m == nil {
				continue
			}
			name := ""
			if dp.nameGroup < len(m) {
				name = m[dp.nameGroup]
			}
			sig := name
			if dp.tailGroup > 0 && dp.tailGroup < len(m) {
				tail := strings.TrimSpace(m[dp.tailGroup])
				// normalize multi-space
				tail = regexp.MustCompile(`\s+`).ReplaceAllString(tail, " ")
				// convert ": T" to "-> T" for return type consistency with tree-sitter spike
				if i := strings.LastIndex(tail, ":"); i > 0 && i < len(tail) {
					// crude: if there's a ":" after the params close paren, treat as return type
					if strings.Contains(tail[:i], ")") {
						tail = tail[:strings.LastIndex(tail[:i], ")")+1] + " -> " + strings.TrimSpace(tail[i+1:])
					}
				}
				sig = name + " " + tail
			}
			defs = append(defs, Def{Line: lineno, Kind: dp.kind, Signature: sig})
			break // only first matching def pattern per line
		}
	}
	return defs, imports
}

// ---------- enumeration + status (copied from b_utility) ----------

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
		if _, ok := langPatterns[ext]; ok {
			f.Lang = strings.TrimPrefix(ext, ".")
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

// ---------- parse (regex) ----------

func parseAll(repo string, files []*File) {
	for _, f := range files {
		if f.Lang == "" {
			continue
		}
		pat, ok := langPatterns[f.Ext]
		if !ok {
			continue
		}
		data, err := os.ReadFile(filepath.Join(repo, f.Path))
		if err != nil {
			continue
		}
		f.Defs, f.Imports = extract(data, pat)
	}
}

// ---------- resolution + graph + render (same as b_utility) ----------

func resolveImports(repo string, files []*File) {
	have := map[string]bool{}
	for _, f := range files {
		have[f.Path] = true
	}
	exts := []string{".ts", ".tsx", ".js", ".jsx", ".mjs", ".kt", ".py", ".go", ".cc", ".cpp", ".h", ".hpp", ".mdx"}
	for _, f := range files {
		for i, imp := range f.Imports {
			if !strings.HasPrefix(imp, ".") {
				continue
			}
			dir := filepath.Dir(f.Path)
			target := filepath.Clean(filepath.Join(dir, imp))
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
	"main.go":     true,
	"index.ts":    true,
	"index.tsx":   true,
	"index.js":    true,
	"server.ts":   true,
	"server.js":   true,
	"app.ts":      true,
	"app.tsx":     true,
	"cli.ts":      true,
	"manage.py":   true,
	"__main__.py": true,
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

func firstTopLevelName(f *File) string {
	if len(f.Defs) == 0 {
		return ""
	}
	s := f.Defs[0].Signature
	if i := strings.IndexAny(s, "( "); i > 0 {
		return s[:i]
	}
	return s
}

func render(files []*File, graph map[string][]string, callers map[string][]string) string {
	var b strings.Builder
	b.WriteString("# tingle-regex v0-spike  tokenizer=cl100k_base\n")
	b.WriteString("# legend: EP=entry U=utility M=module-edge F=file  [M]=modified [untracked]=new-unstaged [test]=test-file  [path:line]=def  f=func c=class m=method\n")
	b.WriteString("# usage: EP answers \"where does execution start\"; U answers \"which files are load-bearing utilities\" with exports+callers inline; M shows dir→dir imports; F has per-file signatures with line anchors\n\n")

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

	sort.Slice(scoredFiles, func(i, j int) bool { return scoredFiles[i].score > scoredFiles[j].score })
	b.WriteString("## Entry points\n")
	eps := 0
	for _, s := range scoredFiles {
		if eps >= 15 || s.score <= 0 {
			break
		}
		name := firstTopLevelName(s.f)
		if name == "" {
			continue
		}
		line := 1
		if len(s.f.Defs) > 0 {
			line = s.f.Defs[0].Line
		}
		fmt.Fprintf(&b, "EP %s:%d %s (out=%d in=%d)\n", s.f.Path, line, name, s.f.OutDeg, s.f.InDeg)
		eps++
	}
	b.WriteString("\n")

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
		}
	}
	b.WriteString("\n")

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
		}
	}
	return b.String()
}

// ---------- main ----------

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: tingle-regex <repo-path>")
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
	fmt.Print(render(files, graph, callers))
}
