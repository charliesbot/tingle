// Package model defines the shared types used across the tingle pipeline.
//
// Pipeline: enumerate → parse → resolve → rank → render
// Each stage reads and augments *FileIndex values through a Graph.
package model

// SymbolKind is the rendered kind code (one char keeps the output compact).
type SymbolKind string

const (
	KindFunc      SymbolKind = "f"
	KindClass     SymbolKind = "c"
	KindMethod    SymbolKind = "m"
	KindType      SymbolKind = "t"
	KindInterface SymbolKind = "i"
	KindEnum      SymbolKind = "e"
)

// Symbol represents a top-level definition (or a method nested under a class).
type Symbol struct {
	Name      string
	Kind      SymbolKind
	Signature string   // single-line, name first (e.g. "bootstrap (x: string) -> Promise<void>")
	Line      int      // 1-indexed
	Children  []Symbol // methods under a class; empty for standalone defs
}

// FileIndex is everything tingle knows about one file.
type FileIndex struct {
	Path    string
	Ext     string
	Lang    string   // "ts", "kt", "go", "" if unsupported

	// enumerate step
	Tags []string // "M", "untracked", "test"

	// parse step
	Defs    []Symbol
	Imports []string // repo-relative when heuristic-resolvable; else raw

	// rank step (populated downstream)
	OutDeg int
	InDeg  int
}

// Graph is the working in-memory representation of a parsed repo.
type Graph struct {
	Files map[string]*FileIndex
}

// MapOutput is what the renderer consumes. One struct = one invocation's output.
type MapOutput struct {
	Manifests []string    // pre-rendered S records
	Entries   []string    // pre-rendered EP records, ranked
	Utilities []string    // pre-rendered U records, ranked by in-degree
	Edges     []string    // pre-rendered M records (dir → dir)
	Files     []FileIndex // source for F records; renderer walks Defs for line-anchored sigs
	Warnings  []string    // rendered as "# warning: ..." lines in header
}
