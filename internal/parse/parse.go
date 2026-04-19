// Package parse runs tree-sitter queries (aider-style tags.scm files) against
// source files to extract definitions and imports.
//
// Uses gotreesitter — pure Go tree-sitter runtime. No cgo.
// Language-agnostic extractor: per-language work lives in the .scm query
// files under queries/. Standard capture names are consumed:
// @definition.function, @name.definition.class, @reference.import, etc.
package parse

import (
	"context"
	_ "embed"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"
	"sync/atomic"

	"github.com/odvcencio/gotreesitter"
	"github.com/odvcencio/gotreesitter/grammars"
	"golang.org/x/sync/errgroup"

	"github.com/charliesbot/tingle/internal/model"
)

// Stats aggregate counters exposed for optional instrumentation.
type Stats struct {
	ParsedOK    atomic.Int64
	ReadErrors  atomic.Int64
	ParseErrors atomic.Int64
}

// PackageStats is a process-wide stats sink. Consumers can read it after All
// returns to get per-run counts. Reset via NewRun().
var PackageStats = &Stats{}

// NewRun resets per-run counters. Call before a fresh All() if you want clean
// stats for that run.
func NewRun() { PackageStats.reset() }

func (s *Stats) reset() {
	s.ParsedOK.Store(0)
	s.ReadErrors.Store(0)
	s.ParseErrors.Store(0)
}

//go:embed queries/typescript-tags.scm
var tsQuery string

//go:embed queries/tsx-tags.scm
var tsxQuery string

//go:embed queries/javascript-tags.scm
var jsQuery string

//go:embed queries/python-tags.scm
var pyQuery string

//go:embed queries/go-tags.scm
var goQuery string

//go:embed queries/kotlin-tags.scm
var ktQuery string

//go:embed queries/cpp-tags.scm
var cppQuery string

type language struct {
	name    string
	grammar *gotreesitter.Language
	query   string
}

var languages = map[string]language{
	".ts":  {"ts", grammars.TypescriptLanguage(), tsQuery},
	".tsx": {"tsx", grammars.TsxLanguage(), tsxQuery},
	".js":  {"js", grammars.JavascriptLanguage(), jsQuery},
	".jsx": {"jsx", grammars.JavascriptLanguage(), jsQuery},
	".mjs": {"mjs", grammars.JavascriptLanguage(), jsQuery},
	".py":  {"py", grammars.PythonLanguage(), pyQuery},
	".go":  {"go", grammars.GoLanguage(), goQuery},
	".kt":  {"kt", grammars.KotlinLanguage(), ktQuery},
	".kts": {"kts", grammars.KotlinLanguage(), ktQuery},
	".cc":  {"cpp", grammars.CppLanguage(), cppQuery},
	".cpp": {"cpp", grammars.CppLanguage(), cppQuery},
	".cxx": {"cpp", grammars.CppLanguage(), cppQuery},
	".h":   {"cpp", grammars.CppLanguage(), cppQuery},
	".hpp": {"cpp", grammars.CppLanguage(), cppQuery},
	".hxx": {"cpp", grammars.CppLanguage(), cppQuery},
}

// Precompile queries once. gotreesitter queries are safe to share across
// goroutines after construction.
var compiledQueries = sync.OnceValue(func() map[string]*gotreesitter.Query {
	out := map[string]*gotreesitter.Query{}
	for ext, l := range languages {
		q, err := gotreesitter.NewQuery(l.query, l.grammar)
		if err != nil {
			// Broken query = build-time bug. Panic surfaces it loudly in tests.
			panic("tingle/parse: invalid query for " + l.name + " (" + ext + "): " + err.Error())
		}
		out[ext] = q
	}
	return out
})

// Per-language parser pools. Reusing a parser (via Reset) across files within
// a pool amortizes allocator setup cost.
var parserPools = sync.OnceValue(func() map[string]*sync.Pool {
	out := map[string]*sync.Pool{}
	for ext, l := range languages {
		lang := l.grammar // capture for closure
		out[ext] = &sync.Pool{
			New: func() any { return gotreesitter.NewParser(lang) },
		}
	}
	return out
})

// parseWorkers caps concurrent parse goroutines. Trades peak memory for
// parallelism. gotreesitter's per-parser arena is memory-hungry, so we
// stay conservative: 2 workers keeps peak RSS bounded even on multi-language
// repos with heavy C++ grammar.
const parseWorkers = 2

// All parses every file in the slice that has a registered language. Files
// with unknown extensions are left with empty Defs/Imports.
func All(repo string, files []*model.FileIndex) {
	queries := compiledQueries()
	pools := parserPools()
	g, ctx := errgroup.WithContext(context.Background())
	g.SetLimit(parseWorkers)

	for _, f := range files {
		f := f
		lang, ok := languages[f.Ext]
		if !ok {
			continue
		}
		f.Lang = lang.name
		query := queries[f.Ext]
		pool := pools[f.Ext]

		g.Go(func() error {
			if ctx.Err() != nil {
				return ctx.Err()
			}
			data, err := os.ReadFile(filepath.Join(repo, f.Path))
			if err != nil {
				PackageStats.ReadErrors.Add(1)
				return nil
			}

			parser := pool.Get().(*gotreesitter.Parser)

			tree, err := parser.Parse(data)
			if err != nil || tree == nil {
				PackageStats.ParseErrors.Add(1)
				pool.Put(parser) // safe to return — no live tree borrows parser state
				return nil
			}

			// Extract while the tree is still live. DO NOT return the parser to
			// the pool until we're done reading nodes — another goroutine could
			// pull it and Parse(), invalidating tree's internal pointers.
			defs, imports := extractOne(query, tree.RootNode(), data, lang.grammar)
			f.Defs = defs
			f.Imports = imports
			PackageStats.ParsedOK.Add(1)

			// Release the tree's arena back to the runtime before returning the
			// parser to the pool. This prevents tree allocations from piling up
			// across files parsed by the same pooled parser.
			tree.Release()
			pool.Put(parser)
			return nil
		})
	}
	_ = g.Wait()
}

// extractOne runs the precompiled query against a parsed tree and returns the
// extracted defs + imports.
func extractOne(query *gotreesitter.Query, root *gotreesitter.Node, src []byte, lang *gotreesitter.Language) ([]model.Symbol, []string) {
	cursor := query.Exec(root, lang, src)

	type rawDef struct {
		kind      string
		nameNode  *gotreesitter.Node
		outerNode *gotreesitter.Node
	}
	var classes, methods, funcs []rawDef
	var imports []string
	seenImport := map[string]bool{}

	for {
		match, ok := cursor.NextMatch()
		if !ok {
			break
		}
		var def rawDef
		for _, cap := range match.Captures {
			name := cap.Name
			switch {
			case name == "name.reference.import":
				raw := strings.Trim(cap.Node.Text(src), "\"'`<>")
				if raw != "" && !seenImport[raw] {
					imports = append(imports, raw)
					seenImport[raw] = true
				}
			case strings.HasPrefix(name, "name.definition."):
				def.nameNode = cap.Node
			case strings.HasPrefix(name, "definition."):
				def.outerNode = cap.Node
				def.kind = strings.TrimPrefix(name, "definition.")
			}
		}

		if def.outerNode == nil || def.nameNode == nil {
			continue
		}
		switch def.kind {
		case "method":
			methods = append(methods, def)
		case "class", "interface", "enum", "type", "object", "module":
			classes = append(classes, def)
		default:
			funcs = append(funcs, def)
		}
	}

	// Attach methods to enclosing classes by byte-range containment.
	classSymbols := make([]model.Symbol, 0, len(classes))
	attached := make([]bool, len(methods))
	for _, c := range classes {
		cs := buildSymbol(c.kind, c.nameNode, c.outerNode, src, model.KindClass)
		for i, m := range methods {
			if contains(c.outerNode, m.outerNode) {
				cs.Children = append(cs.Children, buildSymbol(m.kind, m.nameNode, m.outerNode, src, model.KindMethod))
				attached[i] = true
			}
		}
		classSymbols = append(classSymbols, cs)
	}

	funcSymbols := make([]model.Symbol, 0, len(funcs)+len(methods))
	for _, f := range funcs {
		funcSymbols = append(funcSymbols, buildSymbol(f.kind, f.nameNode, f.outerNode, src, model.KindFunc))
	}
	for i, m := range methods {
		if !attached[i] {
			funcSymbols = append(funcSymbols, buildSymbol(m.kind, m.nameNode, m.outerNode, src, model.KindFunc))
		}
	}

	all := append(classSymbols, funcSymbols...)
	sortByLine(all)
	return all, imports
}

func buildSymbol(queryKind string, nameNode, outerNode *gotreesitter.Node, src []byte, fallback model.SymbolKind) model.Symbol {
	name := nameNode.Text(src)
	sig := renderSignature(name, nameNode, outerNode, src)
	return model.Symbol{
		Name:      name,
		Kind:      kindFromQuery(queryKind, fallback),
		Signature: sig,
		Line:      int(outerNode.StartPoint().Row) + 1,
	}
}

func kindFromQuery(q string, fallback model.SymbolKind) model.SymbolKind {
	switch q {
	case "function":
		return model.KindFunc
	case "class", "object", "module":
		return model.KindClass
	case "interface":
		return model.KindInterface
	case "method":
		return model.KindMethod
	case "type":
		return model.KindType
	case "enum":
		return model.KindEnum
	}
	return fallback
}

// renderSignature returns "name (params) -> return" best-effort, single-line.
func renderSignature(name string, nameNode, outerNode *gotreesitter.Node, src []byte) string {
	start := nameNode.EndByte()
	end := outerNode.EndByte()
	if start >= uint32(len(src)) || end <= start {
		return name
	}
	const maxTail = 400
	if end-start > maxTail {
		end = start + maxTail
	}
	tail := string(src[start:end])
	for _, stop := range []string{"{", "=>", " where ", ";", "\n\n"} {
		if i := strings.Index(tail, stop); i >= 0 {
			tail = tail[:i]
		}
	}
	tail = collapseWS.ReplaceAllString(tail, " ")
	tail = strings.TrimSpace(tail)
	tail = strings.TrimPrefix(tail, "= ")
	if idx := strings.LastIndex(tail, ")"); idx >= 0 && idx < len(tail)-1 {
		after := strings.TrimSpace(tail[idx+1:])
		if strings.HasPrefix(after, ":") {
			tail = tail[:idx+1] + " -> " + strings.TrimSpace(strings.TrimPrefix(after, ":"))
		}
	}
	if tail == "" {
		return name
	}
	const maxSig = 180
	if len(tail) > maxSig {
		tail = tail[:maxSig] + "…"
	}
	return name + " " + tail
}

var collapseWS = regexp.MustCompile(`\s+`)

func contains(outer, inner *gotreesitter.Node) bool {
	return inner.StartByte() >= outer.StartByte() && inner.EndByte() <= outer.EndByte()
}

func sortByLine(defs []model.Symbol) {
	sort.Slice(defs, func(i, j int) bool { return defs[i].Line < defs[j].Line })
}
