// Package render emits the compact tag-prefixed output documented in
// docs/design-doc.md § Output format.
//
// Section order is fixed. Empty sections (no manifests, no modules, no
// utilities) are omitted rather than rendered with a blank body.
package render

import (
	"fmt"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/charliesbot/tingle/internal/model"
)

// Options controls header + legend emission.
type Options struct {
	Version     string // tingle binary version
	Commit      string // short git commit of target repo; "" if unavailable
	TokenizerID string // e.g. "cl100k_base"
	NoLegend    bool   // --no-legend: skip the legend line
	TokensApprox int   // pre-computed approximate token count; 0 to omit
}

const legendLine = "# legend: S=manifest EP=entry U=utility M=module-edge F=file  [M]=modified [untracked]=new-unstaged [test]=test-file  [path:line]=def  f=func c=class m=method i=interface t=type e=enum"

// Render emits the compact map to a string.
func Render(files []*model.FileIndex, entries, utilities []*model.FileIndex, dirEdges map[string][]string, callers map[string][]string, manifests []string, opts Options) string {
	var b strings.Builder

	// Header
	ver := opts.Version
	if ver == "" {
		ver = "v0"
	}
	commit := opts.Commit
	if commit != "" {
		commit = "  commit=" + commit
	}
	tok := ""
	if opts.TokensApprox > 0 {
		tok = fmt.Sprintf("  tokens~%s", humanToken(opts.TokensApprox))
	}
	tknzr := opts.TokenizerID
	if tknzr == "" {
		tknzr = "cl100k_base"
	}
	fmt.Fprintf(&b, "# tingle %s  gen=%s%s  files=%d%s  tokenizer=%s\n",
		ver, time.Now().UTC().Format("2006-01-02"), commit, countParsed(files), tok, tknzr)

	if !opts.NoLegend {
		b.WriteString(legendLine + "\n")
	}
	b.WriteString("\n")

	// Manifests
	if len(manifests) > 0 {
		b.WriteString("## Manifests\n")
		for _, m := range manifests {
			b.WriteString(m + "\n")
		}
		b.WriteString("\n")
	}

	// Entry points
	if len(entries) > 0 {
		b.WriteString("## Entry points\n")
		for _, f := range entries {
			name := firstDefName(f)
			line := firstDefLine(f)
			fmt.Fprintf(&b, "EP %s:%d %s (out=%d in=%d)\n", f.Path, line, name, f.OutDeg, f.InDeg)
		}
		b.WriteString("\n")
	}

	// Utilities — show exports + top callers inline so agent cites directly
	if len(utilities) > 0 {
		b.WriteString("## Utilities\n")
		for _, f := range utilities {
			cs := callers[f.Path]
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
			fmt.Fprintf(&b, "U %s (in=%d)%s\n", f.Path, f.InDeg, callerStr)
			writeDefs(&b, f.Defs)
		}
		b.WriteString("\n")
	}

	// Modules
	if len(dirEdges) > 0 {
		b.WriteString("## Modules\n")
		srcs := make([]string, 0, len(dirEdges))
		for s := range dirEdges {
			srcs = append(srcs, s)
		}
		sort.Strings(srcs)
		for _, src := range srcs {
			fmt.Fprintf(&b, "M %s -> %s\n", src, strings.Join(dirEdges[src], " "))
		}
		b.WriteString("\n")
	}

	// Files
	b.WriteString("## Files\n")
	sorted := make([]*model.FileIndex, 0, len(files))
	for _, f := range files {
		if f.Lang == "" && len(f.Tags) == 0 {
			continue // skip unparsed + untagged (typical binaries, config files)
		}
		sorted = append(sorted, f)
	}
	sort.SliceStable(sorted, func(i, j int) bool { return sorted[i].Path < sorted[j].Path })
	for _, f := range sorted {
		tagStr := ""
		for _, t := range f.Tags {
			tagStr += "[" + t + "]"
		}
		imps := ""
		if len(f.Imports) > 0 {
			imps = "  imp: " + strings.Join(f.Imports, " ")
		}
		fmt.Fprintf(&b, "F %s %s%s\n", f.Path, tagStr, imps)
		writeDefs(&b, f.Defs)
	}

	return b.String()
}

func writeDefs(b *strings.Builder, defs []model.Symbol) {
	for _, d := range defs {
		fmt.Fprintf(b, " %d %s %s\n", d.Line, d.Kind, d.Signature)
		for _, m := range d.Children {
			fmt.Fprintf(b, "  %d %s %s\n", m.Line, m.Kind, m.Signature)
		}
	}
}

func firstDefName(f *model.FileIndex) string {
	if len(f.Defs) == 0 {
		return filepath.Base(f.Path)
	}
	return f.Defs[0].Name
}

func firstDefLine(f *model.FileIndex) int {
	if len(f.Defs) == 0 {
		return 1
	}
	return f.Defs[0].Line
}

func countParsed(files []*model.FileIndex) int {
	n := 0
	for _, f := range files {
		if f.Lang != "" {
			n++
		}
	}
	return n
}

func humanToken(n int) string {
	if n < 1000 {
		return fmt.Sprintf("%d", n)
	}
	return fmt.Sprintf("%.1fk", float64(n)/1000)
}
