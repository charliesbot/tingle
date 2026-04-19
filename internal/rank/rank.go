// Package rank computes the Entry points + Utilities + Modules sections.
//
// Scoring is an equal-weighted blend of deterministic signals:
//
//   - filename conventions (main.go, index.ts, manage.py, etc.)
//   - shebang lines
//   - manifest-declared entries (package.json bin/main, go.mod cmd/*)
//   - out-degree minus in-degree
//   - root-export bonus (file at cmd/, src/, pkg/, internal/ root)
//
// Utility rank is simple in-degree. Every file with in >= 2 qualifies.
package rank

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/charliesbot/tingle/internal/model"
)

// Graph computes all graph edges and populates OutDeg/InDeg on every file.
// Returns dirEdges (src→dsts) and callersByFile (file→list of importers).
func Graph(files []*model.FileIndex) (dirEdges map[string][]string, callers map[string][]string) {
	byPath := make(map[string]*model.FileIndex, len(files))
	for _, f := range files {
		byPath[f.Path] = f
	}

	rawEdges := map[string]map[string]bool{}
	callers = map[string][]string{}

	for _, f := range files {
		src := filepath.Dir(f.Path)
		for _, imp := range f.Imports {
			if target, ok := byPath[imp]; ok {
				target.InDeg++
				callers[imp] = append(callers[imp], f.Path)
				dst := filepath.Dir(imp)
				if dst != "" && dst != src {
					if rawEdges[src] == nil {
						rawEdges[src] = map[string]bool{}
					}
					rawEdges[src][dst] = true
					f.OutDeg++
				}
			}
		}
	}

	dirEdges = make(map[string][]string, len(rawEdges))
	for src, dsts := range rawEdges {
		out := make([]string, 0, len(dsts))
		for d := range dsts {
			out = append(out, d)
		}
		sort.Strings(out)
		dirEdges[src] = out
	}
	for p := range callers {
		sort.Strings(callers[p])
		callers[p] = dedupStrings(callers[p])
	}
	return
}

func dedupStrings(xs []string) []string {
	if len(xs) < 2 {
		return xs
	}
	out := xs[:1]
	for _, x := range xs[1:] {
		if x != out[len(out)-1] {
			out = append(out, x)
		}
	}
	return out
}

// EntryPointsOpts configures EntryPoints ranking.
type EntryPointsOpts struct {
	Repo       string   // absolute path; needed to read shebangs
	ManifestEP []string // file paths declared in manifests (package.json bin/main, go.mod cmd/*)
	MaxEPs     int      // cap on emitted entry points
}

// EntryPoints returns files ranked by the entry-point heuristic, capped at
// opts.MaxEPs with score > 0.
func EntryPoints(files []*model.FileIndex, opts EntryPointsOpts) []*model.FileIndex {
	manifestSet := map[string]bool{}
	for _, p := range opts.ManifestEP {
		manifestSet[p] = true
	}

	type scored struct {
		f     *model.FileIndex
		score int
	}
	list := make([]scored, 0, len(files))
	for _, f := range files {
		if f.Lang == "" || len(f.Defs) == 0 {
			continue
		}
		s := scoreOne(f, opts.Repo, manifestSet)
		if s > 0 {
			list = append(list, scored{f, s})
		}
	}

	sort.SliceStable(list, func(i, j int) bool { return list[i].score > list[j].score })

	max := opts.MaxEPs
	if max <= 0 {
		max = 15
	}
	if max > len(list) {
		max = len(list)
	}
	out := make([]*model.FileIndex, max)
	for i := 0; i < max; i++ {
		out[i] = list[i].f
	}
	return out
}

// Utilities returns every file with in-degree >= 2, sorted descending.
func Utilities(files []*model.FileIndex) []*model.FileIndex {
	out := make([]*model.FileIndex, 0, len(files))
	for _, f := range files {
		if f.InDeg >= 2 {
			out = append(out, f)
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].InDeg > out[j].InDeg })
	return out
}

// --- scoring helpers ---

var conventionEntryFilenames = map[string]bool{
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

var packageRootPrefixes = []string{"cmd/", "src/", "pkg/", "internal/"}

func scoreOne(f *model.FileIndex, repo string, manifestSet map[string]bool) int {
	score := 0
	base := filepath.Base(f.Path)
	if conventionEntryFilenames[base] {
		score += 10
	}
	if strings.HasPrefix(base, "App.") {
		score += 8
	}
	if manifestSet[f.Path] {
		score += 10
	}
	if hasShebang(filepath.Join(repo, f.Path)) {
		score += 10
	}
	for _, prefix := range packageRootPrefixes {
		if strings.HasPrefix(f.Path, prefix) && strings.Count(f.Path[len(prefix):], "/") <= 1 {
			score += 5
			break
		}
	}
	score += f.OutDeg - f.InDeg
	return score
}

// hasShebang checks the first line for #! — best-effort, ignores errors.
func hasShebang(fullPath string) bool {
	file, err := os.Open(fullPath)
	if err != nil {
		return false
	}
	defer file.Close()
	sc := bufio.NewScanner(file)
	if sc.Scan() {
		return strings.HasPrefix(sc.Text(), "#!")
	}
	return false
}

// DebugScore is exposed for tests and CLI --debug flags.
func DebugScore(f *model.FileIndex, repo string, manifestEP []string) string {
	ms := map[string]bool{}
	for _, p := range manifestEP {
		ms[p] = true
	}
	return fmt.Sprintf("%s score=%d (out=%d in=%d)", f.Path, scoreOne(f, repo, ms), f.OutDeg, f.InDeg)
}
